use image::{imageops::FilterType, ImageBuffer, Rgba, RgbaImage};
use screenshots::Screen;

use super::CaptureFrame;

pub struct ScreenshotsCaptureBackend {
    active_screen: Option<Screen>,
    backend_name: &'static str,
}

impl ScreenshotsCaptureBackend {
    pub fn with_backend_name(backend_name: &'static str) -> Self {
        Self {
            active_screen: select_capture_screen(),
            backend_name,
        }
    }

    pub fn try_capture(&mut self, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
        match self.active_screen.as_ref() {
            Some(screen) => match capture_screen_image(screen, max_dimensions) {
                Ok(image) => Ok(image),
                Err(error) => {
                    self.active_screen = select_capture_screen();
                    Err(error)
                }
            },
            None => {
                self.active_screen = select_capture_screen();
                Err("no active screen available".to_owned())
            }
        }
    }

    #[cfg(windows)]
    pub fn backend_name(&self) -> &'static str {
        self.backend_name
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        match self.try_capture(max_dimensions) {
            Ok(image) => CaptureFrame {
                image,
                backend: self.backend_name,
                used_fallback: false,
            },
            Err(_) => CaptureFrame {
                image: build_test_frame(frame_index),
                backend: "test-fallback",
                used_fallback: true,
            },
        }
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

pub(crate) fn fit_frame(image: RgbaImage, max_dimensions: (u32, u32)) -> RgbaImage {
    if max_dimensions.0 == 0 || max_dimensions.1 == 0 {
        return image;
    }

    let width = image.width();
    let height = image.height();
    let scale = (max_dimensions.0 as f32 / width as f32)
        .min(max_dimensions.1 as f32 / height as f32)
        .min(1.0);
    let resized_width = ((width as f32 * scale).round() as u32).max(1);
    let resized_height = ((height as f32 * scale).round() as u32).max(1);
    let resized = if resized_width == width && resized_height == height {
        image
    } else {
        image::imageops::resize(&image, resized_width, resized_height, FilterType::Triangle)
    };

    if resized_width == max_dimensions.0 && resized_height == max_dimensions.1 {
        return resized;
    }

    let mut canvas: RgbaImage =
        ImageBuffer::from_pixel(max_dimensions.0, max_dimensions.1, Rgba([12, 14, 18, 255]));
    let offset_x = (max_dimensions.0.saturating_sub(resized_width)) / 2;
    let offset_y = (max_dimensions.1.saturating_sub(resized_height)) / 2;
    image::imageops::overlay(
        &mut canvas,
        &resized,
        i64::from(offset_x),
        i64::from(offset_y),
    );
    canvas
}

pub(crate) fn build_test_frame(frame_index: u32) -> RgbaImage {
    const TEST_FRAME_WIDTH: u32 = 960;
    const TEST_FRAME_HEIGHT: u32 = 540;

    let mut image: RgbaImage = ImageBuffer::new(TEST_FRAME_WIDTH, TEST_FRAME_HEIGHT);
    let shift = (frame_index * 9) % TEST_FRAME_WIDTH;
    let pulse = ((frame_index * 11) % 255) as u8;

    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let border = x < 6 || y < 6 || x > TEST_FRAME_WIDTH - 7 || y > TEST_FRAME_HEIGHT - 7;
        let header = y < 72;
        let footer = y > TEST_FRAME_HEIGHT.saturating_sub(72);
        let moving_band = x > shift.saturating_sub(40) && x < (shift + 40).min(TEST_FRAME_WIDTH);
        let checker = ((x / 32) + (y / 32)) % 2 == 0;

        *pixel = if border {
            Rgba([245, 248, 252, 255])
        } else if header {
            Rgba([190, 42, 52, 255])
        } else if footer {
            Rgba([34, 39, 46, 255])
        } else if moving_band {
            Rgba([255, 214, 10, 255])
        } else if checker {
            Rgba([46, 134, 171, 255])
        } else {
            Rgba([18, pulse.max(40), 92, 255])
        };
    }

    image
}
