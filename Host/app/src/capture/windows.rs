use image::{ImageBuffer, RgbaImage};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
        CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HGDIOBJ,
        ReleaseDC, SRCCOPY, SelectObject,
    },
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
};

use super::{
    CaptureFrame,
    common::{ScreenshotsCaptureBackend, fit_frame},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsCaptureBackendKind {
    Win32Gdi,
    ScreenshotsFallback,
}

pub struct WindowsCaptureEngine {
    preferred_backend: WindowsCaptureBackendKind,
    screenshots_fallback: ScreenshotsCaptureBackend,
}

impl WindowsCaptureEngine {
    pub fn new() -> Self {
        Self {
            preferred_backend: WindowsCaptureBackendKind::Win32Gdi,
            screenshots_fallback: ScreenshotsCaptureBackend::with_backend_name(
                "windows-screenshots-fallback",
            ),
        }
    }

    pub fn capture(&mut self, max_dimensions: (u32, u32), frame_index: u32) -> CaptureFrame {
        match self.preferred_backend {
            WindowsCaptureBackendKind::Win32Gdi => match capture_primary_screen_gdi(max_dimensions)
            {
                Ok(image) => CaptureFrame {
                    image,
                    backend: "windows-gdi",
                    used_fallback: false,
                },
                Err(_) => {
                    self.preferred_backend = WindowsCaptureBackendKind::ScreenshotsFallback;
                    self.screenshots_fallback.capture(max_dimensions, frame_index)
                }
            }
            WindowsCaptureBackendKind::ScreenshotsFallback => {
                self.screenshots_fallback.capture(max_dimensions, frame_index)
            }
        }
    }
}

fn capture_primary_screen_gdi(max_dimensions: (u32, u32)) -> Result<RgbaImage, String> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("invalid screen size".to_owned());
    }

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
    let blt_ok = unsafe {
        BitBlt(
            memory_dc,
            0,
            0,
            width,
            height,
            screen_dc,
            0,
            0,
            SRCCOPY | CAPTUREBLT,
        )
    };
    if blt_ok == 0 {
        unsafe {
            SelectObject(memory_dc, previous);
            DeleteObject(bitmap as HGDIOBJ);
            DeleteDC(memory_dc);
            ReleaseDC(0 as HWND, screen_dc);
        }
        return Err("BitBlt failed".to_owned());
    }

    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (width * height * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [unsafe { std::mem::zeroed() }; 1],
    };

    let mut bgra = vec![0_u8; (width as usize) * (height as usize) * 4];
    let scanlines = unsafe {
        GetDIBits(
            memory_dc,
            bitmap,
            0,
            height as u32,
            bgra.as_mut_ptr() as *mut _,
            &mut info,
            DIB_RGB_COLORS,
        )
    };

    unsafe {
        SelectObject(memory_dc, previous);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(memory_dc);
        ReleaseDC(0 as HWND, screen_dc);
    }

    if scanlines == 0 {
        return Err("GetDIBits failed".to_owned());
    }

    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let image = ImageBuffer::from_raw(width as u32, height as u32, bgra)
        .ok_or_else(|| "failed to build RGBA frame".to_owned())?;
    Ok(fit_frame(image, max_dimensions))
}
