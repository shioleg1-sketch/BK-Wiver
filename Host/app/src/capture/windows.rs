use dxgi_capture_rs::{CaptureError, DXGIManager};
use image::{ImageBuffer, RgbaImage};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
        CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP,
        HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    },
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
};

use crate::logging;

use super::{
    CaptureFrame,
    common::{ScreenshotsCaptureBackend, fit_frame},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsCaptureBackendKind {
    DxgiDuplication,
    Win32Gdi,
    ScreenshotsFallback,
}

pub struct WindowsCaptureEngine {
    preferred_backend: WindowsCaptureBackendKind,
    dxgi_backend: Option<DxgiCaptureBackend>,
    gdi_backend: Option<GdiCaptureBackend>,
    screenshots_fallback: ScreenshotsCaptureBackend,
}

impl WindowsCaptureEngine {
    pub fn new() -> Self {
        let dxgi_backend = match DxgiCaptureBackend::new() {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.dxgi",
                    format!("initialization failed: {}", error),
                );
                None
            }
        };

        let gdi_backend = match GdiCaptureBackend::new() {
            Ok(backend) => Some(backend),
            Err(error) => {
                logging::append_log(
                    "WARN",
                    "capture.gdi",
                    format!("initialization failed: {}", error),
                );
                None
            }
        };

        Self {
            preferred_backend: WindowsCaptureBackendKind::DxgiDuplication,
            dxgi_backend,
            gdi_backend,
            screenshots_fallback: ScreenshotsCaptureBackend::with_backend_name(
                "windows-screenshots-fallback",
            ),
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        match self.preferred_backend {
            WindowsCaptureBackendKind::DxgiDuplication => {
                if let Some(backend) = &mut self.dxgi_backend {
                    match backend.capture(max_dimensions) {
                        Ok(image) => CaptureFrame {
                            image,
                            backend: "windows-dxgi",
                            used_fallback: false,
                        },
                        Err(error) => {
                            logging::append_log(
                                "WARN",
                                "capture.dxgi",
                                format!("frame capture failed: {}", error),
                            );

                            if should_retry_dxgi(&error) {
                                match self.capture_with_gdi(max_dimensions) {
                                    Ok(image) => CaptureFrame {
                                        image,
                                        backend: "windows-gdi-temporary",
                                        used_fallback: false,
                                    },
                                    Err(gdi_error) => {
                                        logging::append_log(
                                            "WARN",
                                            "capture.gdi",
                                            format!(
                                                "temporary fallback after dxgi failure also failed: {}",
                                                gdi_error
                                            ),
                                        );
                                        self.preferred_backend =
                                            WindowsCaptureBackendKind::ScreenshotsFallback;
                                        self.screenshots_fallback
                                            .capture(max_dimensions, frame_index)
                                    }
                                }
                            } else {
                                logging::append_log(
                                    "WARN",
                                    "capture.dxgi",
                                    "switching preferred backend to windows-gdi",
                                );
                                self.preferred_backend = WindowsCaptureBackendKind::Win32Gdi;
                                self.capture(max_dimensions, frame_index)
                            }
                        }
                    }
                } else {
                    logging::append_log(
                        "WARN",
                        "capture.dxgi",
                        "backend unavailable, switching preferred backend to windows-gdi",
                    );
                    self.preferred_backend = WindowsCaptureBackendKind::Win32Gdi;
                    self.capture(max_dimensions, frame_index)
                }
            }
            WindowsCaptureBackendKind::Win32Gdi => {
                match self.capture_with_gdi(max_dimensions) {
                    Ok(image) => CaptureFrame {
                        image,
                        backend: "windows-gdi",
                        used_fallback: false,
                    },
                    Err(error) => {
                        logging::append_log(
                            "WARN",
                            "capture.gdi",
                            format!(
                                "frame capture failed, switching preferred backend to screenshots fallback: {}",
                                error
                            ),
                        );
                        self.preferred_backend = WindowsCaptureBackendKind::ScreenshotsFallback;
                        self.screenshots_fallback.capture(max_dimensions, frame_index)
                    }
                }
            }
            WindowsCaptureBackendKind::ScreenshotsFallback => {
                self.screenshots_fallback.capture(max_dimensions, frame_index)
            }
        }
    }

    fn capture_with_gdi(&mut self, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
        if let Some(backend) = &mut self.gdi_backend {
            return backend.capture(max_dimensions);
        }

        let mut backend = GdiCaptureBackend::new()?;
        let result = backend.capture(max_dimensions);
        self.gdi_backend = Some(backend);
        result
    }
}

struct DxgiCaptureBackend {
    manager: DXGIManager,
    last_frame: Option<RgbaImage>,
}

impl DxgiCaptureBackend {
    fn new() -> Result<Self, String> {
        let mut manager = DXGIManager::new(16).map_err(|error| error.to_string())?;
        manager.set_capture_source_index(0);
        Ok(Self {
            manager,
            last_frame: None,
        })
    }

    fn capture(&mut self, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
        match self.manager.capture_frame_components() {
            Ok((mut bgra, (width, height))) => {
                for pixel in bgra.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }

                let image = ImageBuffer::from_raw(width as u32, height as u32, bgra)
                    .ok_or_else(|| "failed to build RGBA frame".to_owned())?;
                let image = fit_frame(image, max_dimensions);
                self.last_frame = Some(image.clone());
                Ok(image)
            }
            Err(CaptureError::Timeout) => self
                .last_frame
                .clone()
                .ok_or_else(|| "dxgi frame timeout before first frame".to_owned()),
            Err(CaptureError::AccessLost) => {
                self.manager = DXGIManager::new(16).map_err(|error| error.to_string())?;
                self.manager.set_capture_source_index(0);
                self.last_frame
                    .clone()
                    .ok_or_else(|| "dxgi access lost before first frame".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

struct GdiCaptureBackend {
    screen_dc: HDC,
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

impl GdiCaptureBackend {
    fn new() -> Result<Self, String> {
        let (width, height) = current_screen_size()?;
        Self::create(width, height)
    }

    fn capture(&mut self, max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
        let (width, height) = current_screen_size()?;
        if width != self.width || height != self.height {
            let replacement = Self::create(width, height)?;
            *self = replacement;
        }

        let blt_ok = unsafe {
            BitBlt(
                self.memory_dc,
                0,
                0,
                self.width,
                self.height,
                self.screen_dc,
                0,
                0,
                SRCCOPY | CAPTUREBLT,
            )
        };
        if blt_ok == 0 {
            return Err("BitBlt failed".to_owned());
        }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.width,
                biHeight: -self.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: (self.width * self.height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [unsafe { std::mem::zeroed() }; 1],
        };

        let scanlines = unsafe {
            GetDIBits(
                self.memory_dc,
                self.bitmap,
                0,
                self.height as u32,
                self.bgra.as_mut_ptr() as *mut _,
                &mut info,
                DIB_RGB_COLORS,
            )
        };

        if scanlines == 0 {
            return Err("GetDIBits failed".to_owned());
        }

        let mut rgba = self.bgra.clone();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let image = ImageBuffer::from_raw(self.width as u32, self.height as u32, rgba)
            .ok_or_else(|| "failed to build RGBA frame".to_owned())?;
        Ok(fit_frame(image, max_dimensions))
    }

    fn create(width: i32, height: i32) -> Result<Self, String> {
        let screen_dc = unsafe { GetDC(0 as HWND) };
        if screen_dc.is_null() {
            return Err("GetDC failed".to_owned());
        }

        let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if memory_dc.is_null() {
            unsafe {
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("CreateCompatibleDC failed".to_owned());
        }

        let bitmap = unsafe { CreateCompatibleBitmap(screen_dc, width, height) };
        if bitmap.is_null() {
            unsafe {
                DeleteDC(memory_dc);
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("CreateCompatibleBitmap failed".to_owned());
        }

        let previous = unsafe { SelectObject(memory_dc, bitmap as HGDIOBJ) };
        if previous.is_null() {
            unsafe {
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(memory_dc);
                ReleaseDC(0 as HWND, screen_dc);
            }
            return Err("SelectObject failed".to_owned());
        }

        Ok(Self {
            screen_dc,
            memory_dc,
            bitmap,
            previous,
            width,
            height,
            bgra: vec![0_u8; (width as usize) * (height as usize) * 4],
        })
    }
}

impl Drop for GdiCaptureBackend {
    fn drop(&mut self) {
        unsafe {
            if !self.memory_dc.is_null() && !self.previous.is_null() {
                SelectObject(self.memory_dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap as HGDIOBJ);
            }
            if !self.memory_dc.is_null() {
                DeleteDC(self.memory_dc);
            }
            if !self.screen_dc.is_null() {
                ReleaseDC(0 as HWND, self.screen_dc);
            }
        }
    }
}

fn current_screen_size() -> Result<(i32, i32), String> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("invalid screen size".to_owned());
    }
    Ok((width, height))
}

fn should_retry_dxgi(error: &str) -> bool {
    error.contains("timeout before first frame") || error.contains("access lost before first frame")
}
