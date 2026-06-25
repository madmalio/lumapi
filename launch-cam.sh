#!/bin/bash

PLAYBACK_REQUEST_PATH="/tmp/lumapi-playback-request"
PLAYBACK_LOG_PATH="/tmp/lumapi-media-playback.log"

# Hide cursor and clear console at startup
sudo sh -c 'tput civis > /dev/tty1 2>/dev/null || true'
sudo sh -c 'clear > /dev/tty1 2>/dev/null || true'
sudo dd if=/dev/zero of=/dev/fb0 2>/dev/null || true

cleanup_runtime() {
  sudo pkill -f "/home/pi/lumapi-cam/camera_service.py" 2>/dev/null || true
  sudo killall -9 rpicam-vid lumapi-cam lumapi-hud-test 2>/dev/null || true
  sleep 0.3
}

play_requested_clip() {
  if [ ! -f "$PLAYBACK_REQUEST_PATH" ]; then
    return
  fi

  clip_path="$(sudo head -n 1 "$PLAYBACK_REQUEST_PATH" | tr -d '\r')"
  sudo rm -f "$PLAYBACK_REQUEST_PATH"

  if [ -z "$clip_path" ] || [ ! -f "$clip_path" ]; then
    echo "playback: request missing clip path or file: $clip_path" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
    return
  fi

  runtime_dir="${XDG_RUNTIME_DIR:-}"
  if [ -z "$runtime_dir" ]; then
    if [ -d /run/user/1000 ]; then
      runtime_dir="/run/user/1000"
    elif [ -d /run/user/0 ]; then
      runtime_dir="/run/user/0"
    else
      runtime_dir="/tmp"
    fi
  fi

  # Clear terminal to black before starting player
  sudo dd if=/dev/zero of=/dev/fb0 2>/dev/null || true
  sudo sh -c 'tput civis > /dev/tty1 2>/dev/null || true'
  sudo sh -c 'clear > /dev/tty1 2>/dev/null || true'

  echo "playback: handoff start -> $clip_path" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null

  playback_rotation="${LUMAPI_PLAYBACK_ROTATION:-${LUMAPI_KMS_ROTATION:-270}}"

  if command -v mpv >/dev/null 2>&1; then
    echo "playback: handoff launch mpv -> $clip_path (rotate: $playback_rotation)" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
    XDG_RUNTIME_DIR="$runtime_dir" mpv --fs --vo=drm --ao=alsa --hwdec=no --osc=no --cursor-autohide=no --input-touch-emulate-mouse=yes --input-vo-keyboard=yes --input-default-bindings=yes --input-conf=/home/pi/lumapi-cam/mpv-touch-input.conf --script=/home/pi/lumapi-cam/touch-helper.lua --video-rotate="${playback_rotation}" "$clip_path" 2>&1 | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
  elif command -v vlc >/dev/null 2>&1; then
    echo "playback: handoff launch vlc -> $clip_path (rotate: $playback_rotation)" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
    XDG_RUNTIME_DIR="$runtime_dir" vlc --fullscreen --play-and-exit --vout=drm_vout --video-filter=transform --transform-type="${playback_rotation}" "$clip_path" 2>&1 | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
  elif command -v ffplay >/dev/null 2>&1; then
    echo "playback: handoff launch ffplay -> $clip_path (rotate: $playback_rotation)" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
    # ffplay transpose codes: 1=90Clockwise, 2=90CounterClockwise
    transpose_arg="transpose=2"
    if [ "${playback_rotation}" = "90" ]; then
      transpose_arg="transpose=1"
    elif [ "${playback_rotation}" = "180" ]; then
      transpose_arg="transpose=1,transpose=1"
    elif [ "${playback_rotation}" = "0" ] || [ "${playback_rotation}" = "360" ]; then
      transpose_arg=""
    fi
    if [ -n "$transpose_arg" ]; then
      ffplay_vf_args="-vf $transpose_arg"
    else
      ffplay_vf_args=""
    fi
    XDG_RUNTIME_DIR="$runtime_dir" ffplay -loglevel error -autoexit -fs $ffplay_vf_args "$clip_path" 2>&1 | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
  else
    echo "playback: no installed player for handoff" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null
  fi

  echo "playback: handoff complete -> relaunch app" | sudo tee -a "$PLAYBACK_LOG_PATH" >/dev/null

  # Black out console again before restarting Slint
  sudo dd if=/dev/zero of=/dev/fb0 2>/dev/null || true
  sudo sh -c 'clear > /dev/tty1 2>/dev/null || true'
}

while true; do
  cleanup_runtime

  # Black out console transition before launching Slint
  sudo dd if=/dev/zero of=/dev/fb0 2>/dev/null || true
  sudo sh -c 'clear > /dev/tty1 2>/dev/null || true'

  # Run Slint app, redirecting stdout/stderr to log file to keep the console clean
  sudo env \
    SLINT_BACKEND=linuxkms \
    SLINT_KMS_ROTATION="${LUMAPI_KMS_ROTATION:-270}" \
    LUMAPI_FORCE_COMPACT_UI="${LUMAPI_FORCE_COMPACT_UI:-}" \
    LUMAPI_AUDIO_DEVICE="${LUMAPI_AUDIO_DEVICE:-}" \
    LUMAPI_PLAYBACK_ROTATION="${LUMAPI_PLAYBACK_ROTATION:-}" \
    /home/pi/lumapi-cam/target/release/lumapi-cam 2>&1 | sudo tee -a /tmp/lumapi-cam.log >/dev/null

  play_requested_clip
done
