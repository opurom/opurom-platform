//! Win32 window host (BROWSER_AND_NATIVE_SURFACES.md section 3).
//!
//! This is compile-verified on the Windows GNU target; this repository has no
//! Windows host or mingw linker, so no adapter is executed here. `WM_PAINT`
//! converts the ABI's top-down RGBA8 frame into the bottom-up/top-down BGRA
//! layout expected by a 32-bit `BI_RGB` DIB and presents it with
//! `StretchDIBits`.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, MutexGuard, OnceLock};

use windows_sys::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, StretchDIBits, UpdateWindow, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT, RGBQUAD, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, LoadCursorW,
    PostQuitMessage, RegisterClassW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, IDC_ARROW, MSG, SW_SHOW, WM_DESTROY, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_PAINT, WM_SIZE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use crate::{AdapterError, Capabilities, FrameView, InputEvent, Lifecycle, PlatformAdapter, Size};

const INVALID_STATE: &str = "Win32 state mutex is poisoned";

// shortcut: one window per process; use per-HWND state for multi-window.
static CLASS: OnceLock<u16> = OnceLock::new();
static STATE: Mutex<Win32State> = Mutex::new(Win32State::new());

struct Win32State {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    events: Vec<InputEvent>,
}

impl Win32State {
    const fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bgra: Vec::new(),
            events: Vec::new(),
        }
    }
}

fn state() -> Result<MutexGuard<'static, Win32State>, AdapterError> {
    STATE
        .lock()
        .map_err(|_| AdapterError::Backend(INVALID_STATE))
}

fn push_event(event: InputEvent) {
    if let Ok(mut state) = STATE.lock() {
        state.events.push(event);
    }
}

fn checked_len(width: u32, height: u32) -> Result<usize, AdapterError> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(AdapterError::InvalidFrame);
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(4))
        .filter(|&count| count <= u32::MAX as usize)
        .ok_or(AdapterError::InvalidFrame)
}

fn validate_frame(frame: FrameView<'_>) -> Result<usize, AdapterError> {
    let expected = checked_len(frame.width, frame.height)?;
    if frame.pixels.len() != expected {
        return Err(AdapterError::InvalidFrame);
    }
    Ok(expected)
}

/// A 32-bit `BI_RGB` DIB stores BGRA bytes; BI_RGB requires a bottom-up image,
/// and a negative height preserves the ABI's top-down row order.
fn rgba_to_bgra(pixels: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(pixels.len());
    for rgba in pixels.chunks_exact(4) {
        bgra.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
    }
    bgra
}

fn register_class() -> u16 {
    // SAFETY: all arguments are null-terminated literals or system constants.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        if module == 0 {
            return 0;
        }
        let class: WNDCLASSW = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: module,
            hIcon: 0,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: w!("OpuromWindow"),
        };
        RegisterClassW(&class)
    }
}

unsafe fn draw_dib(hdc: HDC, hwnd: HWND, width: u32, height: u32, pixels: &[u8]) {
    let mut rect: RECT = std::mem::zeroed();
    if GetClientRect(hwnd, &mut rect) == 0 {
        return;
    }
    let dest_width = rect.right - rect.left;
    let dest_height = rect.bottom - rect.top;
    if dest_width <= 0 || dest_height <= 0 {
        return;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: pixels.len() as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    StretchDIBits(
        hdc,
        rect.left,
        rect.top,
        dest_width,
        dest_height,
        0,
        0,
        width as i32,
        height as i32,
        pixels.as_ptr().cast::<c_void>(),
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

fn paint(hwnd: HWND) {
    let mut paint: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
    if hdc == 0 {
        return;
    }
    // Copy while locked, then release the lock before reentrant GDI calls.
    let snapshot = STATE
        .lock()
        .ok()
        .map(|state| (state.width, state.height, state.bgra.clone()));
    if let Some((width, height, pixels)) = snapshot {
        if checked_len(width, height).is_ok() && pixels.len() == checked_len(width, height).unwrap()
        {
            unsafe { draw_dib(hdc, hwnd, width, height, &pixels) };
        }
    }
    unsafe { EndPaint(hwnd, &paint) };
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_DESTROY => {
            push_event(InputEvent::Lifecycle(Lifecycle::Destroyed));
            PostQuitMessage(0);
            0
        }
        WM_SIZE => {
            let packed = lparam as u32;
            let width = (packed & 0xffff) as u32;
            let height = ((packed >> 16) & 0xffff) as u32;
            push_event(InputEvent::Resize {
                size: Size { width, height },
            });
            0
        }
        WM_LBUTTONDOWN => {
            let packed = lparam as u32;
            let x = (packed & 0xffff) as u16 as i16 as f32;
            let y = ((packed >> 16) & 0xffff) as u16 as i16 as f32;
            push_event(InputEvent::Tap {
                x,
                y,
                node_id: u32::MAX,
            });
            0
        }
        WM_KEYDOWN | WM_KEYUP => {
            push_event(InputEvent::Key {
                code: wparam as u32,
                pressed: message == WM_KEYDOWN,
            });
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// A live Win32 window host. `run()` is the message pump; `present()` updates
/// the frame and requests an immediate `WM_PAINT`.
pub struct Win32Host {
    hwnd: HWND,
    viewport: Size,
}

impl Win32Host {
    pub fn open(width: u32, height: u32) -> Result<Self, AdapterError> {
        checked_len(width, height)?;
        if *CLASS.get_or_init(register_class) == 0 {
            return Err(AdapterError::Backend("RegisterClassW failed"));
        }
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        if module == 0 {
            return Err(AdapterError::Backend("GetModuleHandleW failed"));
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                w!("OpuromWindow"),
                w!("OPUROM Window"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                0,
                0,
                module,
                std::ptr::null(),
            )
        };
        if hwnd == 0 {
            return Err(AdapterError::Backend("CreateWindowExW failed"));
        }
        {
            let mut state = state()?;
            state.width = width;
            state.height = height;
            state.bgra.clear();
            state.events.clear();
        }
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
        Ok(Self {
            hwnd,
            viewport: Size { width, height },
        })
    }

    /// Run until `WM_DESTROY` posts `WM_QUIT`.
    pub fn run(&mut self) -> Result<(), AdapterError> {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        loop {
            let status = unsafe { GetMessageW(&mut message, 0, 0, 0) };
            if status == 0 {
                return Ok(());
            }
            if status == -1 {
                return Err(AdapterError::Backend("GetMessageW failed"));
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl PlatformAdapter for Win32Host {
    fn name(&self) -> &str {
        "win32"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            gpu: false,
            native_embed: false,
            multi_window: false,
            lifecycle: true,
            dpi_scale: 1.0,
        }
    }

    fn viewport(&self) -> Size {
        self.viewport
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        let mut state = state().ok()?;
        if state.events.is_empty() {
            return None;
        }
        let event = state.events.remove(0);
        if let InputEvent::Resize { size } = &event {
            self.viewport = *size;
        }
        Some(event)
    }

    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        validate_frame(frame)?;
        let bgra = rgba_to_bgra(frame.pixels);
        {
            let mut state = state()?;
            state.width = frame.width;
            state.height = frame.height;
            state.bgra = bgra;
        }
        if unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) } == 0 {
            return Err(AdapterError::Backend("InvalidateRect failed"));
        }
        if unsafe { UpdateWindow(self.hwnd) } == 0 {
            return Err(AdapterError::Backend("UpdateWindow failed"));
        }
        Ok(())
    }
}

impl Drop for Win32Host {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd) };
            self.hwnd = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{checked_len, rgba_to_bgra, validate_frame};
    use crate::{AdapterError, FrameView};

    #[test]
    fn rgba_becomes_bgra_for_a_32_bit_dib() {
        assert_eq!(
            rgba_to_bgra(&[10, 20, 30, 40, 50, 60, 70, 80]),
            vec![30, 20, 10, 40, 70, 60, 50, 80]
        );
    }

    #[test]
    fn frame_lengths_are_checked_before_any_gdi_call() {
        assert_eq!(checked_len(2, 2), Ok(16));
        assert_eq!(checked_len(0, 1), Err(AdapterError::InvalidFrame));
        assert_eq!(
            checked_len(u32::MAX, u32::MAX),
            Err(AdapterError::InvalidFrame)
        );
        assert_eq!(
            validate_frame(FrameView {
                width: 2,
                height: 2,
                pixels: &[0; 15],
            }),
            Err(AdapterError::InvalidFrame)
        );
    }
}
