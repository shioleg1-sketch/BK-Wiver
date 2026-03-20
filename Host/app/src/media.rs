use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use image::{
    ColorType, ImageBuffer, ImageEncoder, Rgba, RgbaImage, codecs::jpeg::JpegEncoder,
    codecs::png::PngEncoder, imageops::FilterType,
};
use screenshots::Screen;
use tungstenite::{Message, connect};
use url::Url;

const TEST_FRAME_WIDTH: u32 = 960;
const TEST_FRAME_HEIGHT: u32 = 540;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamProfile {
    Fast,
    Balanced,
    Sharp,
}

impl StreamProfile {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "fast" => Self::Fast,
            "sharp" => Self::Sharp,
            _ => Self::Balanced,
        }
    }

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Sharp => "sharp",
        }
    }

    fn max_dimensions(self) -> (u32, u32) {
        match self {
            Self::Fast => (960, 540),
            Self::Balanced => (1280, 720),
            Self::Sharp => (1600, 900),
        }
    }

    fn jpeg_quality(self) -> u8 {
        match self {
            Self::Fast => 65,
            Self::Balanced => 80,
            Self::Sharp => 90,
        }
    }

    fn active_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(55),
            Self::Balanced => Duration::from_millis(75),
            Self::Sharp => Duration::from_millis(95),
        }
    }

    fn idle_frame_delay(self) -> Duration {
        match self {
            Self::Fast => Duration::from_millis(140),
            Self::Balanced => Duration::from_millis(180),
            Self::Sharp => Duration::from_millis(230),
        }
    }
}

pub fn spawn_stream(
    server_url: String,
    token: String,
    session_id: String,
    stop_flag: Arc<AtomicBool>,
    profile: Arc<Mutex<StreamProfile>>,
) {
    thread::spawn(move || {
        let Ok(url) = media_url(&server_url, &token, &session_id) else {
            return;
        };

        let mut frame_index = 0_u32;
        let mut active_screen = select_capture_screen();
        let mut previous_signature: Option<Vec<u8>> = None;

        while !stop_flag.load(Ordering::Relaxed) {
            match connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    while !stop_flag.load(Ordering::Relaxed) {
                        let stream_profile =
                            profile.lock().map(|guard| *guard).unwrap_or(StreamProfile::Balanced);

                        let frame = match active_screen.as_ref() {
                            Some(screen) => match capture_screen_frame(screen, stream_profile) {
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

                        let signature = frame_signature(&frame);
                        let is_active = previous_signature
                            .as_ref()
                            .map(|previous| signature_distance(previous, &signature) > 18)
                            .unwrap_or(true);
                        previous_signature = Some(signature);

                        if socket.send(Message::Binary(frame.into())).is_err() {
                            break;
                        }

                        thread::sleep(if is_active {
                            stream_profile.active_frame_delay()
                        } else {
                            stream_profile.idle_frame_delay()
                        });
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

fn capture_screen_frame(screen: &Screen, profile: StreamProfile) -> Result<Vec<u8>, String> {
    let captured = screen.capture().map_err(|error| error.to_string())?;
    let width = captured.width();
    let height = captured.height();
    let raw = captured.into_raw();

    let image = ImageBuffer::from_raw(width, height, raw)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;

    let image = fit_frame(image, profile.max_dimensions());
    encode_jpeg(&image, profile.jpeg_quality())
}

fn fit_frame(image: RgbaImage, max_dimensions: (u32, u32)) -> RgbaImage {
    let width = image.width();
    let height = image.height();
    if width <= max_dimensions.0 && height <= max_dimensions.1 {
        return image;
    }

    let scale = (max_dimensions.0 as f32 / width as f32)
        .min(max_dimensions.1 as f32 / height as f32);
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

fn encode_jpeg(image: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut cursor = Cursor::new(&mut bytes);
    let encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
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

fn frame_signature(bytes: &[u8]) -> Vec<u8> {
    let desired = 96usize;
    let step = (bytes.len() / desired.max(1)).max(1);
    bytes.iter().step_by(step).take(desired).copied().collect()
}

fn signature_distance(previous: &[u8], next: &[u8]) -> u32 {
    previous
        .iter()
        .zip(next.iter())
        .map(|(left, right)| left.abs_diff(*right) as u32)
        .sum::<u32>()
        / previous.len().max(1) as u32
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
