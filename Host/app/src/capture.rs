mod common;
#[cfg(windows)]
mod windows;

use image::RgbaImage;

#[cfg(windows)]
use windows::WindowsCaptureEngine;

#[cfg(not(windows))]
use common::ScreenshotsCaptureBackend;

pub struct CaptureFrame {
    pub image: RgbaImage,
    pub backend: &'static str,
    pub used_fallback: bool,
    pub capture_time: Instant,
    pub frame_index: u32,
}

use std::time::Instant;

pub struct CaptureEngine {
    #[cfg(windows)]
    inner: WindowsCaptureEngine,
    #[cfg(not(windows))]
    inner: ScreenshotsCaptureBackend,
    pub frame_buffer: Vec<RgbaImage>,
    pub frame_buffer_size: usize,
    pub network_quality: u8,
}

impl CaptureEngine {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            inner: WindowsCaptureEngine::new(),
            #[cfg(not(windows))]
            inner: ScreenshotsCaptureBackend::with_backend_name("screenshots"),
            frame_buffer: Vec::new(),
            frame_buffer_size: 5,
            network_quality: 100,
        }
    }

    pub fn set_network_quality(&mut self, quality: u8) {
        self.network_quality = quality;
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        // Оптимизация: добавляем кадры в буфер
        self.frame_buffer.push(RgbaImage::from_raw(max_dimensions.0, max_dimensions.1, vec![0; (max_dimensions.0 * max_dimensions.1) as usize]));
        
        // Ограничиваем размер буфера
        if self.frame_buffer.len() > self.frame_buffer_size {
            self.frame_buffer.remove(0);
        }

        CaptureFrame {
            image: self.inner.capture(max_dimensions, frame_index),
            backend: "capture",
            used_fallback: false,
            capture_time: Instant::now(),
            frame_index,
        }
    }

    // Адаптивное разрешение
    pub fn get_optimal_resolution(&self) -> (u32, u32) {
        match self.network_quality {
            90..=100 => (1920, 1080),  // Отлично
            70..=89  => (1280, 720),   // Хорошо
            50..=69  => (854, 480),    // Удовлетворительно
            _        => (640, 480),    // Плохо
        }
    }
}
