// Legacy Tauri entrypoint retained for reference only.
// The active Raspberry Pi camera runtime is the root Rust + Slint app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|_app| {
            // Your tauri.conf.json handles the transparency on boot automatically.
            // We can leave this setup block clean for future camera commands!
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
