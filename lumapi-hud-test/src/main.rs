slint::include_modules!();

use std::io::Read;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn main() {
    let app = AppWindow::new().unwrap();
    let app_weak = app.as_weak();

    // Bind the Record button interaction logic
    app.on_toggle_record({
        let app_handle = app_weak.clone();
        move || {
            let ui = app_handle.unwrap();
            let current_recording_state = ui.get_is_recording();
            ui.set_is_recording(!current_recording_state);
        }
    });

    // Ingestion loop for capturing incoming TCP camera frames
    thread::spawn(move || {
        let mut stream = loop {
            if let Ok(s) = TcpStream::connect("127.0.0.1:5000") {
                break s;
            }
            thread::sleep(Duration::from_millis(100));
        };

        let mut frame_buffer: Vec<u8> = Vec::with_capacity(1024 * 1024);
        let mut read_buf = [0u8; 8192];

        loop {
            if let Ok(count) = stream.read(&mut read_buf) {
                if count == 0 { break; }
                frame_buffer.extend_from_slice(&read_buf[..count]);

                if let Some(pos) = frame_buffer.windows(2).position(|w| w == [0xFF, 0xD9]) {
                    let end_index = pos + 1;
                    let jpeg_data = &frame_buffer[..=end_index];

                    if let Ok(img) = image::load_from_memory(jpeg_data) {
                        let rgba = img.into_rgba8();

                        let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                            rgba.as_raw(),
                            rgba.width(),
                            rgba.height(),
                        );

                        let app_handle = app_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                let slint_image = slint::Image::from_rgba8(pixel_buffer);
                                ui.set_camera_feed(slint_image);
                            }
                        });
                    }

                    let remaining = frame_buffer[end_index + 1..].to_vec();
                    frame_buffer = remaining;
                }
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });

    app.run().unwrap();
}