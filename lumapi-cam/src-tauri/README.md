# Legacy Tauri Path

This directory is not the active runtime for the Raspberry Pi camera app.

The active headless application lives at the repository root and runs as a standalone Rust + Slint app:
- `Cargo.toml`
- `build.rs`
- `rust-src/main.rs`
- `ui/appwindow.slint`

Use these commands for the active app:

```bash
cargo build --release
sudo ./launch-cam.sh
```

Keep `src-tauri/` only as legacy reference material unless a task explicitly requires Tauri work.
