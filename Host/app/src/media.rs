use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use image::{
    ColorType, ImageBuffer, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder,
    imageops::FilterType,
};
use screenshots::Screen;
use tungstenite::{Message, connect};
use url::Url;

const TEST_FRAME_WIDTH: u32 = 960;
const TEST_FRAME_HEIGHT: u32 = 540;
const STREAM_MAX_WIDTH: u32 = 1440;
const STREAM_MAX_HEIGHT: u32 = 900;

pub fn spawn_test_stream(
    server_url: String,
    token: String,
    session_id: String,
    stop_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            return;
        };

        let mut frame_index = 0_u32;
        let mut active_screen = select_capture_screen();
        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    while !stop_flag.load(Ordering::Relaxed) {
                        let frame = match active_screen.as_ref() {
                            Some(screen) => match capture_screen_frame(screen) {
                                Ok(frame) => frame,
                                Err(_) => {
                                    active_screen = select_capture_screen();
                                    build_test_frame(frame_index)
                                }
                            },
                            None => {
                                active_screen = select_capture_screen();
                                build_test_frame(frame_index)
                            }
                        };
                        frame_index = frame_index.wrapping_add(1);

                        if socket.send(Message::Binary(frame.into())).is_err() {
                            break;
                        }

                        thread::sleep(Duration::from_millis(250));
                    }

                    let _ = socket.close(None);
                }
                Err(_) => {
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    });
}

fn select_capture_screen() -> Option<Screen> {
    let screens = Screen::all().ok()?;
    screens
        .iter()
        .find(|screen| screen.display_info.is_primary)
        .copied()
        .or_else(|| screens.into_iter().next())
}

fn capture_screen_frame(screen: &Screen) -> Result<Vec<u8>, String> {
    let captured = screen.capture().map_err(|error| error.to_string())?;
    let width = captured.width();
    let height = captured.height();
    let raw = captured.into_raw();

    let image = ImageBuffer::from_raw(width, height, raw)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;

    let image = fit_frame(image);
    encode_png(&image)
}

fn fit_frame(image: RgbaImage) -> RgbaImage {
    let width = image.width();
    let height = image.height();
    if width <= STREAM_MAX_WIDTH && height <= STREAM_MAX_HEIGHT {
        return image;
    }

    let scale = (STREAM_MAX_WIDTH as f32 / width as f32)
        .min(STREAM_MAX_HEIGHT as f32 / height as f32);
    let resized_width = ((width as f32 * scale).round() as u32).max(1);
    let resized_height = ((height as f32 * scale).round() as u32).max(1);
    image::imageops::resize(&image, resized_width, resized_height, FilterType::Triangle)
}

fn build_test_frame(frame_index: u32) -> Vec<u8> {
    let mut image: RgbaImage = ImageBuffer::new(TEST_FRAME_WIDTH, TEST_FRAME_HEIGHT);
    let shift = (frame_index * 13) % TEST_FRAME_WIDTH;
    let pulse = ((frame_index * 17) % 255) as u8;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let r = (((x + shift) % TEST_FRAME_WIDTH) * 255 / TEST_FRAME_WIDTH) as u8;
        let g = ((y * 255) / TEST_FRAME_HEIGHT) as u8;
        let mut b = pulse;

        if x > shift.saturating_sub(28) && x < (shift + 28).min(TEST_FRAME_WIDTH) {
            b = 240;
        }

        let border = x < 6 || y < 6 || x > TEST_FRAME_WIDTH - 7 || y > TEST_FRAME_HEIGHT - 7;
        *pixel = if border {
            Rgba([220, 226, 233, 255])
        } else {
            Rgba([r, g, b, 255])
        };
    }

    encode_png(&image).unwrap_or_default()
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    let encoder = PngEncoder::new(&mut cursor);
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn media_url(server_url: &str, token: &str, session_id: &str) -> Result<Url, String> {
    let normalized = server_url.trim().trim_end_matches('/');
    let ws_base = if let Some(rest) = normalized.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = normalized.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if normalized.starts_with("ws://") || normalized.starts_with("wss://") {
        normalized.to_owned()
    } else {
        format!("ws://{normalized}")
    };

    Url::parse(&format!(
        "{ws_base}/ws/v1/media?token={token}&sessionId={session_id}"
    ))
    .map_err(|error| error.to_string())
}
