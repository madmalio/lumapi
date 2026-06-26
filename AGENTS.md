Project Context

Active app: `lumapi-cam`

Reference sandbox: `lumapi-hud-test`

Primary target right now: compact/small-screen camera UX on the Waveshare 2.8 inch DSI panel (480x640 native, used in rotated landscape for camera UI).

Current Goals

- Keep compact/small-screen work as the only priority.
- Keep recording reliable and responsive.
- Keep MP4 and MKV finalization with real audio streams.
- Improve compact UI readability and touch ergonomics for the 2.8 inch screen.
- Improve clip playback flow after capture.

Current Architecture

- UI/runtime path: standalone Rust + Slint app in `lumapi-cam`
- Rendering backend: Slint `linuxkms` on headless Raspberry Pi
- Live preview: Picamera2 sidecar serves JPEG preview over `127.0.0.1:5000`
- Camera control/status: JSON control socket on `127.0.0.1:5001`
- Rust app: `lumapi-cam/rust-src/main.rs`
- Sidecar: `lumapi-cam/camera_service.py`
- Launcher: `launch-cam.sh`

Current State Summary

- Compact mode is the primary design path.
- Recording start/stop is wired and stable in compact mode.
- MP4 and MKV post-stop finalize path has been hardened; validation now checks for real audio packets/duration (not stream presence only).
- Audio capture warm path is active and muxing is post-stop.
- Compact media browser and playback handoff flow are integrated and resume back into Media Browser after playback.
- Playback handoff transitions are completely blacked out (suppressed log printouts and full framebuffer clearing) with no console leak.
- Playback rotation is customizable via `LUMAPI_PLAYBACK_ROTATION` (independent of Slint's KMS rotation).
- UI automatically resumes in the Media Browser and highlights the active clip after playing.
- Durations are cached locally in `.metadata-cache.json` for instantaneous loading times.
- Spinning loading icon (`↻`) and enlarged Close/Refresh buttons (48px targets) are active.
- Enlarged Delete Confirmation modal (440x250 with 170x56 buttons) is present to prevent accidental deletion.
- Direct on-screen viewfinder overlay aids use icon buttons (`grid.png`, `peak.png`, `zoom.png`) as enlarged `56px` circular touch targets on the right side of the compact preview.
- Focus peaking edge detection is computed on the background frame ingestion thread in Rust, and 2x digital zoom uses Slint clipping coordinates.
- Left and right compact settings drawers are matched at `348px` height.
- Compact exposure traffic-light overlay is implemented and toggleable via `traffic.png`; it shifts right when settings is open.
- Compact vertical audio meter is reintroduced on the left and shifts right when the traffic-light overlay is enabled.
- Playback controls use a minimal `touch-helper.lua` overlay with `mp.get_osd_size()` for coordinate-agnostic tap detection; left-half = Play/Pause, right-half = Exit. No rotation-specific coordinate transforms needed.
- Touch controls in `touch_bridge.py` are now mapped directly from the raw portrait digitizer correctly into the visual landscape orientation without improper inverted axes.
- System Settings page is implemented as a full-page overlay (like Media Browser) with Sony-style vertical tab strip on the left and content on the right. Five tabs: Display (brightness + screen timeout), Audio (gain + live level VU bars with clip flashes & peak holds), Storage (usage bar with segmented capacity grid + delete all), Network (interface/IP/gateway status + Wi-Fi scanning with credential entry via custom OSK), System (CPU temp/uptime + side-by-side Reboot/Shutdown confirmation triggers + dynamic device tree model resolution).
- System settings hardware queries (using `df`, `ip`, `nmcli`, and `arecord`) are executed asynchronously on a background worker thread (`thread::spawn`) and posted back to Slint's event loop to prevent UI thread blocking and touch registration lag.
- Tab strip buttons in the System Settings vertical drawer use high-fidelity custom vector drawings (Display screen, Audio speaker, Storage floppy disk, Network signal bars, and System microchip layout) that change to the red menu accent color `#ff5c5c` when active.
- Wi-Fi Scan Result list box is enlarged to `220px` height and touch-drag scrollable with swipe prevention to ignore virtual keyboard activation while dragging/swiping.
- Connected network row in the Wi-Fi scan list is highlighted with a green background tint, green border, checkmark suffix, and a dedicated green "CONNECTED" badge.
- Screen timeout monitors evdev touch events and blanks the display via `/sys/class/graphics/fb0/blank` after configurable idle time (Off / 1 min / 5 min / 10 min). Any touch unblanks the screen. Blanking is suppressed during recording.
- All main UI overlays (top bar, bottom bar, exposure traffic, audio meter, settings drawers, view aids, SAVING indicator) check `!root.system_open` to prevent bleed-through onto the System Settings page.
- Bracing and conditional tab checks in Slint are nested independently to prevent layout overlap.
- Supported 1080p recording via the new Camera settings tab (index 4), which passes `--record-width` and `--record-height` to the Python camera sidecar so the live preview remains unchanged at 720p while recording captures at 1080p.
- HUD settings drawer has been decluttered; the redundant video container format selector was removed.
- Tab strip icons have been updated, but the Camera Resolution tab icon (Video camera) still exhibits slight edge clipping distortion when highlighted. This needs further refinement in Slint path definitions.

Important User Preferences

- Focus compact/small-screen UX first.
- Big-screen work can wait until compact path is complete.
- Camera-monitor-inspired layout is preferred.
- Record control should remain centered and obvious.
- Avoid heavy shadow box overlays where possible; prefer cleaner, flatter UI.
- Keep controls touch-friendly on 2.8 inch screen.

Known Good Commands

PC local build:

```powershell
cargo build --release
```

Typical SCP commands from PC to Pi (`192.168.8.145`):

```powershell
scp "C:\Users\Mark\Dev\lumapi-v2\lumapi-cam\rust-src\main.rs" pi@192.168.8.145:/home/pi/lumapi-cam/rust-src/main.rs
scp "C:\Users\Mark\Dev\lumapi-v2\lumapi-cam\ui\appwindow.slint" pi@192.168.8.145:/home/pi/lumapi-cam/ui/appwindow.slint
scp "C:\Users\Mark\Dev\lumapi-v2\lumapi-cam\camera_service.py" pi@192.168.8.145:/home/pi/lumapi-cam/camera_service.py
scp "C:\Users\Mark\Dev\lumapi-v2\lumapi-cam\touch-helper.lua" pi@192.168.8.145:/home/pi/lumapi-cam/touch-helper.lua
scp -r "C:\Users\Mark\Dev\lumapi-v2\lumapi-cam\ui\assets" pi@192.168.8.145:/home/pi/lumapi-cam/ui/
scp "C:\Users\Mark\Dev\lumapi-v2\launch-cam.sh" pi@192.168.8.145:/home/pi/launch-cam.sh
```

Pi build:

```bash
source "$HOME/.cargo/env"
cd ~/lumapi-cam
cargo build --release
```

Pi run (compact + known-good rotation + audio device):

```bash
cd ~
LUMAPI_KMS_ROTATION=270 LUMAPI_AUDIO_DEVICE='hw:CARD=Device,DEV=0' LUMAPI_FORCE_COMPACT_UI=1 ./launch-cam.sh
```

Important Environment Variables

- `LUMAPI_FORCE_COMPACT_UI=1`: force compact mode for testing
- `LUMAPI_AUDIO_DEVICE='hw:CARD=Device,DEV=0'`: explicit USB mic
- `LUMAPI_AUDIO_DEBUG=1`: optional audio meter debug logging
- `LUMAPI_KMS_ROTATION=270`: current known-good Slint KMS rotation for this panel setup
- `LUMAPI_PLAYBACK_ROTATION=270`: override rotation for video playback if it differs from KMS (mpv/vlc/ffplay)
- `LUMAPI_MEDIA_PLAYER_BIN=mpv`: optional explicit playback player selection

Display and Touch Notes (Waveshare 2.8 DSI)

- Display and app orientation are currently stabilized by using app-side KMS rotation (`LUMAPI_KMS_ROTATION=270`) with the current panel config.
- Touch mapping for `touch_bridge.py` matches the hardware's native output directly. No inverse portrait rotation is required.
- If orientation/touch are changed at boot level, re-verify app rotation and touch mapping together.

Important Files

- `launch-cam.sh`
  Main launcher in `/home/pi`. Runs app in a loop and handles playback handoff/relaunch flow.

- `lumapi-cam/ui/appwindow.slint`
  Main UI. Contains compact and non-compact layouts. Compact path is the active design surface.

- `lumapi-cam/ui/assets`
  UI icon/font assets (`img/settings.png`, `img/monitor.png`, `img/grid.png`, `img/peak.png`, `img/zoom.png`, `img/traffic.png`, `font/ENGCAPS.TTF`, Font Awesome files).

- `lumapi-cam/rust-src/main.rs`
  Main Rust app. Handles UI bootstrap, preview ingest, camera state sync, recording state, media browser, playback request handoff, and ALSA audio meter plumbing.

- `lumapi-cam/camera_service.py`
  Picamera2 sidecar. Owns camera preview/control/recording/finalization behavior.

- `lumapi-cam/mpv-touch-input.conf`
  mpv input override file used by launcher playback path.

- `lumapi-cam/touch-helper.lua`
  mpv touch overlay/helper script used during playback to draw OS level visual controls.

- `lumapi-cam/build_bgra.py`
  Local utility script for converting playback UI PNG icons into raw pre-multiplied BGRA assets.

Recording-With-Audio Status

- Main blocker from earlier sessions is now resolved for current tests.
- MP4 and MKV both finalize with actual audio packets in successful runs.
- Acceptance remains: `ffprobe` must show usable video + audio stream data.

Verification Commands

Camera/record logs:

```bash
tail -n 160 /tmp/lumapi-camera-service.log
ls -lt /home/pi/lumapi-cam/recordings | head
ffprobe /home/pi/lumapi-cam/recordings/<newest-file>.mp4
ffprobe /home/pi/lumapi-cam/recordings/<newest-file>.mkv
```

Playback handoff logs:

```bash
tail -n 200 /tmp/lumapi-media-playback.log
```

Important Constraint

- Do not optimize/redesign big-screen mode unless it directly helps compact small-screen path.

Next Recommended Work

1. Fix the Camera Resolution tab icon distortion in the tab strip when highlighted.
2. Validate the asynchronous system status querying thread behavior on the Pi to confirm that no UI frame rates drop or touch events are lost.
3. Verify Wi-Fi list scrolling, credential typing on the OSK, and dynamic connected badge coloring on the physical Waveshare panel.
4. Verify that the side-by-side Reboot/Shutdown confirmation overlays execute correctly on the background thread on the physical target.
5. Validate the screen timeout blanking/unblanking behaviour on-device under dynamic settings inputs.
