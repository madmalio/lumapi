#!/usr/bin/env python3

import argparse
import collections
import io
import json
import os
import shutil
import socket
import subprocess
import threading
import traceback
import time
import wave
from datetime import datetime

from libcamera import controls
from picamera2 import Picamera2
from picamera2.encoders import H264Encoder, JpegEncoder
from picamera2.outputs import FileOutput
from picamera2.outputs import CircularOutput2
from picamera2.outputs.output import Output

try:
    from picamera2.outputs import PyavOutput
except ImportError:  # pragma: no cover - depends on Pi packages
    PyavOutput = None

try:
    from picamera2.outputs import FfmpegOutput
except ImportError:  # pragma: no cover - depends on Pi packages
    FfmpegOutput = None


STREAM_HOST = "127.0.0.1"
STREAM_PORT = 5000
CONTROL_HOST = "127.0.0.1"
CONTROL_PORT = 5001
RECORDINGS_DIR = "/home/pi/lumapi-cam/recordings"
RECORD_PREROLL_MS = 140
AUDIO_SAMPLE_RATE = 48000
AUDIO_CHANNELS = 1
AUDIO_SYNC_OFFSET = 0.0
AUDIO_SAMPLE_WIDTH_BYTES = 2
AUDIO_CHUNK_MS = 20
AUDIO_CHUNK_FRAMES = AUDIO_SAMPLE_RATE * AUDIO_CHUNK_MS // 1000
AUDIO_CHUNK_BYTES = AUDIO_CHUNK_FRAMES * AUDIO_CHANNELS * AUDIO_SAMPLE_WIDTH_BYTES


class SwitchableOutput(Output):
    def __init__(self):
        super().__init__()
        self._lock = threading.Lock()
        self._output = None
        self._streams = []

    def open_output(self, output):
        output.start()
        for encoder_stream, codec, kwargs in self._streams:
            output._add_stream(encoder_stream, codec, **kwargs)
        with self._lock:
            self._output = output

    def close_output(self):
        with self._lock:
            output = self._output
            self._output = None
        if output is not None:
            output.stop()

    def outputframe(self, frame, keyframe=True, timestamp=None, packet=None, audio=False):
        with self._lock:
            output = self._output
        if output is not None:
            output.outputframe(frame, keyframe, timestamp, packet, audio)

    def _add_stream(self, encoder_stream, codec_name, **kwargs):
        self._streams.append((encoder_stream, codec_name, kwargs))

    def stop(self):
        self.close_output()
        super().stop()


class StreamingOutput(io.BufferedIOBase):
    def __init__(self):
        super().__init__()
        self._clients = set()
        self._lock = threading.Lock()

    def writable(self):
        return True

    def add_client(self, client_socket):
        client_socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        with self._lock:
            self._clients.add(client_socket)

    def remove_client(self, client_socket):
        with self._lock:
            self._clients.discard(client_socket)
        try:
            client_socket.close()
        except OSError:
            pass

    def write(self, buf):
        dead_clients = []
        with self._lock:
            clients = tuple(self._clients)

        for client_socket in clients:
            try:
                client_socket.sendall(buf)
            except OSError:
                dead_clients.append(client_socket)

        for client_socket in dead_clients:
            self.remove_client(client_socket)


class CameraService:
    def __init__(self, width, height):
        self.picam2 = Picamera2()
        self.output = StreamingOutput()
        self.width = width
        self.height = height
        self.running = True
        os.makedirs(RECORDINGS_DIR, exist_ok=True)
        self._control_lock = threading.Lock()
        self._record_lock = threading.RLock()
        self._requested = {
            "fps": 30.0,
            "shutter_us": 16667,
            "analogue_gain": 1.0,
            "awb_mode": "auto",
            "full_auto": False,
            "recording_format": "mp4",
        }
        self._record_encoder = H264Encoder(12_000_000)
        self._record_output = CircularOutput2(buffer_duration_ms=RECORD_PREROLL_MS)
        self._is_recording = False
        self._recording_path = None
        self._recording_has_audio = False
        self._recording_output_label = "idle"
        self._recording_video_path = None
        self._audio_capture_process = None
        self._audio_capture_device = None
        self._audio_capture_thread = None
        self._audio_running = False
        self._audio_preroll = collections.deque(maxlen=max(1, RECORD_PREROLL_MS // AUDIO_CHUNK_MS + 2))
        self._active_audio_chunks = None

    def start(self, initial_controls):
        config = self.picam2.create_video_configuration(
            main={"size": (self.width, self.height)},
            lores={"size": (self.width, self.height), "format": "YUV420"},
        )
        self.picam2.configure(config)
        self.picam2.start_recording(JpegEncoder(), FileOutput(self.output), name="lores")
        self.picam2.start_encoder(self._record_encoder, self._record_output, name="main")
        self.start_audio_capture()
        time.sleep(0.2)
        self.apply_controls(initial_controls)

    def stop(self):
        self.running = False
        if self._is_recording:
            self.stop_recording()
        self.stop_audio_capture()
        self.picam2.stop_encoder(self._record_encoder)
        self.picam2.stop_recording()

    def apply_controls(self, payload):
        fps = max(float(payload.get("fps", 30)), 1.0)
        shutter_us = max(int(payload.get("shutter_us", 16667)), 100)
        analogue_gain = max(float(payload.get("analogue_gain", 1.0)), 1.0)
        awb_mode = str(payload.get("awb_mode", "auto")).lower()
        full_auto = bool(payload.get("full_auto", False))
        recording_format = str(payload.get("recording_format", "mp4")).lower()

        self._requested = {
            "fps": fps,
            "shutter_us": shutter_us,
            "analogue_gain": analogue_gain,
            "awb_mode": awb_mode,
            "full_auto": full_auto,
            "recording_format": recording_format,
        }

        camera_controls = {
            "FrameRate": fps,
        }

        if full_auto:
            camera_controls["AeEnable"] = True
        else:
            camera_controls["ExposureTime"] = shutter_us
            camera_controls["AnalogueGain"] = analogue_gain
            camera_controls["AeEnable"] = False

        if full_auto or awb_mode == "auto":
            camera_controls["AwbEnable"] = True
            camera_controls["AwbMode"] = controls.AwbModeEnum.Auto
        else:
            camera_controls["AwbEnable"] = True
            camera_controls["AwbMode"] = {
                "tungsten": controls.AwbModeEnum.Tungsten,
                "fluorescent": controls.AwbModeEnum.Fluorescent,
                "daylight": controls.AwbModeEnum.Daylight,
                "cloudy": controls.AwbModeEnum.Cloudy,
            }.get(awb_mode, controls.AwbModeEnum.Auto)

        with self._control_lock:
            self.picam2.set_controls(camera_controls)

    def get_status(self):
        with self._control_lock:
            metadata = self.picam2.capture_metadata()

        frame_duration = metadata.get("FrameDuration") or int(1_000_000 / max(self._requested["fps"], 1.0))
        exposure_time = metadata.get("ExposureTime") or self._requested["shutter_us"]
        analogue_gain = metadata.get("AnalogueGain") or self._requested["analogue_gain"]
        colour_temperature = metadata.get("ColourTemperature")
        colour_gains = metadata.get("ColourGains") or []

        return {
            "ok": True,
            "fps": round(1_000_000.0 / max(frame_duration, 1), 2),
            "frame_duration_us": frame_duration,
            "exposure_time_us": exposure_time,
            "analogue_gain": round(float(analogue_gain), 3),
            "iso": int(round(float(analogue_gain) * 100.0)),
            "colour_temperature": colour_temperature,
            "awb_mode": self._requested["awb_mode"],
            "full_auto": self._requested["full_auto"],
            "recording_format": self._requested["recording_format"],
            "colour_gains": [round(float(value), 3) for value in colour_gains],
            "is_recording": self._is_recording,
            "recording_path": self._recording_path,
            "recording_has_audio": self._recording_has_audio,
            "recording_output_label": self._recording_output_label,
        }

    def get_quick_status(self):
        frame_duration = int(1_000_000 / max(self._requested["fps"], 1.0))

        return {
            "ok": True,
            "fps": round(self._requested["fps"], 2),
            "frame_duration_us": frame_duration,
            "exposure_time_us": self._requested["shutter_us"],
            "analogue_gain": round(float(self._requested["analogue_gain"]), 3),
            "iso": int(round(float(self._requested["analogue_gain"]) * 100.0)),
            "awb_mode": self._requested["awb_mode"],
            "full_auto": self._requested["full_auto"],
            "recording_format": self._requested["recording_format"],
            "is_recording": self._is_recording,
            "recording_path": self._recording_path,
            "recording_has_audio": self._recording_has_audio,
            "recording_output_label": self._recording_output_label,
        }

    def toggle_recording(self):
        with self._record_lock:
            if self._is_recording:
                self.stop_recording()
            else:
                self.start_recording()

        return self.get_quick_status()

    def start_recording(self):
        recording_format = self._requested.get("recording_format", "mp4")
        filename = datetime.now().strftime(f"clip_%Y%m%d_%H%M%S.{recording_format}")
        self._recording_path = os.path.join(RECORDINGS_DIR, filename)
        self._recording_video_path = f"{self._recording_path}.h264"
        outputs = self._create_record_outputs(self._recording_video_path)
        last_error = None

        for output, has_audio in outputs:
            try:
                self._record_output.open_output(output)
                self._recording_has_audio = self._audio_capture_process is not None
                self._recording_output_label = describe_record_output(output, has_audio)
                print(f"recording output active: {self._recording_output_label}", flush=True)
                break
            except Exception as error:
                print(f"recording output failed: {describe_record_output(output, has_audio)} :: {error}", flush=True)
                last_error = error
        else:
            self._recording_path = None
            self._recording_video_path = None
            self._recording_has_audio = False
            self._recording_output_label = "failed"
            raise last_error or RuntimeError("No recording outputs available")

        with self._record_lock:
            self._active_audio_chunks = list(self._audio_preroll)

        self._record_encoder.force_key_frame()
        self._is_recording = True

    def stop_recording(self):
        self._record_output.close_output()
        self._is_recording = False
        with self._record_lock:
            audio_chunks = list(self._active_audio_chunks or [])
            self._active_audio_chunks = None
        audio_bytes = sum(len(chunk) for chunk in audio_chunks)
        final_path = self._recording_path
        video_path = self._recording_video_path
        recording_has_audio = self._recording_has_audio
        self._recording_output_label = "idle"

        print(f"recording stop: captured {len(audio_chunks)} audio chunks / {audio_bytes} bytes", flush=True)

        if final_path and video_path:
            finalize_thread = threading.Thread(
                target=self.finalize_recording,
                args=(video_path, final_path, audio_chunks, recording_has_audio),
                daemon=True,
            )
            finalize_thread.start()

        self._recording_has_audio = False
        self._recording_path = None
        self._recording_video_path = None

    def _create_record_outputs(self, path):
        return [(FileOutput(path), False)]

    def start_audio_capture(self):
        for audio_device in record_audio_device_candidates():
            process = spawn_audio_capture_process(audio_device)
            if process is None:
                continue

            self._audio_capture_process = process
            self._audio_capture_device = audio_device
            self._audio_running = True
            self._audio_capture_thread = threading.Thread(target=self.audio_capture_loop, daemon=True)
            self._audio_capture_thread.start()
            print(f"audio capture active: {audio_device}", flush=True)
            return

        print("audio capture unavailable for warm recording", flush=True)

    def stop_audio_capture(self):
        self._audio_running = False
        if self._audio_capture_process is not None:
            try:
                self._audio_capture_process.terminate()
            except Exception:
                pass
        if self._audio_capture_thread is not None:
            self._audio_capture_thread.join(timeout=1)
            self._audio_capture_thread = None
        if self._audio_capture_process is not None:
            try:
                self._audio_capture_process.wait(timeout=1)
            except Exception:
                pass
        self._audio_capture_process = None
        self._audio_capture_device = None

    def audio_capture_loop(self):
        while self._audio_running and self._audio_capture_process is not None:
            stdout = self._audio_capture_process.stdout
            if stdout is None:
                return
            chunk = stdout.read(AUDIO_CHUNK_BYTES)
            if not chunk:
                return
            with self._record_lock:
                self._audio_preroll.append(chunk)
                if self._active_audio_chunks is not None:
                    self._active_audio_chunks.append(chunk)

    def finalize_recording(self, video_path, final_path, audio_chunks, recording_has_audio):
        if not os.path.exists(video_path):
            return

        muxed_path = temporary_mux_path(final_path)
        audio_bytes = sum(len(chunk) for chunk in audio_chunks)

        if recording_has_audio and audio_bytes > 0 and shutil.which("ffmpeg"):
            audio_path = f"{final_path}.wav"
            try:
                write_audio_wav(audio_path, audio_chunks)
                print(f"recording finalize: muxing {video_path} with {audio_bytes} audio bytes", flush=True)
                mux_audio_and_video(video_path, audio_path, muxed_path)
                os.replace(muxed_path, final_path)
                os.remove(video_path)
                os.remove(audio_path)
                print(f"recording finalized with audio: {final_path}", flush=True)
                return
            except Exception as error:
                print(f"recording audio mux fallback to video-only: {error}", flush=True)
                for path in (audio_path, muxed_path):
                    if path and os.path.exists(path):
                        os.remove(path)

        remux_video_only(video_path, muxed_path)
        os.replace(muxed_path, final_path)
        os.remove(video_path)
        print(f"recording finalized video-only: {final_path}", flush=True)


def record_audio_device_candidates():
    devices = []

    configured = (os.environ.get("LUMAPI_RECORD_AUDIO_DEVICE") or os.environ.get("LUMAPI_AUDIO_DEVICE") or "").strip()
    if configured:
        if configured.startswith("hw:"):
            devices.append(configured.replace("hw:", "dsnoop:", 1))
            devices.append(configured.replace("hw:", "plughw:", 1))
        devices.append(configured)

    for device in [
        "dsnoop:CARD=Device,DEV=0",
        "plughw:CARD=Device,DEV=0",
        "default:CARD=Device",
        "default",
        "plughw:3,0",
        "hw:3,0",
    ]:
        if device not in devices:
            devices.append(device)

    return devices


def spawn_audio_capture_process(audio_device):
    if shutil.which("arecord") is None:
        return None

    command = [
        "arecord",
        "-D",
        audio_device,
        "-f",
        "S16_LE",
        "-c",
        str(AUDIO_CHANNELS),
        "-r",
        str(AUDIO_SAMPLE_RATE),
        "-t",
        "raw",
        "-q",
        "-",
    ]

    try:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except Exception:
        return None

    time.sleep(0.15)
    if process.poll() is not None:
        stderr = b""
        if process.stderr is not None:
            stderr = process.stderr.read()
        print(
            f"audio capture failed: {audio_device} :: {stderr.decode('utf-8', errors='replace').strip() or 'unknown error'}",
            flush=True,
        )
        return None

    return process


def write_audio_wav(path, chunks):
    with wave.open(path, "wb") as wav_file:
        wav_file.setnchannels(AUDIO_CHANNELS)
        wav_file.setsampwidth(AUDIO_SAMPLE_WIDTH_BYTES)
        wav_file.setframerate(AUDIO_SAMPLE_RATE)
        for chunk in chunks:
            wav_file.writeframes(chunk)


def mux_audio_and_video(video_path, audio_path, output_path):
    if shutil.which("ffmpeg") is None:
        raise RuntimeError("ffmpeg is not installed")

    output_ext = os.path.splitext(output_path)[1].lower()
    is_mp4_family = output_ext in {".mp4", ".mov", ".m4v"}

    codec_variants = [
        ["-c:a", "aac", "-b:a", "128k"],
    ]
    if output_ext == ".mkv":
        codec_variants.insert(0, ["-c:a", "libopus", "-b:a", "96k"])

    attempts = []
    for codec_args in codec_variants:
        for use_shortest in (True, False):
            command = [
                "ffmpeg",
                "-loglevel",
                "warning",
                "-y",
                "-fflags",
                "+genpts",
                "-f",
                "h264",
                "-i",
                video_path,
                "-itsoffset",
                str(AUDIO_SYNC_OFFSET),
                "-i",
                audio_path,
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "copy",
                *codec_args,
            ]

            if is_mp4_family:
                command.extend(["-movflags", "+faststart"])
            if use_shortest:
                command.append("-shortest")

            command.append(output_path)
            attempts.append(command)

    last_error = None
    for index, command in enumerate(attempts):
        try:
            run_checked_command(command, f"ffmpeg mux attempt {index + 1} failed")
            if output_has_audio_stream(output_path):
                return
            last_error = RuntimeError(f"ffmpeg mux attempt {index + 1} produced no audio stream")
        except Exception as error:
            last_error = error

    raise RuntimeError(str(last_error) if last_error else "ffmpeg mux produced output without an audio stream")


def output_has_audio_stream(path):
    if shutil.which("ffprobe") is None:
        return True

    command = [
        "ffprobe",
        "-v",
        "error",
        "-count_packets",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=index,nb_read_packets,duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        path,
    ]

    completed = subprocess.run(command, capture_output=True)
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(stderr or "ffprobe stream validation failed")

    fields = [line.strip() for line in completed.stdout.decode("utf-8", errors="replace").splitlines() if line.strip()]
    if len(fields) < 3:
        return False

    packet_count = 0
    try:
        packet_count = int(fields[1])
    except ValueError:
        packet_count = 0

    duration = 0.0
    if fields[2] != "N/A":
        try:
            duration = float(fields[2])
        except ValueError:
            duration = 0.0

    return packet_count > 0 or duration > 0.0


def run_checked_command(command, default_error):
    completed = subprocess.run(command, capture_output=True)
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(stderr or default_error)


def remux_video_only(video_path, output_path):
    if shutil.which("ffmpeg") is None:
        raise RuntimeError("ffmpeg is not installed")

    command = [
        "ffmpeg",
        "-loglevel",
        "warning",
        "-y",
        "-f",
        "h264",
        "-i",
        video_path,
        "-c:v",
        "copy",
        output_path,
    ]

    completed = subprocess.run(command, capture_output=True)
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(stderr or "ffmpeg video remux failed")


def temporary_mux_path(final_path):
    root, ext = os.path.splitext(final_path)
    if not ext:
        return f"{final_path}.tmp"
    return f"{root}.tmp{ext}"


def describe_record_output(output, has_audio):
    if isinstance(output, FileOutput):
        return "raw h264 video-only"
    return f"{'audio' if has_audio else 'video'} output"


def stream_server(output):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server_socket:
        server_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server_socket.bind((STREAM_HOST, STREAM_PORT))
        server_socket.listen()

        while True:
            client_socket, _ = server_socket.accept()
            output.add_client(client_socket)


def control_server(camera_service):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server_socket:
        server_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server_socket.bind((CONTROL_HOST, CONTROL_PORT))
        server_socket.listen()

        while camera_service.running:
            client_socket, _ = server_socket.accept()
            with client_socket:
                payload = client_socket.recv(4096)
                if not payload:
                    continue

                try:
                    request = json.loads(payload.decode("utf-8"))
                    command = request.get("command", "apply")

                    if command == "status":
                        response = camera_service.get_status()
                    elif command == "toggle_record":
                        response = camera_service.toggle_recording()
                    else:
                        camera_service.apply_controls(request)
                        time.sleep(0.05)
                        response = camera_service.get_status()

                    client_socket.sendall(json.dumps(response).encode("utf-8"))
                except Exception as error:  # pragma: no cover - runtime path on Pi
                    client_socket.sendall(json.dumps({"ok": False, "error": str(error)}).encode("utf-8"))


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--width", type=int, default=1140)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--fps", type=float, default=30.0)
    parser.add_argument("--shutter-us", type=int, default=16667)
    parser.add_argument("--analogue-gain", type=float, default=1.0)
    parser.add_argument("--awb-mode", type=str, default="auto")
    parser.add_argument("--recording-format", type=str, default="mp4")
    return parser.parse_args()


def main():
    args = parse_args()
    camera_service = CameraService(args.width, args.height)
    initial_controls = {
        "fps": args.fps,
        "shutter_us": args.shutter_us,
        "analogue_gain": args.analogue_gain,
        "awb_mode": args.awb_mode,
        "recording_format": args.recording_format,
    }
    try:
        stream_thread = threading.Thread(target=stream_server, args=(camera_service.output,), daemon=True)
        control_thread = threading.Thread(target=control_server, args=(camera_service,), daemon=True)
        stream_thread.start()
        control_thread.start()
        camera_service.start(initial_controls)

        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    except Exception:  # pragma: no cover - runtime path on Pi
        traceback.print_exc()
        raise
    finally:
        camera_service.stop()


if __name__ == "__main__":
    main()
