# lumapi-cam

Headless digital cinema camera UI for Raspberry Pi.

This repository's active runtime path is a standalone Rust + Slint app that renders directly through Slint's `linuxkms` backend on a headless OS. It does not use X11 or Wayland.

The current app provides:
- full-screen live camera feed from `rpicam-vid` over a local TCP MJPEG stream
- Blackmagic-style monitoring overlay in Slint
- Rust-driven running timecode at 30 fps
- record button UI state toggling

The older Tauri/Vite files are still present in the repository, but they are not the active headless launch path.

## Active Files

- `Cargo.toml`: root manifest for the active headless app
- `build.rs`: compiles the Slint UI
- `rust-src/main.rs`: backend runtime, TCP ingest, timecode loop, UI state updates
- `ui/appwindow.slint`: camera UI layout and bindings
- `/home/pi/launch-cam.sh`: hardware launch script

## Build

```bash
cargo build --release
```

## Run On Hardware

Use the launcher so the camera process and KMS permissions are set up correctly:

```bash
sudo ./launch-cam.sh
```

The launcher:
- stops stale `rpicam-vid` and camera UI processes
- starts `rpicam-vid` as a TCP MJPEG source on `127.0.0.1:5000`
- launches the `lumapi-cam` binary with `SLINT_BACKEND=linuxkms`

## Current Status

- live viewfinder feed is wired
- timecode is driven from Rust as a running 30 fps counter
- ISO, shutter, white balance, VU meters, and histogram are still placeholder values

## Next Wiring Targets

1. Replace placeholder exposure metadata with real camera values.
2. Add ALSA-driven audio VU meters.
3. Wire GPIO buttons for record and menu actions.
