slint::include_modules!();

use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::net::TcpStream as StdTcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
use alsa::pcm::{Access, Format, HwParams, PCM};
#[cfg(target_os = "linux")]
use alsa::{Direction, ValueOr};

const CAMERA_STREAM_ADDR: &str = "127.0.0.1:5000";
const CAMERA_CONTROL_ADDR: &str = "127.0.0.1:5001";
const CAMERA_SERVICE_LOG_PATH: &str = "/tmp/lumapi-camera-service.log";
const CAMERA_SETTINGS_PATH_LINUX: &str = "/home/pi/lumapi-cam/camera-settings.json";
const DEFAULT_CAMERA_WIDTH: u32 = 1140;
const DEFAULT_CAMERA_HEIGHT: u32 = 720;
const DEFAULT_TINT_DISPLAY: &str = "0";
#[cfg(target_os = "linux")]
const DEFAULT_AUDIO_DEVICE: &str = "default";
#[cfg(target_os = "linux")]
const AUDIO_SAMPLE_RATE: u32 = 48_000;
#[cfg(target_os = "linux")]
const AUDIO_BUFFER_FRAMES: usize = 1024;
#[cfg(target_os = "linux")]
const AUDIO_DEBUG_PRINT_INTERVAL: u32 = 20;
#[cfg(target_os = "linux")]
const AUDIO_METER_GAIN: f32 = 1.8;
#[cfg(target_os = "linux")]
const AUDIO_METER_DB_FLOOR: f32 = -55.0;
#[cfg(target_os = "linux")]
const AUDIO_NOISE_GATE: f32 = 0.015;
#[cfg(target_os = "linux")]
const AUDIO_PEAK_HOLD_MS: u64 = 900;
#[cfg(target_os = "linux")]
const AUDIO_PEAK_FALL_RATE: f32 = 0.045;
#[cfg(target_os = "linux")]
const AUDIO_CLIP_THRESHOLD: f32 = 0.98;
#[cfg(target_os = "linux")]
const AUDIO_CLIP_HOLD_MS: u64 = 1200;
const FPS_OPTIONS: [&str; 5] = ["24", "25", "30", "50", "60"];
const SHUTTER_ANGLE_OPTIONS: [&str; 6] = ["45°", "90°", "144°", "180°", "270°", "360°"];
const ISO_OPTIONS: [&str; 6] = ["100", "200", "400", "800", "1600", "3200"];
const WB_OPTIONS: [&str; 5] = ["Auto", "3200K", "4300K", "5600K", "6500K"];
const RECORD_FORMAT_OPTIONS: [&str; 2] = ["MP4", "MKV"];
const RECORD_BUSY_PULSE_INTERVAL_MS: u64 = 70;
const RECORD_BUSY_PULSE_STEP: f32 = 0.08;
const RECORD_STOP_SAVING_MIN_MS: u64 = 900;
const MEDIA_LIST_MAX_ITEMS: usize = 12;
const RECORDINGS_DIR_LINUX: &str = "/home/pi/lumapi-cam/recordings";
const MEDIA_PLAYBACK_LOG_PATH: &str = "/tmp/lumapi-media-playback.log";
#[cfg(target_os = "linux")]
const MEDIA_PLAYBACK_REQUEST_PATH: &str = "/tmp/lumapi-playback-request";

#[derive(Clone, Copy, Serialize, Deserialize)]
enum SettingKind {
    Fps = 0,
    Shutter = 1,
    Iso = 2,
    Wb = 3,
    RecordFormat = 4,
}

impl SettingKind {
    fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Shutter,
            2 => Self::Iso,
            3 => Self::Wb,
            4 => Self::RecordFormat,
            _ => Self::Fps,
        }
    }
}

#[derive(Default)]
struct RecordingState {
    started_at: Option<Instant>,
}

#[derive(Clone, Default)]
struct MediaClipEntry {
    path: String,
    name: String,
    detail: String,
    row: String,
    duration_badge: String,
    thumbnail_path: String,
}

#[derive(Default)]
struct MediaBrowserState {
    clips: Vec<MediaClipEntry>,
    selected_index: Option<usize>,
}

#[derive(Clone, Copy, Default)]
struct AudioLevels {
    left: f32,
    right: f32,
}

#[derive(Clone, Copy, Default)]
struct AudioMeterUiState {
    current: AudioLevels,
    peak: AudioLevels,
    clip_left: bool,
    clip_right: bool,
}

#[cfg(target_os = "linux")]
struct AudioMeterState {
    current: AudioLevels,
    peak: AudioLevels,
    peak_left_hold_until: Option<Instant>,
    peak_right_hold_until: Option<Instant>,
    clip_left_until: Option<Instant>,
    clip_right_until: Option<Instant>,
}

#[cfg(target_os = "linux")]
impl Default for AudioMeterState {
    fn default() -> Self {
        Self {
            current: AudioLevels::default(),
            peak: AudioLevels::default(),
            peak_left_hold_until: None,
            peak_right_hold_until: None,
            clip_left_until: None,
            clip_right_until: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct CameraSettingsState {
    selected_setting: SettingKind,
    fps_index: usize,
    shutter_index: usize,
    iso_index: usize,
    wb_index: usize,
    record_format_index: usize,
    full_auto: bool,
}

impl CameraSettingsState {
    fn fps(&self) -> u32 {
        FPS_OPTIONS[self.fps_index].parse().unwrap_or(30)
    }

    fn shutter_angle_degrees(&self) -> u32 {
        SHUTTER_ANGLE_OPTIONS[self.shutter_index]
            .trim_end_matches('°')
            .parse()
            .unwrap_or(180)
    }

    fn shutter_microseconds(&self) -> u32 {
        let frame_duration_us = 1_000_000u32 / self.fps().max(1);
        let exposure = frame_duration_us.saturating_mul(self.shutter_angle_degrees()) / 360;
        exposure.clamp(100, frame_duration_us.saturating_sub(100).max(100))
    }

    fn analog_gain(&self) -> f32 {
        match ISO_OPTIONS[self.iso_index].parse::<f32>() {
            Ok(iso) => (iso / 100.0).max(1.0),
            Err(_) => 1.0,
        }
    }

    fn awb_mode(&self) -> &'static str {
        match self.wb_index {
            0 => "auto",
            1 => "tungsten",
            2 => "fluorescent",
            3 => "daylight",
            4 => "cloudy",
            _ => "auto",
        }
    }

    fn record_format(&self) -> &'static str {
        match self.record_format_index {
            1 => "mkv",
            _ => "mp4",
        }
    }
}

impl Default for CameraSettingsState {
    fn default() -> Self {
        Self {
            selected_setting: SettingKind::Fps,
            fps_index: 2,
            shutter_index: 3,
            iso_index: 0,
            wb_index: 0,
            record_format_index: 0,
            full_auto: false,
        }
    }
}

enum CameraControlMessage {
    Apply(CameraSettingsState),
}

#[derive(Serialize)]
struct CameraControlRequest<'a> {
    command: &'a str,
    fps: u32,
    shutter_us: u32,
    analogue_gain: f32,
    awb_mode: &'a str,
    full_auto: bool,
    recording_format: &'a str,
}

#[derive(Deserialize)]
struct CameraStatusResponse {
    ok: bool,
    fps: Option<f32>,
    frame_duration_us: Option<u32>,
    exposure_time_us: Option<u32>,
    analogue_gain: Option<f32>,
    iso: Option<u32>,
    awb_mode: Option<String>,
    full_auto: Option<bool>,
    recording_format: Option<String>,
    recording_path: Option<String>,
    is_recording: Option<bool>,
    error: Option<String>,
}

fn main() {
    let app = AppWindow::new().unwrap();
    let app_weak = app.as_weak();
    let recording_state = Arc::new(Mutex::new(RecordingState::default()));
    let media_state = Arc::new(Mutex::new(MediaBrowserState::default()));
    let record_toggle_in_flight = Arc::new(AtomicBool::new(false));
    let focus_peaking_active = Arc::new(AtomicBool::new(false));
    let camera_settings_state = Arc::new(Mutex::new(load_camera_settings()));
    let (camera_control_tx, camera_control_rx) = mpsc::channel();

    app.set_force_compact_mode(compact_ui_preview_enabled());
    apply_default_camera_settings(&app, &camera_settings_state);

    let resume_clip_name = if cfg!(target_os = "linux") {
        let resume_path = Path::new("/tmp/lumapi-playback-resume");
        if resume_path.exists() {
            let name = fs::read_to_string(resume_path).ok().map(|s| s.trim().to_string());
            let _ = fs::remove_file(resume_path);
            name
        } else {
            None
        }
    } else {
        None
    };

    if let Some(ref name) = resume_clip_name {
        app.set_media_open(true);
        app.set_media_loading(true);
        refresh_media_browser(app_weak.clone(), Arc::clone(&media_state), Some(name.clone()));
    }

    let initial_camera_settings = camera_settings_state
        .lock()
        .ok()
        .map(|state| state.clone())
        .unwrap_or_default();

    start_camera_control_loop(camera_control_rx, initial_camera_settings.clone());
    let _ = camera_control_tx.send(CameraControlMessage::Apply(initial_camera_settings));

    start_camera_metadata_loop(
        app_weak.clone(),
        Arc::clone(&camera_settings_state),
        Arc::clone(&recording_state),
        Arc::clone(&record_toggle_in_flight),
    );
    start_record_busy_pulse_loop(app_weak.clone());
    start_audio_meter_loop(app_weak.clone());

    app.on_toggle_record({
        let app_handle = app_weak.clone();
        let recording_state = Arc::clone(&recording_state);
        let camera_settings_state = Arc::clone(&camera_settings_state);
        let record_toggle_in_flight = Arc::clone(&record_toggle_in_flight);
        move || {
            if record_toggle_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }

            let Some(ui) = app_handle.upgrade() else {
                record_toggle_in_flight.store(false, Ordering::SeqCst);
                return;
            };

            let previous_is_recording = ui.get_is_recording();
            let next_is_recording = !previous_is_recording;
            let is_stop_transition = previous_is_recording;
            let transition_started_at = Instant::now();
            let fps = camera_settings_state
                .lock()
                .ok()
                .map(|state| state.fps())
                .unwrap_or(30);

            ui.set_record_busy(true);
            if next_is_recording {
                set_recording_ui_state(&ui, &recording_state, true, fps);
            } else {
                let record_format = ui.get_current_record_format().to_string();
                ui.set_is_recording(false);
                ui.set_current_media_status("SAVING".into());
                ui.set_current_media_detail(format!("Finalizing {record_format}").into());
            }

            let app_handle = app_handle.clone();
            let recording_state = Arc::clone(&recording_state);
            let record_toggle_in_flight = Arc::clone(&record_toggle_in_flight);

            thread::spawn(move || {
                let actual_is_recording = toggle_camera_recording()
                    .and_then(|status| status.is_recording)
                    .unwrap_or(previous_is_recording);

                if is_stop_transition && !actual_is_recording {
                    let elapsed = transition_started_at.elapsed();
                    let min_duration = Duration::from_millis(RECORD_STOP_SAVING_MIN_MS);
                    if elapsed < min_duration {
                        thread::sleep(min_duration - elapsed);
                    }
                }

                record_toggle_in_flight.store(false, Ordering::SeqCst);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        ui.set_record_busy(false);
                        set_recording_ui_state(&ui, &recording_state, actual_is_recording, fps);
                    }
                });
            });
        }
    });

    app.on_select_setting({
        let app_handle = app_weak.clone();
        let camera_settings_state = Arc::clone(&camera_settings_state);
        move |selected_setting| {
            if let Ok(mut state) = camera_settings_state.lock() {
                state.selected_setting = SettingKind::from_index(selected_setting);
                save_camera_settings(&state);
                if let Some(ui) = app_handle.upgrade() {
                    sync_camera_settings_to_ui(&ui, &state);
                }
            }
        }
    });

    app.on_toggle_settings({
        let app_handle = app_weak.clone();
        let camera_settings_state = Arc::clone(&camera_settings_state);
        let camera_control_tx = camera_control_tx.clone();
        move || {
            if let Some(ui) = app_handle.upgrade() {
                let will_open = !ui.get_settings_open();
                ui.set_settings_open(will_open);

                if !will_open {
                    if let Ok(state) = camera_settings_state.lock() {
                        let _ = camera_control_tx.send(CameraControlMessage::Apply(state.clone()));
                    }
                }
            }
        }
    });

    app.on_toggle_media({
        let app_handle = app_weak.clone();
        let media_state = Arc::clone(&media_state);
        move || {
            if let Some(ui) = app_handle.upgrade() {
                let will_open = !ui.get_media_open();
                ui.set_media_open(will_open);

                if will_open {
                    ui.set_settings_open(false);
                    ui.set_media_loading(true);
                    refresh_media_browser(app_handle.clone(), Arc::clone(&media_state), None);
                }
            }
        }
    });

    app.on_media_refresh({
        let app_handle = app_weak.clone();
        let media_state = Arc::clone(&media_state);
        move || {
            if let Some(ui) = app_handle.upgrade() {
                ui.set_media_loading(true);
            }
            refresh_media_browser(app_handle.clone(), Arc::clone(&media_state), None);
        }
    });

    app.on_media_select({
        let app_handle = app_weak.clone();
        let media_state = Arc::clone(&media_state);
        move |index| {
            if let Some(ui) = app_handle.upgrade() {
                if let Ok(mut state) = media_state.lock() {
                    let selected = usize::try_from(index).ok().filter(|value| *value < state.clips.len());
                    state.selected_index = selected;
                    apply_media_selection_to_ui(&ui, &state);
                }
            }
        }
    });

    app.on_media_play({
        let app_handle = app_weak.clone();
        let media_state = Arc::clone(&media_state);
        move |index| {
            let clip = media_state
                .lock()
                .ok()
                .and_then(|state| usize::try_from(index).ok().and_then(|value| state.clips.get(value).cloned()));

            if let Some(clip) = clip {
                match launch_media_playback(&clip.path) {
                    Ok(player) => {
                        if player == "handoff" {
                            std::process::exit(0);
                        }

                        if let Some(ui) = app_handle.upgrade() {
                            ui.set_media_selected_detail(format!("Playing via {player}: {}", clip.name).into());
                        }
                    }
                    Err(error) => {
                        eprintln!("media playback unavailable: {error}");
                        if let Some(ui) = app_handle.upgrade() {
                            ui.set_media_selected_detail(
                                format!("Playback failed. Install ffplay/mpv/vlc. {}", clip.name).into(),
                            );
                        }
                    }
                }
            }
        }
    });

    app.on_media_delete_selected({
        let app_handle = app_weak.clone();
        let media_state = Arc::clone(&media_state);
        move || {
            let selected_path = media_state
                .lock()
                .ok()
                .and_then(|state| state.selected_index.and_then(|index| state.clips.get(index).map(|clip| clip.path.clone())));

            if let Some(path) = selected_path {
                let _ = fs::remove_file(path);
                if let Some(ui) = app_handle.upgrade() {
                    ui.set_media_loading(true);
                }
                refresh_media_browser(app_handle.clone(), Arc::clone(&media_state), None);
            }
        }
    });

    app.on_toggle_full_auto({
        let app_handle = app_weak.clone();
        let camera_settings_state = Arc::clone(&camera_settings_state);
        let camera_control_tx = camera_control_tx.clone();
        move || {
            if let Ok(mut state) = camera_settings_state.lock() {
                state.full_auto = !state.full_auto;

                save_camera_settings(&state);

                if let Some(ui) = app_handle.upgrade() {
                    sync_camera_settings_to_ui(&ui, &state);
                }

                let _ = camera_control_tx.send(CameraControlMessage::Apply(state.clone()));
            }
        }
    });

    app.on_adjust_setting({
        let app_handle = app_weak.clone();
        let camera_settings_state = Arc::clone(&camera_settings_state);
        let camera_control_tx = camera_control_tx.clone();
        move |delta| {
            if let Ok(mut state) = camera_settings_state.lock() {
                adjust_selected_setting(&mut state, delta);
                save_camera_settings(&state);

                if let Some(ui) = app_handle.upgrade() {
                    sync_camera_settings_to_ui(&ui, &state);
                }

                let _ = camera_control_tx.send(CameraControlMessage::Apply(state.clone()));
            }
        }
    });

    app.on_toggle_focus_peaking({
        let app_handle = app_weak.clone();
        let focus_peaking_active = Arc::clone(&focus_peaking_active);
        move || {
            if let Some(ui) = app_handle.upgrade() {
                focus_peaking_active.store(ui.get_focus_peaking_visible(), Ordering::SeqCst);
            }
        }
    });

    start_timecode_loop(app_weak.clone(), recording_state, camera_settings_state);
    start_camera_ingest_loop(app_weak, Arc::clone(&focus_peaking_active));

    app.run().unwrap();
}

fn compact_ui_preview_enabled() -> bool {
    matches!(
        env::var("LUMAPI_FORCE_COMPACT_UI").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn apply_default_camera_settings(app: &AppWindow, camera_settings_state: &Arc<Mutex<CameraSettingsState>>) {
    if let Ok(state) = camera_settings_state.lock() {
        sync_camera_settings_to_ui(app, &state);
    }

    apply_resolution_to_ui(app, DEFAULT_CAMERA_WIDTH, DEFAULT_CAMERA_HEIGHT);
    app.set_current_tint(DEFAULT_TINT_DISPLAY.into());
    app.set_record_busy(false);
    app.set_record_busy_pulse(0.0);
    app.set_media_open(false);
    app.set_media_loading(false);
    app.set_media_clip_rows(slint::ModelRc::new(slint::VecModel::from(vec![])));
    app.set_media_clip_durations(slint::ModelRc::new(slint::VecModel::from(vec![])));
    app.set_media_clip_thumbs(slint::ModelRc::new(slint::VecModel::from(vec![])));
    app.set_media_selected_index(-1);
    app.set_media_selected_name("No clip selected".into());
    app.set_media_selected_detail("Open Media to browse recordings".into());
    app.set_current_media_status("READY".into());
    app.set_current_media_detail("MP4 ready".into());
    app.set_exposure_signal(0);
    apply_audio_levels_to_ui(app, AudioMeterUiState::default());
    let fps = camera_settings_state.lock().ok().map(|state| state.fps()).unwrap_or(30);
    app.set_timecode(format_timecode(0, fps).into());
}

fn sync_camera_settings_to_ui(app: &AppWindow, state: &CameraSettingsState) {
    app.set_full_auto(state.full_auto);
    app.set_current_mode_label(if state.full_auto { "AUTO".into() } else { "M".into() });
    app.set_selected_setting(state.selected_setting as i32);
    app.set_current_fps(FPS_OPTIONS[state.fps_index].into());
    app.set_current_shutter(SHUTTER_ANGLE_OPTIONS[state.shutter_index].into());
    app.set_current_iso(ISO_OPTIONS[state.iso_index].into());
    app.set_current_wb(WB_OPTIONS[state.wb_index].into());
    app.set_current_record_format(RECORD_FORMAT_OPTIONS[state.record_format_index].into());

    let (label, previous, current, next) = selected_setting_display(state);
    app.set_selected_setting_label(label.into());
    app.set_selected_setting_prev_value(previous.into());
    app.set_selected_setting_value(current.into());
    app.set_selected_setting_next_value(next.into());
}

fn adjust_selected_setting(state: &mut CameraSettingsState, delta: i32) {
    match state.selected_setting {
        SettingKind::Fps => cycle_index(&mut state.fps_index, FPS_OPTIONS.len(), delta),
        SettingKind::Shutter => cycle_index(&mut state.shutter_index, SHUTTER_ANGLE_OPTIONS.len(), delta),
        SettingKind::Iso => cycle_index(&mut state.iso_index, ISO_OPTIONS.len(), delta),
        SettingKind::Wb => cycle_index(&mut state.wb_index, WB_OPTIONS.len(), delta),
        SettingKind::RecordFormat => cycle_index(&mut state.record_format_index, RECORD_FORMAT_OPTIONS.len(), delta),
    }
}

fn cycle_index(index: &mut usize, len: usize, delta: i32) {
    let current = *index as i32;
    let wrapped = (current + delta).rem_euclid(len as i32);
    *index = wrapped as usize;
}

fn selected_setting_display(state: &CameraSettingsState) -> (&'static str, &'static str, &'static str, &'static str) {
    match state.selected_setting {
        SettingKind::Fps => indexed_display("FPS", FPS_OPTIONS, state.fps_index),
        SettingKind::Shutter => indexed_display("ANGLE", SHUTTER_ANGLE_OPTIONS, state.shutter_index),
        SettingKind::Iso => indexed_display("ISO", ISO_OPTIONS, state.iso_index),
        SettingKind::Wb => indexed_display("WB", WB_OPTIONS, state.wb_index),
        SettingKind::RecordFormat => indexed_display("REC", RECORD_FORMAT_OPTIONS, state.record_format_index),
    }
}

fn indexed_display<const N: usize>(
    label: &'static str,
    options: [&'static str; N],
    index: usize,
) -> (&'static str, &'static str, &'static str, &'static str) {
    let previous = if index == 0 { N - 1 } else { index - 1 };
    let next = (index + 1) % N;
    (label, options[previous], options[index], options[next])
}

fn load_camera_settings() -> CameraSettingsState {
    let settings_path = camera_settings_path();

    fs::read_to_string(settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<CameraSettingsState>(&content).ok())
        .unwrap_or_default()
}

fn save_camera_settings(settings: &CameraSettingsState) {
    let settings_path = camera_settings_path();

    if let Ok(content) = serde_json::to_string_pretty(settings) {
        if let Err(error) = fs::write(settings_path, content) {
            eprintln!("failed to save camera settings: {error}");
        }
    }
}

fn camera_settings_path() -> &'static str {
    if cfg!(target_os = "linux") {
        CAMERA_SETTINGS_PATH_LINUX
    } else {
        "camera-settings.json"
    }
}

fn start_camera_metadata_loop(
    app_weak: slint::Weak<AppWindow>,
    camera_settings_state: Arc<Mutex<CameraSettingsState>>,
    recording_state: Arc<Mutex<RecordingState>>,
    record_toggle_in_flight: Arc<AtomicBool>,
) {
    thread::spawn(move || loop {
        if let Some(status) = query_camera_status() {
            let settings = camera_settings_state.lock().ok().map(|state| state.clone()).unwrap_or_default();
            let app_handle = app_weak.clone();
            let recording_state = Arc::clone(&recording_state);
            let record_toggle_in_flight = Arc::clone(&record_toggle_in_flight);

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    let toggle_in_flight = record_toggle_in_flight.load(Ordering::SeqCst);
                    apply_camera_status_to_ui(&ui, &settings, &status, toggle_in_flight);

                    if !toggle_in_flight {
                        if let Some(is_recording) = status.is_recording {
                            set_recording_ui_state(&ui, &recording_state, is_recording, settings.fps());
                        }
                    }
                }
            });
        }

        thread::sleep(Duration::from_millis(500));
    });
}

fn apply_camera_status_to_ui(
    app: &AppWindow,
    settings: &CameraSettingsState,
    status: &CameraStatusResponse,
    toggle_in_flight: bool,
) {
    let full_auto = status.full_auto.unwrap_or(settings.full_auto);
    app.set_full_auto(full_auto);
    app.set_current_mode_label(if full_auto { "AUTO".into() } else { "M".into() });

    let fps = status
        .fps
        .map(|value| format_fps_display(value))
        .unwrap_or_else(|| FPS_OPTIONS[settings.fps_index].to_string());
    app.set_current_fps(fps.into());

    let shutter_angle = if full_auto {
        match (status.exposure_time_us, status.frame_duration_us) {
            (Some(exposure_time), Some(frame_duration)) if frame_duration > 0 => {
                let angle = ((exposure_time as f32 / frame_duration as f32) * 360.0).round() as u32;
                format!("{angle}°")
            }
            _ => SHUTTER_ANGLE_OPTIONS[settings.shutter_index].to_string(),
        }
    } else {
        SHUTTER_ANGLE_OPTIONS[settings.shutter_index].to_string()
    };
    app.set_current_shutter(shutter_angle.into());

    let iso = if full_auto {
        status
            .iso
            .map(|value| value.to_string())
            .or_else(|| status.analogue_gain.map(|gain| ((gain * 100.0).round() as u32).to_string()))
            .unwrap_or_else(|| ISO_OPTIONS[settings.iso_index].to_string())
    } else {
        ISO_OPTIONS[settings.iso_index].to_string()
    };
    app.set_current_iso(iso.into());

    let wb = if full_auto {
        "Auto".to_string()
    } else {
        match status.awb_mode.as_deref() {
            Some("auto") => "Auto".to_string(),
            _ => WB_OPTIONS[settings.wb_index].to_string(),
        }
    };
    app.set_current_wb(wb.into());

    if let Some(recording_format) = status.recording_format.as_deref() {
        app.set_current_record_format(recording_format.to_ascii_uppercase().into());
    }

    let recording_format = app.get_current_record_format().to_string();

    app.set_record_busy(toggle_in_flight);

    if toggle_in_flight && !status.is_recording.unwrap_or(false) {
        app.set_current_media_status("SAVING".into());
        app.set_current_media_detail(format!("Finalizing {recording_format}").into());
        return;
    }

    let media_detail = if status.is_recording.unwrap_or(false) {
        format!("Recording {recording_format}")
    } else if let Some(path) = status.recording_path.as_deref() {
        clip_name_from_path(path)
    } else {
        format!("{recording_format} ready")
    };

    app.set_current_media_status(if status.is_recording.unwrap_or(false) { "REC".into() } else { "READY".into() });
    app.set_current_media_detail(media_detail.into());
}

fn set_recording_ui_state(
    app: &AppWindow,
    recording_state: &Arc<Mutex<RecordingState>>,
    is_recording: bool,
    fps: u32,
) {
    app.set_is_recording(is_recording);

    if let Ok(mut state) = recording_state.lock() {
        if is_recording {
            state.started_at.get_or_insert_with(Instant::now);
        } else {
            state.started_at = None;
        }
    }

    if !is_recording {
        app.set_timecode(format_timecode(0, fps).into());
    }

    app.set_current_media_status(if is_recording { "REC".into() } else { "READY".into() });
    if is_recording {
        let record_format = app.get_current_record_format().to_string();
        app.set_current_media_detail(format!("Recording {record_format}").into());
    } else {
        let record_format = app.get_current_record_format().to_string();
        app.set_current_media_detail(format!("{record_format} ready").into());
    }
}

fn start_record_busy_pulse_loop(app_weak: slint::Weak<AppWindow>) {
    thread::spawn(move || loop {
        let app_handle = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_handle.upgrade() {
                if ui.get_record_busy() && !ui.get_is_recording() {
                    let current = ui.get_record_busy_pulse();
                    let next = if current >= 1.0 { 0.0 } else { (current + RECORD_BUSY_PULSE_STEP).min(1.0) };
                    ui.set_record_busy_pulse(next);
                } else {
                    ui.set_record_busy_pulse(0.0);
                }

                if ui.get_media_loading() {
                    let current = ui.get_media_loading_pulse();
                    let next = if current >= 360.0 { 0.0 } else { current + 15.0 };
                    ui.set_media_loading_pulse(next);
                } else {
                    ui.set_media_loading_pulse(0.0);
                }
            }
        });

        thread::sleep(Duration::from_millis(RECORD_BUSY_PULSE_INTERVAL_MS));
    });
}

fn clip_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn recordings_dir() -> &'static str {
    if cfg!(target_os = "linux") {
        RECORDINGS_DIR_LINUX
    } else {
        "recordings"
    }
}

fn refresh_media_browser(
    app_weak: slint::Weak<AppWindow>,
    media_state: Arc<Mutex<MediaBrowserState>>,
    select_name: Option<String>,
) {
    thread::spawn(move || {
        let clips = load_media_clips();

        let app_handle = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_handle.upgrade() {
                if let Ok(mut state) = media_state.lock() {
                    state.clips = clips;
                    
                    let mut found_index = None;
                    if let Some(ref name) = select_name {
                        found_index = state.clips.iter().position(|c| &c.name == name);
                    }
                    state.selected_index = found_index.or_else(|| {
                        if state.clips.is_empty() { None } else { Some(0) }
                    });

                    let rows: Vec<slint::SharedString> = state
                        .clips
                        .iter()
                        .map(|clip| slint::SharedString::from(clip.row.as_str()))
                        .collect();
                    let durations: Vec<slint::SharedString> = state
                        .clips
                        .iter()
                        .map(|clip| slint::SharedString::from(clip.duration_badge.as_str()))
                        .collect();
                    let thumbs: Vec<slint::Image> = state
                        .clips
                        .iter()
                        .map(|clip| {
                            if clip.thumbnail_path.is_empty() {
                                slint::Image::default()
                            } else {
                                slint::Image::load_from_path(Path::new(&clip.thumbnail_path)).unwrap_or_default()
                            }
                        })
                        .collect();
                    ui.set_media_clip_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
                    ui.set_media_clip_durations(slint::ModelRc::new(slint::VecModel::from(durations)));
                    ui.set_media_clip_thumbs(slint::ModelRc::new(slint::VecModel::from(thumbs)));
                    apply_media_selection_to_ui(&ui, &state);
                }

                ui.set_media_loading(false);
            }
        });
    });
}

fn apply_media_selection_to_ui(app: &AppWindow, state: &MediaBrowserState) {
    if let Some(index) = state.selected_index.and_then(|value| state.clips.get(value).map(|_| value)) {
        if let Some(clip) = state.clips.get(index) {
            app.set_media_selected_index(index as i32);
            app.set_media_selected_name(clip.name.clone().into());
            app.set_media_selected_detail(clip.detail.clone().into());
            return;
        }
    }

    app.set_media_selected_index(-1);
    app.set_media_selected_name("No clip selected".into());
    app.set_media_selected_detail("Choose a clip to view details".into());
}

#[derive(Serialize, Deserialize, Clone)]
struct CachedMetadata {
    size: u64,
    modified_sec: u64,
    duration: String,
}

fn load_metadata_cache() -> std::collections::HashMap<String, CachedMetadata> {
    let cache_path = Path::new(recordings_dir()).join(".metadata-cache.json");
    fs::read_to_string(cache_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_metadata_cache(cache: &std::collections::HashMap<String, CachedMetadata>) {
    let cache_path = Path::new(recordings_dir()).join(".metadata-cache.json");
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path, content);
    }
}

fn load_media_clips() -> Vec<MediaClipEntry> {
    let directory = recordings_dir();
    let Ok(entries) = fs::read_dir(directory) else {
        return vec![];
    };

    // 1. Gather all video files with their modified times
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let extension = path.extension().and_then(|val| val.to_str()).unwrap_or_default().to_ascii_lowercase();
        if extension != "mp4" && extension != "mkv" {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            let modified = metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            files.push((modified, path));
        }
    }

    // 2. Sort by modified time descending (newest first)
    files.sort_by(|(left_time, _), (right_time, _)| right_time.cmp(left_time));

    // Load local metadata cache
    let mut cache = load_metadata_cache();
    let mut cache_dirty = false;

    // 3. Process only the newest up to MEDIA_LIST_MAX_ITEMS
    let mut clips = vec![];
    for (_modified, path) in files.into_iter().take(MEDIA_LIST_MAX_ITEMS) {
        let Some(name) = path.file_name().and_then(|val| val.to_str()).map(|val| val.to_string()) else {
            continue;
        };
        let extension = path.extension().and_then(|val| val.to_str()).unwrap_or_default().to_ascii_lowercase();
        let metadata = match fs::metadata(&path) {
            Ok(val) => val,
            Err(_) => continue,
        };

        let size = metadata.len();
        let modified_sec = metadata.modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Check if duration cache is valid
        let mut duration_label = None;
        if let Some(cached) = cache.get(&name) {
            if cached.size == size && cached.modified_sec == modified_sec {
                duration_label = Some(cached.duration.clone());
            }
        }

        if duration_label.is_none() {
            let probed = probe_duration_label(&path).unwrap_or_else(|| "--:--".to_string());
            cache.insert(name.clone(), CachedMetadata {
                size,
                modified_sec,
                duration: probed.clone(),
            });
            duration_label = Some(probed);
            cache_dirty = true;
        }

        let duration_val = duration_label.unwrap();
        let size_label = format_file_size(size);
        let recorded_label = recording_timestamp_from_name(&name).unwrap_or_else(|| "Unknown time".to_string());
        
        let format_label = extension.to_ascii_uppercase();
        let detail = format!("{format_label}  {duration_val}  {size_label}  {recorded_label}");
        let row = name.clone();
        let thumbnail_path = ensure_thumbnail_path(&path, &name);

        clips.push(MediaClipEntry {
            path: path.to_string_lossy().to_string(),
            name,
            detail,
            row,
            duration_badge: duration_val,
            thumbnail_path,
        });
    }

    if cache_dirty {
        save_metadata_cache(&cache);
    }

    clips
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    if bytes as f64 >= GB {
        format!("{:.2} GB", bytes as f64 / GB)
    } else if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes as f64 >= KB {
        format!("{:.0} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

fn recording_timestamp_from_name(name: &str) -> Option<String> {
    let stem = name.split('.').next()?;
    let mut parts = stem.split('_');
    let _prefix = parts.next()?;
    let date = parts.next()?;
    let time = parts.next()?;

    if date.len() != 8 || time.len() != 6 {
        return None;
    }

    let year = &date[0..4];
    let month = &date[4..6];
    let day = &date[6..8];
    let hour = &time[0..2];
    let minute = &time[2..4];
    let second = &time[4..6];

    Some(format!("{year}-{month}-{day} {hour}:{minute}:{second}"))
}

fn probe_duration_label(path: &Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let seconds = raw.parse::<f32>().ok()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }

    let total_seconds = seconds.round() as u32;
    let minutes = total_seconds / 60;
    let remaining_seconds = total_seconds % 60;
    Some(format!("{minutes:02}:{remaining_seconds:02}"))
}

fn ensure_thumbnail_path(video_path: &Path, clip_name: &str) -> String {
    let cache_dir = Path::new(recordings_dir()).join(".thumbs");
    let _ = fs::create_dir_all(&cache_dir);

    let thumb_name = format!("{clip_name}.jpg");
    let thumbnail_path = cache_dir.join(thumb_name);

    if !thumbnail_path.exists() {
        let _ = Command::new("ffmpeg")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-i")
            .arg(video_path)
            .arg("-ss")
            .arg("00:00:00.2")
            .arg("-vframes")
            .arg("1")
            .arg("-vf")
            .arg("scale=240:-2")
            .arg(&thumbnail_path)
            .output();
    }

    if thumbnail_path.exists() {
        thumbnail_path.to_string_lossy().to_string()
    } else {
        String::new()
    }
}

fn launch_media_playback(path: &str) -> std::io::Result<&'static str> {
    #[cfg(target_os = "linux")]
    {
        return launch_media_playback_handoff(path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let runtime_dir = playback_runtime_dir();

        if let Ok(custom_player) = env::var("LUMAPI_MEDIA_PLAYER_BIN") {
            let trimmed = custom_player.trim();
            if !trimmed.is_empty() {
                let mut child = Command::new(trimmed)
                    .arg(path)
                    .stdin(Stdio::null())
                    .stdout(playback_log_stdio()?)
                    .stderr(playback_log_stdio()?)
                    .env("XDG_RUNTIME_DIR", &runtime_dir)
                    .spawn()?;
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok("custom");
            }
        }

        let candidates: [(&str, &[&str]); 3] = [
            ("ffplay", &["-loglevel", "error", "-autoexit", "-fs"]),
            ("mpv", &["--fs", "--really-quiet"]),
            ("vlc", &["--fullscreen", "--play-and-exit"]),
        ];

        for (program, args) in candidates {
            let mut command = Command::new(program);
            command.args(args);
            command
                .arg(path)
                .stdin(Stdio::null())
                .stdout(playback_log_stdio()?)
                .stderr(playback_log_stdio()?)
                .env("XDG_RUNTIME_DIR", &runtime_dir);

            match command.spawn() {
                Ok(mut child) => {
                    thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(program);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }

        Err(std::io::Error::other("no media player found (tried ffplay, mpv, vlc)"))
    }
}

#[cfg(target_os = "linux")]
fn launch_media_playback_handoff(path: &str) -> std::io::Result<&'static str> {
    fs::write(MEDIA_PLAYBACK_REQUEST_PATH, format!("{path}\n"))?;
    
    // Write name of selected clip for UI resume
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let _ = fs::write("/tmp/lumapi-playback-resume", file_name);

    append_playback_log(&format!("playback: scheduling handoff -> {path}"));
    Ok("handoff")
}

#[cfg(not(target_os = "linux"))]
fn playback_log_stdio() -> std::io::Result<Stdio> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(MEDIA_PLAYBACK_LOG_PATH)
        .map(Stdio::from)
}

#[cfg(target_os = "linux")]
fn append_playback_log(line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(MEDIA_PLAYBACK_LOG_PATH) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(not(target_os = "linux"))]
fn playback_runtime_dir() -> String {
    if let Ok(existing) = env::var("XDG_RUNTIME_DIR") {
        if !existing.trim().is_empty() {
            return existing;
        }
    }

    for candidate in ["/run/user/1000", "/run/user/0", "/tmp"] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }

    "/tmp".to_string()
}

fn apply_audio_levels_to_ui(app: &AppWindow, state: AudioMeterUiState) {
    app.set_audio_level_left(state.current.left);
    app.set_audio_level_right(state.current.right);
    app.set_audio_peak_left(state.peak.left);
    app.set_audio_peak_right(state.peak.right);
    app.set_audio_clip_left(state.clip_left);
    app.set_audio_clip_right(state.clip_right);
}

fn apply_resolution_to_ui(app: &AppWindow, width: u32, height: u32) {
    app.set_current_resolution(resolution_label(width, height).into());
    app.set_current_aspect_ratio(aspect_ratio_label(width, height).into());
}

fn resolution_label(width: u32, height: u32) -> String {
    match height {
        2160 => "4K".to_string(),
        1440 => "1440p".to_string(),
        1080 => "1080p".to_string(),
        720 => "720p".to_string(),
        _ => format!("{width}x{height}"),
    }
}

fn aspect_ratio_label(width: u32, height: u32) -> String {
    let divisor = gcd(width.max(1), height.max(1));
    format!("{}:{}", width / divisor, height / divisor)
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }

    a.max(1)
}

fn start_audio_meter_loop(app_weak: slint::Weak<AppWindow>) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app_weak;
    }

    #[cfg(target_os = "linux")]
    thread::spawn(move || loop {
        if let Err(error) = run_audio_meter_loop(app_weak.clone()) {
            eprintln!("audio meter unavailable: {error}");

            let app_handle = app_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    apply_audio_levels_to_ui(&ui, AudioMeterUiState::default());
                }
            });

            thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(target_os = "linux")]
fn run_audio_meter_loop(app_weak: slint::Weak<AppWindow>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (pcm, channels) = open_audio_capture()?;
    let io = pcm.io_i16()?;
    let channel_count = usize::try_from(channels).unwrap_or(1).max(1);
    let mut buffer = vec![0i16; AUDIO_BUFFER_FRAMES * channel_count];
    let mut meter_state = AudioMeterState::default();
    let debug_audio = audio_debug_enabled();
    let mut debug_counter = 0u32;

    loop {
        match io.readi(&mut buffer) {
            Ok(frames) if frames > 0 => {
                let sample_count = frames * channel_count;
                let measured = measure_audio_levels(&buffer[..sample_count], channel_count);
                let ui_state = update_audio_meter_state(&mut meter_state, measured.scaled, Instant::now());

                if debug_audio {
                    debug_counter += 1;
                    if debug_counter % AUDIO_DEBUG_PRINT_INTERVAL == 0 {
                        eprintln!(
                            "audio debug: channels={channel_count} raw_left={:.4} raw_right={:.4} meter_left={:.4} meter_right={:.4}",
                            measured.raw_left,
                            measured.raw_right,
                            ui_state.current.left,
                            ui_state.current.right,
                        );
                    }
                }

                let app_handle = app_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        apply_audio_levels_to_ui(&ui, ui_state);
                    }
                });
            }
            Ok(_) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = pcm.prepare();
                return Err(Box::new(error));
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn open_audio_capture() -> Result<(PCM, u32), Box<dyn std::error::Error + Send + Sync>> {
    let device_candidates = audio_device_candidates();
    let mut errors = Vec::new();

    for device_name in &device_candidates {
        match configure_audio_capture(device_name, 2) {
            Ok(pcm) => {
                eprintln!("audio meter using ALSA device: {device_name} (stereo)");
                return Ok((pcm, 2));
            }
            Err(stereo_error) => match configure_audio_capture(device_name, 1) {
                Ok(pcm) => {
                    eprintln!("audio meter using ALSA device: {device_name} (mono)");
                    return Ok((pcm, 1));
                }
                Err(mono_error) => errors.push(format!(
                    "{device_name}: stereo={stereo_error}; mono={mono_error}"
                )),
            },
        }
    }

    Err(format!(
        "no usable ALSA capture device found; tried {}",
        errors.join(" | ")
    )
    .into())
}

#[cfg(target_os = "linux")]
fn audio_device_candidates() -> Vec<String> {
    let mut devices = Vec::new();

    if let Ok(device) = env::var("LUMAPI_AUDIO_DEVICE") {
        let trimmed = device.trim();
        if !trimmed.is_empty() {
            for variant in prioritized_audio_device_variants(trimmed) {
                if !devices.iter().any(|existing| existing == &variant) {
                    devices.push(variant);
                }
            }
        }
    }

    for device in [
        DEFAULT_AUDIO_DEVICE,
        "default:CARD=Device",
        "sysdefault:CARD=Device",
        "front:CARD=Device,DEV=0",
        "dsnoop:CARD=Device,DEV=0",
        "plughw:CARD=Device,DEV=0",
        "hw:CARD=Device,DEV=0",
        "plughw:3,0",
        "hw:3,0",
        "plughw:1,0",
        "hw:1,0",
        "plughw:2,0",
        "hw:2,0",
        "plughw:0,0",
        "hw:0,0",
    ] {
        if !devices.iter().any(|existing| existing == device) {
            devices.push(device.to_string());
        }
    }

    devices
}

#[cfg(target_os = "linux")]
fn prioritized_audio_device_variants(device: &str) -> Vec<String> {
    let mut variants = Vec::new();

    if let Some(suffix) = device.strip_prefix("hw:") {
        variants.push(format!("dsnoop:{suffix}"));
        variants.push(format!("plughw:{suffix}"));
    } else if let Some(suffix) = device.strip_prefix("plughw:") {
        variants.push(format!("dsnoop:{suffix}"));
    }

    variants.push(device.to_string());
    variants
}

#[cfg(target_os = "linux")]
fn configure_audio_capture(device_name: &str, channels: u32) -> Result<PCM, Box<dyn std::error::Error + Send + Sync>> {
    let pcm = PCM::new(device_name, Direction::Capture, false)?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_rate(AUDIO_SAMPLE_RATE, ValueOr::Nearest)?;
        hwp.set_channels(channels)?;
        hwp.set_buffer_size(4096)?;
        pcm.hw_params(&hwp)?;
    }
    pcm.start()?;
    Ok(pcm)
}

#[cfg(target_os = "linux")]
fn update_audio_meter_state(state: &mut AudioMeterState, measured: AudioLevels, now: Instant) -> AudioMeterUiState {
    state.current.left = smooth_audio_level(state.current.left, measured.left);
    state.current.right = smooth_audio_level(state.current.right, measured.right);

    update_peak_channel(
        &mut state.peak.left,
        state.current.left,
        &mut state.peak_left_hold_until,
        now,
    );
    update_peak_channel(
        &mut state.peak.right,
        state.current.right,
        &mut state.peak_right_hold_until,
        now,
    );

    update_clip_channel(state.current.left, &mut state.clip_left_until, now);
    update_clip_channel(state.current.right, &mut state.clip_right_until, now);

    AudioMeterUiState {
        current: state.current,
        peak: state.peak,
        clip_left: state.clip_left_until.is_some_and(|until| now < until),
        clip_right: state.clip_right_until.is_some_and(|until| now < until),
    }
}

#[cfg(target_os = "linux")]
fn update_peak_channel(peak: &mut f32, current: f32, hold_until: &mut Option<Instant>, now: Instant) {
    if current >= *peak {
        *peak = current;
        *hold_until = Some(now + Duration::from_millis(AUDIO_PEAK_HOLD_MS));
        return;
    }

    if hold_until.is_some_and(|until| now < until) {
        return;
    }

    *peak = (*peak - AUDIO_PEAK_FALL_RATE).max(current).clamp(0.0, 1.0);
}

#[cfg(target_os = "linux")]
fn update_clip_channel(current: f32, clip_until: &mut Option<Instant>, now: Instant) {
    if current >= AUDIO_CLIP_THRESHOLD {
        *clip_until = Some(now + Duration::from_millis(AUDIO_CLIP_HOLD_MS));
    } else if clip_until.is_some_and(|until| now >= until) {
        *clip_until = None;
    }
}

#[cfg(target_os = "linux")]
fn measure_audio_levels(samples: &[i16], channels: usize) -> MeasuredAudioLevels {
    if samples.is_empty() {
        return MeasuredAudioLevels::default();
    }

    let mut left_peak = 0.0f32;
    let mut right_peak = 0.0f32;
    let mut left_sum = 0.0f32;
    let mut right_sum = 0.0f32;
    let mut left_count = 0usize;
    let mut right_count = 0usize;

    if channels >= 2 && samples.len() >= 2 {
        for frame in samples.chunks_exact(2) {
            let left = normalize_audio_sample(frame[0]);
            let right = normalize_audio_sample(frame[1]);
            left_peak = left_peak.max(left);
            right_peak = right_peak.max(right);
            left_sum += left * left;
            right_sum += right * right;
            left_count += 1;
            right_count += 1;
        }

        if left_peak > 0.0 && right_peak < 0.001 {
            right_peak = left_peak;
            right_sum = left_sum;
            right_count = left_count;
        } else if right_peak > 0.0 && left_peak < 0.001 {
            left_peak = right_peak;
            left_sum = right_sum;
            left_count = right_count;
        }
    } else {
        for sample in samples {
            let value = normalize_audio_sample(*sample);
            left_peak = left_peak.max(value);
            left_sum += value * value;
            left_count += 1;
        }
        right_peak = left_peak;
        right_sum = left_sum;
        right_count = left_count;
    }

    let left_rms = if left_count > 0 {
        (left_sum / left_count as f32).sqrt()
    } else {
        0.0
    };
    let right_rms = if right_count > 0 {
        (right_sum / right_count as f32).sqrt()
    } else {
        0.0
    };

    let left_level = meter_input_level(left_peak, left_rms);
    let right_level = meter_input_level(right_peak, right_rms);

    MeasuredAudioLevels {
        raw_left: left_level.clamp(0.0, 1.0),
        raw_right: right_level.clamp(0.0, 1.0),
        scaled: AudioLevels {
            left: meter_scale(left_level),
            right: meter_scale(right_level),
        },
    }
}

#[cfg(target_os = "linux")]
fn normalize_audio_sample(sample: i16) -> f32 {
    f32::from(sample.abs()) / f32::from(i16::MAX)
}

#[cfg(target_os = "linux")]
fn smooth_audio_level(previous: f32, measured: f32) -> f32 {
    let attack = 0.78;
    let release = 0.10;
    let factor = if measured > previous { attack } else { release };
    (previous + (measured - previous) * factor).clamp(0.0, 1.0)
}

#[cfg(target_os = "linux")]
fn meter_scale(linear: f32) -> f32 {
    if linear <= 0.0005 {
        return 0.0;
    }

    let db = 20.0 * linear.log10();
    ((db - AUDIO_METER_DB_FLOOR) / -AUDIO_METER_DB_FLOOR).clamp(0.0, 1.0)
}

#[cfg(target_os = "linux")]
fn meter_input_level(peak: f32, rms: f32) -> f32 {
    let blended = (rms * 1.9 + peak * 0.45) * AUDIO_METER_GAIN;
    let gated = (blended - AUDIO_NOISE_GATE).max(0.0);
    gated.clamp(0.0, 1.0)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Default)]
struct MeasuredAudioLevels {
    raw_left: f32,
    raw_right: f32,
    scaled: AudioLevels,
}

#[cfg(target_os = "linux")]
fn audio_debug_enabled() -> bool {
    matches!(
        env::var("LUMAPI_AUDIO_DEBUG").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn format_fps_display(value: f32) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.05 {
        format!("{}", rounded as u32)
    } else {
        format!("{value:.1}")
    }
}

fn start_timecode_loop(
    app_weak: slint::Weak<AppWindow>,
    recording_state: Arc<Mutex<RecordingState>>,
    camera_settings_state: Arc<Mutex<CameraSettingsState>>,
) {
    thread::spawn(move || {
        loop {
            let fps = camera_settings_state.lock().ok().map(|state| state.fps()).unwrap_or(30);
            let total_frames = recording_state
                .lock()
                .ok()
                .and_then(|state| state.started_at.map(|started_at| elapsed_frames(started_at, fps)))
                .unwrap_or(0);
            let app_handle = app_weak.clone();

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    ui.set_timecode(format_timecode(total_frames, fps).into());
                }
            });

            thread::sleep(Duration::from_millis(1_000 / u64::from(fps.max(1))));
        }
    });
}

fn elapsed_frames(started_at: Instant, fps: u32) -> u64 {
    started_at.elapsed().as_millis() as u64 * u64::from(fps.max(1)) / 1_000
}

fn format_timecode(total_frames: u64, fps: u32) -> String {
    let fps = u64::from(fps.max(1));
    let total_seconds = total_frames / fps;
    let frames = total_frames % fps;
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = (total_seconds / 3_600) % 24;

    format!("{hours:02}:{minutes:02}:{seconds:02}:{frames:02}")
}

fn start_camera_ingest_loop(app_weak: slint::Weak<AppWindow>, focus_peaking_active: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut frame_buffer = Vec::with_capacity(1024 * 1024);
        let mut read_buf = [0u8; 8192];

        loop {
            let mut stream = loop {
                if let Ok(stream) = TcpStream::connect(CAMERA_STREAM_ADDR) {
                    break stream;
                }
                thread::sleep(Duration::from_millis(100));
            };

            frame_buffer.clear();

            loop {
                match stream.read(&mut read_buf) {
                    Ok(0) => break,
                    Ok(count) => {
                        frame_buffer.extend_from_slice(&read_buf[..count]);

                        while let Some(pos) = frame_buffer.windows(2).position(|w| w == [0xFF, 0xD9]) {
                            let end_index = pos + 1;
                            let jpeg_data = &frame_buffer[..=end_index];

                            if let Ok(img) = image::load_from_memory(jpeg_data) {
                                let mut rgba = img.into_rgba8();
                                let exposure_signal = estimate_exposure_signal(&rgba);
                                if focus_peaking_active.load(Ordering::SeqCst) {
                                    apply_focus_peaking(&mut rgba);
                                }
                                let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                    rgba.as_raw(),
                                    rgba.width(),
                                    rgba.height(),
                                );

                                let app_handle = app_weak.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_handle.upgrade() {
                                        ui.set_camera_feed(slint::Image::from_rgba8(pixel_buffer));
                                        ui.set_exposure_signal(exposure_signal);
                                    }
                                });
                            }

                            frame_buffer.drain(..=end_index);
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(10));
                        break;
                    }
                }
            }
        }
    });
}

fn estimate_exposure_signal(rgba: &image::RgbaImage) -> i32 {
    let raw = rgba.as_raw();
    if raw.len() < 4 {
        return 0;
    }

    let pixel_count = raw.len() / 4;
    let stride = if pixel_count > 200_000 {
        10
    } else if pixel_count > 120_000 {
        8
    } else {
        6
    };

    let mut sampled = 0usize;
    let mut low_count = 0usize;
    let mut high_count = 0usize;

    for pixel_index in (0..pixel_count).step_by(stride) {
        let idx = pixel_index * 4;
        let r = raw[idx] as u32;
        let g = raw[idx + 1] as u32;
        let b = raw[idx + 2] as u32;
        let luminance = ((r + (g << 1) + b) >> 2) as u8;

        if luminance <= 28 {
            low_count += 1;
        } else if luminance >= 235 {
            high_count += 1;
        }
        sampled += 1;
    }

    if sampled == 0 {
        return 0;
    }

    let low_ratio = low_count as f32 / sampled as f32;
    let high_ratio = high_count as f32 / sampled as f32;

    let over_level = if high_ratio >= 0.08 {
        2
    } else if high_ratio >= 0.03 {
        1
    } else {
        0
    };

    let under_level = if low_ratio >= 0.62 {
        2
    } else if low_ratio >= 0.45 {
        1
    } else {
        0
    };

    match (under_level, over_level) {
        (0, 0) => 0,
        (u, 0) => -u,
        (0, o) => o,
        (u, o) => {
            let under_stress = low_ratio / if u >= 2 { 0.62 } else { 0.45 };
            let over_stress = high_ratio / if o >= 2 { 0.08 } else { 0.03 };
            if over_stress >= under_stress {
                o
            } else {
                -u
            }
        }
    }
}

fn apply_focus_peaking(rgba: &mut image::RgbaImage) {
    let width = rgba.width();
    let height = rgba.height();
    if width < 3 || height < 3 {
        return;
    }

    thread_local! {
        static LUM_BUFFER: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(Vec::new());
    }

    LUM_BUFFER.with(|buf| {
        let mut lum = buf.borrow_mut();
        let total_pixels = (width * height) as usize;
        if lum.len() != total_pixels {
            lum.resize(total_pixels, 0);
        }

        let raw = rgba.as_mut();

        // 1. Calculate luminance map
        for y in 0..height {
            let row_offset = (y * width) as usize;
            for x in 0..width {
                let idx = (row_offset + x as usize) * 4;
                let r = raw[idx] as u32;
                let g = raw[idx + 1] as u32;
                let b = raw[idx + 2] as u32;
                // standard fast approximation of luminance: (R + 2G + B) / 4
                lum[row_offset + x as usize] = ((r + (g << 1) + b) >> 2) as u8;
            }
        }

        // 2. Detect edges and overlay green pixels
        let threshold = 18;
        for y in 1..(height - 1) {
            let row_offset = (y * width) as usize;
            let next_row_offset = row_offset + width as usize;
            for x in 1..(width - 1) {
                let idx = row_offset + x as usize;
                let current = lum[idx] as i32;
                let right = lum[idx + 1] as i32;
                let down = lum[next_row_offset + x as usize] as i32;

                if (current - right).abs() > threshold || (current - down).abs() > threshold {
                    let raw_idx = idx * 4;
                    raw[raw_idx] = 0;
                    raw[raw_idx + 1] = 255;
                    raw[raw_idx + 2] = 0;
                }
            }
        }
    });
}

fn start_camera_control_loop(
    camera_control_rx: mpsc::Receiver<CameraControlMessage>,
    initial_settings: CameraSettingsState,
) {
    thread::spawn(move || {
        if !cfg!(target_os = "linux") {
            return;
        }

        let mut active_process = match spawn_camera_service_process(&initial_settings) {
            Ok(child) => Some(child),
            Err(error) => {
                eprintln!("failed to start camera service: {error}");
                None
            }
        };

        if active_process.is_some() && wait_for_camera_service(Duration::from_secs(5)).is_err() {
            eprintln!("camera service did not become ready; check {CAMERA_SERVICE_LOG_PATH}");
        }

        while let Ok(CameraControlMessage::Apply(settings)) = camera_control_rx.recv() {
            if let Err(error) = send_camera_controls(&settings) {
                eprintln!("failed to apply live camera controls: {error}");

                if let Some(mut process) = active_process.take() {
                    let _ = process.kill();
                    let _ = process.wait();
                }

                match spawn_camera_service_process(&settings) {
                    Ok(child) => {
                        active_process = Some(child);
                        let _ = wait_for_camera_service(Duration::from_secs(5));
                        let _ = send_camera_controls(&settings);
                    }
                    Err(spawn_error) => {
                        eprintln!("failed to restart camera service: {spawn_error}");
                    }
                }
            }
        }
    });
}

fn spawn_camera_service_process(initial_settings: &CameraSettingsState) -> std::io::Result<Child> {
    let service_path = "/home/pi/lumapi-cam/camera_service.py";
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(CAMERA_SERVICE_LOG_PATH)?;
    let log_file_err = log_file.try_clone()?;

    Command::new("python3")
        .arg("-u")
        .arg(service_path)
        .arg("--width")
        .arg(DEFAULT_CAMERA_WIDTH.to_string())
        .arg("--height")
        .arg(DEFAULT_CAMERA_HEIGHT.to_string())
        .arg("--fps")
        .arg(initial_settings.fps().to_string())
        .arg("--shutter-us")
        .arg(initial_settings.shutter_microseconds().to_string())
        .arg("--analogue-gain")
        .arg(format!("{:.2}", initial_settings.analog_gain()))
        .arg("--awb-mode")
        .arg(initial_settings.awb_mode())
        .arg("--recording-format")
        .arg(initial_settings.record_format())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err))
        .spawn()
}

fn send_camera_controls(settings: &CameraSettingsState) -> std::io::Result<()> {
    let request = CameraControlRequest {
        command: "apply",
        fps: settings.fps(),
        shutter_us: settings.shutter_microseconds(),
        analogue_gain: settings.analog_gain(),
        awb_mode: settings.awb_mode(),
        full_auto: settings.full_auto,
        recording_format: settings.record_format(),
    };

    let response = send_camera_request(&request)?;
    if response.ok {
        Ok(())
    } else {
        Err(std::io::Error::other(
            response.error.unwrap_or_else(|| "camera service rejected apply request".to_string()),
        ))
    }
}

fn wait_for_camera_service(timeout: Duration) -> std::io::Result<()> {
    let started_at = Instant::now();

    while started_at.elapsed() < timeout {
        if StdTcpStream::connect(CAMERA_CONTROL_ADDR).is_ok() {
            return Ok(());
        }

        thread::sleep(Duration::from_millis(100));
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "camera control service did not start listening in time",
    ))
}

fn query_camera_status() -> Option<CameraStatusResponse> {
    let request = CameraControlRequest {
        command: "status",
        fps: 0,
        shutter_us: 0,
        analogue_gain: 0.0,
        awb_mode: "auto",
        full_auto: false,
        recording_format: "mp4",
    };

    match send_camera_request(&request) {
        Ok(response) if response.ok => Some(response),
        _ => None,
    }
}

fn toggle_camera_recording() -> Option<CameraStatusResponse> {
    let request = CameraControlRequest {
        command: "toggle_record",
        fps: 0,
        shutter_us: 0,
        analogue_gain: 0.0,
        awb_mode: "auto",
        full_auto: false,
        recording_format: "mp4",
    };

    match send_camera_request(&request) {
        Ok(response) if response.ok => Some(response),
        _ => None,
    }
}

fn send_camera_request(request: &CameraControlRequest<'_>) -> std::io::Result<CameraStatusResponse> {
    let payload = serde_json::to_vec(request)
        .map_err(|error| std::io::Error::other(format!("failed to encode camera request: {error}")))?;

    let mut stream = StdTcpStream::connect(CAMERA_CONTROL_ADDR)?;
    stream.write_all(&payload)?;
    stream.flush()?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    serde_json::from_str::<CameraStatusResponse>(&response)
        .map_err(|error| std::io::Error::other(format!("failed to decode camera response: {error}")))
}
