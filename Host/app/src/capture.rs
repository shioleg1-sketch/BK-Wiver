use image::{ImageBuffer, Rgba, RgbaImage, imageops::FilterType};
use screenshots::Screen;

pub struct CaptureFrame {
    pub image: RgbaImage,
    pub backend: &'static str,
    pub used_fallback: bool,
}

pub struct CaptureEngine {
    active_screen: Option<Screen>,
}

impl CaptureEngine {
    pub fn new() -> Self {
        Self {
            active_screen: select_capture_screen(),
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        match self.active_screen.as_ref() {
            Some(screen) => match capture_screen_image(screen, max_dimensions) {
                Ok(image) => CaptureFrame {
                    image,
                    backend: backend_name(),
                    used_fallback: false,
                },
                Err(_) => {
                    self.active_screen = select_capture_screen();
                    CaptureFrame {
                        image: build_test_frame(frame_index),
                        backend: "test-fallback",
                        used_fallback: true,
                    }
                }
            },
            None => {
                self.active_screen = select_capture_screen();
                CaptureFrame {
                    image: build_test_frame(frame_index),
                    backend: "test-fallback",
                    used_fallback: true,
                }
            }
        }
    }
}

fn backend_name() -> &'static str {
    #[cfg(windows)]
    {
        // Current Windows path still uses screenshots as a temporary backend.
        // This module is the entry point where DXGI Desktop Duplication can replace it next.
        "windows-screenshots"
    }

    #[cfg(not(windows))]
    {
        "screenshots"
    }
}

fn select_capture_screen() -> Option<Screen> {
    let screens = Screen::all().ok()?;
    screens
        .iter()
        .find(|screen| screen.display_info.is_primary)
        .copied()
        .or_else(|| screens.into_iter().next())
}

fn capture_screen_image(screen: &Screen, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
    let captured = screen.capture().map_err(|error| error.to_string())?;
    let width = captured.width();
    let height = captured.height();
    let raw = captured.into_raw();

    let image = ImageBuffer::from_raw(width, height, raw)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;

    Ok(fit_frame(image, max_dimensions))
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

fn build_test_frame(frame_index: u32) -> RgbaImage {
    const TEST_FRAME_WIDTH: u32 = 960;
    const TEST_FRAME_HEIGHT: u32 = 540;

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

    image
}
