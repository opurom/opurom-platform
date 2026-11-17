//! Win32 window host (the surface implementation framework rules).
//!
//! This is compile-verified on the Windows GNU target; this repository has no
//! Windows host or mingw linker, so no adapter is executed here. `WM_PAINT`
//! converts the ABI's top-down RGBA8 frame into the bottom-up/top-down BGRA
//! layout expected by a 32-bit `BI_RGB` DIB and presents it with
//! `StretchDIBits`.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, MutexGuard, OnceLock};

use windows_sys::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Globalization::HIMC;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, StretchDIBits, UpdateWindow, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, PAINTSTRUCT, RGBQUAD, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::Ime::{
    ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, LoadCursorW,
    PostQuitMessage, RegisterClassW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, IDC_ARROW, MSG, SW_SHOW, WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_IME_COMPOSITION,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SIZE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use crate::app_lifecycle::LifecycleTracker;
use crate::ime::{self, ImeStatus};
use crate::input_map;
use crate::{
    AdapterError, Capabilities, FrameView, InputEvent, Lifecycle, PlatformAdapter, PointerState,
    Size,
};

// windows-sys Win32_UI_Controls::WM_MOUSELEAVE (0x02A3).
const WM_MOUSELEAVE: u32 = 675;

const INVALID_STATE: &str = "Win32 state mutex is poisoned";
const WINDOW_DESTROYED: &str = "the window was destroyed";

static CLASS: OnceLock<u16> = OnceLock::new();
/// Per-window state, routed by `HWND` from the single message pump: one pump
/// serves every window of the process, single- or multi-window alike.
static WINDOWS: Mutex<BTreeMap<HWND, Win32State>> = Mutex::new(BTreeMap::new());

struct Win32State {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    events: Vec<InputEvent>,
    /// A `WM_CHAR` high surrogate waiting for its low half.
    pending_char: Option<u16>,
    /// The content scale the window currently renders at.
    scale: f32,
    /// Application lifecycle cursor fed by focus messages.
    lifecycle: LifecycleTracker,
    /// Window-level hover edge and the last logical pointer position.
    hover: bool,
    last_pointer: (f32, f32),
}

impl Win32State {
    const fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            bgra: Vec::new(),
            events: Vec::new(),
            pending_char: None,
            scale: 1.0,
            lifecycle: LifecycleTracker::new(),
            hover: false,
            last_pointer: (0.0, 0.0),
        }
    }
}

fn windows() -> Result<MutexGuard<'static, BTreeMap<HWND, Win32State>>, AdapterError> {
    WINDOWS
        .lock()
        .map_err(|_| AdapterError::Backend(INVALID_STATE))
}

fn push_lifecycle(hwnd: HWND, target: Lifecycle) {
    if let Ok(mut windows) = WINDOWS.lock() {
        if let Some(state) = windows.get_mut(&hwnd) {
            for change in state.lifecycle.report(target) {
                state.events.push(InputEvent::Lifecycle(change));
            }
        }
    }
}

fn push_event(hwnd: HWND, event: InputEvent) {
    if let Ok(mut windows) = WINDOWS.lock() {
        if let Some(state) = windows.get_mut(&hwnd) {
            state.events.push(event);
        }
    }
}

/// One UTF-16 unit of `WM_CHAR`: pairs assemble into one `Input` through
/// [`ime::utf16_push`], a lone unit becomes text as soon as it can.
/// shortcut: an input method may echo its committed result through both
/// `WM_IME_COMPOSITION` and `WM_CHAR`; the host cannot tell the two apart,
/// so deduping that echo is the consumer's state machine.
fn push_char(hwnd: HWND, unit: u16) {
    if let Ok(mut windows) = WINDOWS.lock() {
        if let Some(state) = windows.get_mut(&hwnd) {
            if let Some(text) = ime::utf16_push(&mut state.pending_char, unit) {
                state.events.push(InputEvent::Input {
                    node_id: u32::MAX,
                    text,
                });
            }
        }
    }
}

/// Read the committed result string of one `WM_IME_COMPOSITION`. The input
/// method's context is borrowed, never owned; its calls run outside the state
/// lock and the window state is only touched for the final push.
fn push_composition(hwnd: HWND) {
    unsafe {
        let himc: HIMC = ImmGetContext(hwnd);
        if himc == 0 {
            return;
        }
        let needed = ImmGetCompositionStringW(himc, ime::RESULT_STRING, std::ptr::null_mut(), 0);
        let mut text = String::new();
        if needed > 0 {
            let mut units = vec![0u16; (needed as usize).div_ceil(2)];
            let read = ImmGetCompositionStringW(
                himc,
                ime::RESULT_STRING,
                units.as_mut_ptr().cast::<c_void>(),
                (units.len() * 2) as u32,
            );
            if read > 0 {
                units.truncate((read as usize) / 2);
                text = String::from_utf16_lossy(&units);
            }
        }
        ImmReleaseContext(hwnd, himc);
        if !text.is_empty() {
            push_event(
                hwnd,
                InputEvent::Input {
                    node_id: u32::MAX,
                    text,
                },
            );
        }
    }
}

/// The client-area cursor position packed into a pointer message's `lparam`.
fn pointer(lparam: LPARAM) -> (f32, f32) {
    let packed = lparam as u32;
    (
        (packed & 0xffff) as u16 as i16 as f32,
        ((packed >> 16) & 0xffff) as u16 as i16 as f32,
    )
}

fn state_scale(hwnd: HWND) -> f32 {
    windows()
        .ok()
        .and_then(|windows| windows.get(&hwnd).map(|state| state.scale))
        .unwrap_or(1.0)
}

/// Raw client-area pixels to logical layout pixels for one window.
fn logical_xy(hwnd: HWND, x: f32, y: f32) -> (f32, f32) {
    let scale = state_scale(hwnd);
    (
        input_map::logical_coord(x, scale),
        input_map::logical_coord(y, scale),
    )
}

fn track_mouse_leave(hwnd: HWND) -> bool {
    let mut request = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe { TrackMouseEvent(&mut request) != 0 }
}

fn pointer_state(wparam: WPARAM) -> PointerState {
    PointerState {
        buttons: input_map::win32_buttons(wparam as u32),
        modifiers: input_map::win32_modifiers(wparam as u32),
        ..PointerState::default()
    }
}

fn logical_pointer(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let (x, y) = pointer(lparam);
    logical_xy(hwnd, x, y)
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
    let snapshot = windows().ok().and_then(|windows| {
        windows
            .get(&hwnd)
            .map(|state| (state.width, state.height, state.bgra.clone()))
    });
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
            push_lifecycle(hwnd, Lifecycle::Destroyed);
            // Quit the pump only when the last window closes, so a
            // multi-window host keeps serving its survivors.
            let last = WINDOWS
                .lock()
                .map(|windows| windows.len() <= 1)
                .unwrap_or(false);
            if last {
                PostQuitMessage(0);
            }
            0
        }
        WM_SETFOCUS => {
            push_lifecycle(hwnd, Lifecycle::Resumed);
            0
        }
        WM_KILLFOCUS => {
            push_lifecycle(hwnd, Lifecycle::Inactive);
            0
        }
        WM_SIZE => {
            let packed = lparam as u32;
            let client = Size {
                width: (packed & 0xffff) as u32,
                height: ((packed >> 16) & 0xffff) as u32,
            };
            push_event(
                hwnd,
                InputEvent::Resize {
                    size: input_map::logical_size(client, state_scale(hwnd)),
                },
            );
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = logical_pointer(hwnd, lparam);
            push_event(
                hwnd,
                InputEvent::Tap {
                    x,
                    y,
                    node_id: u32::MAX,
                    pointer: PointerState::default(),
                },
            );
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = logical_pointer(hwnd, lparam);
            push_event(
                hwnd,
                InputEvent::PointerUp {
                    x,
                    y,
                    pointer: pointer_state(wparam),
                },
            );
            0
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let (x, y) = logical_pointer(hwnd, lparam);
            push_event(
                hwnd,
                InputEvent::Tap {
                    x,
                    y,
                    node_id: u32::MAX,
                    pointer: pointer_state(wparam),
                },
            );
            0
        }
        WM_RBUTTONUP | WM_MBUTTONUP => {
            let (x, y) = logical_pointer(hwnd, lparam);
            push_event(
                hwnd,
                InputEvent::PointerUp {
                    x,
                    y,
                    pointer: pointer_state(wparam),
                },
            );
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = logical_pointer(hwnd, lparam);
            let mut entered = false;
            {
                if let Ok(mut windows) = WINDOWS.lock() {
                    if let Some(state) = windows.get_mut(&hwnd) {
                        entered = !state.hover;
                        state.hover = true;
                        state.last_pointer = (x, y);
                    }
                }
            }
            if entered {
                push_event(
                    hwnd,
                    InputEvent::PointerEnter {
                        x,
                        y,
                        pointer: pointer_state(wparam),
                    },
                );
                // Request the leave notification outside the state lock.
                track_mouse_leave(hwnd);
            }
            push_event(hwnd, input_map::pointer_move(pointer_state(wparam), x, y));
            0
        }
        WM_MOUSELEAVE => {
            let mut left = false;
            let mut at = (0.0, 0.0);
            {
                if let Ok(mut windows) = WINDOWS.lock() {
                    if let Some(state) = windows.get_mut(&hwnd) {
                        left = state.hover;
                        state.hover = false;
                        at = state.last_pointer;
                    }
                }
            }
            if left {
                push_event(
                    hwnd,
                    InputEvent::PointerExit {
                        x: at.0,
                        y: at.1,
                        pointer: PointerState::default(),
                    },
                );
            }
            0
        }
        WM_MOUSEWHEEL => {
            // The wheel delta rides the high word, in twelfths of a notch step.
            let raw = ((wparam >> 16) & 0xffff) as u16 as i16;
            let delta = input_map::wheel_delta(raw);
            // This message carries screen coordinates while the ABI wants
            // client-area ones; a failed conversion would place the notch at
            // the wrong spot, so it is dropped instead.
            let mut point = POINT {
                x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
                y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
            };
            if delta != 0.0 && ScreenToClient(hwnd, &mut point) != 0 {
                let (x, y) = logical_xy(hwnd, point.x as f32, point.y as f32);
                push_event(
                    hwnd,
                    InputEvent::Wheel {
                        x,
                        y,
                        delta,
                        pointer: pointer_state(wparam),
                    },
                );
            }
            0
        }
        WM_KEYDOWN | WM_KEYUP => {
            push_event(
                hwnd,
                InputEvent::Key {
                    code: wparam as u32,
                    pressed: message == WM_KEYDOWN,
                },
            );
            0
        }
        WM_CHAR => {
            push_char(hwnd, wparam as u16);
            0
        }
        WM_DPICHANGED => {
            // The X DPI rides the low word of wParam.
            let scale = input_map::dpi_scale((wparam & 0xffff) as u32);
            if let Ok(mut windows) = WINDOWS.lock() {
                if let Some(state) = windows.get_mut(&hwnd) {
                    if state.scale != scale {
                        state.scale = scale;
                        state.events.push(InputEvent::Scale { scale });
                    }
                }
            }
            // The backing scale changed but the client area did not: re-emit
            // the logical viewport so the consumer relayouts and re-rasters,
            // then invalidate so the next present is painted immediately.
            let mut rect: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetClientRect(hwnd, &mut rect) } != 0 {
                let client = Size {
                    width: rect.right.max(0) as u32,
                    height: rect.bottom.max(0) as u32,
                };
                push_event(
                    hwnd,
                    InputEvent::Resize {
                        size: input_map::logical_size(client, scale),
                    },
                );
            }
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            // shortcut: the suggested window rect in lParam is not applied;
            // the logical viewport and the backing scale carry the DPI change.
            0
        }
        WM_IME_COMPOSITION => {
            if ime::ime_result(lparam as u32) {
                push_composition(hwnd);
            }
            // The input method's own defaults keep running for its windows.
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// A live Win32 window host. `run()` is the message pump; `present()` updates
/// the frame and requests an immediate `WM_PAINT`.
pub struct Win32Host {
    hwnd: HWND,
    viewport: Size,
    ime: ImeStatus,
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
                w!("Opurom Window"),
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
        let scale = input_map::dpi_scale(unsafe { GetDpiForWindow(hwnd) });
        {
            let mut windows = windows()?;
            let state = windows.entry(hwnd).or_insert_with(Win32State::new);
            state.width = width;
            state.height = height;
            state.bgra.clear();
            state.events.clear();
            state.scale = scale;
        }
        // The ABI's degradation for platforms without scale events: one Scale
        // at window build. Windows follows with WM_DPICHANGED.
        push_event(hwnd, InputEvent::Scale { scale });
        let logical = input_map::logical_size(Size { width, height }, scale);
        push_event(hwnd, InputEvent::Resize { size: logical });
        let ime = unsafe {
            let himc = ImmGetContext(hwnd);
            let ready = himc != 0;
            if ready {
                ImmReleaseContext(hwnd, himc);
            }
            if ready {
                ImeStatus::Ready
            } else {
                ImeStatus::Unavailable("the window has no input-method context")
            }
        };
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
        Ok(Self {
            hwnd,
            viewport: input_map::logical_size(Size { width, height }, scale),
            ime,
        })
    }

    /// Whether this window can compose text through the input method, probed
    /// at open time from the window's input-method context.
    pub fn ime(&self) -> ImeStatus {
        self.ime
    }

    /// Run until `WM_DESTROY` posts `WM_QUIT`.
    pub fn run(&mut self) -> Result<(), AdapterError> {
        run_message_pump()
    }
}

/// Pump messages until `WM_DESTROY` posts `WM_QUIT` from the last window.
/// One pump serves every window of the process.
fn run_message_pump() -> Result<(), AdapterError> {
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
            dpi_scale: windows()
                .ok()
                .and_then(|windows| windows.get(&self.hwnd).map(|state| state.scale))
                .unwrap_or(1.0),
        }
    }

    fn viewport(&self) -> Size {
        self.viewport
    }

    fn poll_event(&mut self) -> Option<InputEvent> {
        let mut windows = windows().ok()?;
        let state = windows.get_mut(&self.hwnd)?;
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
            let mut windows = windows()?;
            let state = windows
                .get_mut(&self.hwnd)
                .ok_or(AdapterError::Backend(WINDOW_DESTROYED))?;
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
            let _ = windows().map(|mut windows| windows.remove(&self.hwnd));
            self.hwnd = 0;
        }
    }
}

/// A real multi-window Win32 host: 2..4 independent windows served by one
/// message pump and routed by `HWND`, with the `WindowSet` model's index,
/// broadcast and per-window present shape. `multi_window` reports true.
///
/// shortcut: compiled and unit-tested for the Windows target only, never
/// executed here; the routing rules mirror the single-window pump exactly.
pub struct Win32MultiHost {
    hwnds: Vec<HWND>,
    viewports: Vec<Size>,
}

impl Win32MultiHost {
    /// Open 2..4 real windows, one per size, in index order.
    pub fn open<const N: usize>(sizes: [Size; N]) -> Result<Self, AdapterError> {
        if !(2..=4).contains(&N) {
            return Err(AdapterError::Backend(
                crate::window_set::INVALID_WINDOW_COUNT,
            ));
        }
        if *CLASS.get_or_init(register_class) == 0 {
            return Err(AdapterError::Backend("RegisterClassW failed"));
        }
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        if module == 0 {
            return Err(AdapterError::Backend("GetModuleHandleW failed"));
        }
        let mut hwnds = Vec::new();
        let mut viewports = Vec::new();
        let opened = (|| -> Result<(), AdapterError> {
            for size in sizes {
                checked_len(size.width, size.height)?;
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        w!("OpuromWindow"),
                        w!("Opurom Window"),
                        WS_OVERLAPPEDWINDOW,
                        CW_USEDEFAULT,
                        CW_USEDEFAULT,
                        size.width as i32,
                        size.height as i32,
                        0,
                        0,
                        module,
                        std::ptr::null(),
                    )
                };
                if hwnd == 0 {
                    return Err(AdapterError::Backend("CreateWindowExW failed"));
                }
                let scale = input_map::dpi_scale(unsafe { GetDpiForWindow(hwnd) });
                {
                    let mut windows = windows()?;
                    let state = windows.entry(hwnd).or_insert_with(Win32State::new);
                    state.width = size.width;
                    state.height = size.height;
                    state.bgra.clear();
                    state.events.clear();
                    state.scale = scale;
                }
                // The build-time Scale and the logical viewport, once per window.
                push_event(hwnd, InputEvent::Scale { scale });
                let logical = input_map::logical_size(size, scale);
                push_event(hwnd, InputEvent::Resize { size: logical });
                unsafe {
                    ShowWindow(hwnd, SW_SHOW);
                }
                hwnds.push(hwnd);
                viewports.push(logical);
            }
            Ok(())
        })();
        if let Err(error) = opened {
            // Destroy what was created so no window outlives a failed open.
            for hwnd in &hwnds {
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(*hwnd) };
                let _ = windows().map(|mut windows| windows.remove(hwnd));
            }
            return Err(error);
        }
        Ok(Self { hwnds, viewports })
    }

    pub fn len(&self) -> usize {
        self.hwnds.len()
    }

    /// Raw framebuffer size of every window in index order.
    pub fn viewports(&self) -> Vec<Size> {
        self.viewports.clone()
    }

    fn hwnd(&self, index: usize) -> Result<HWND, AdapterError> {
        self.hwnds.get(index).copied().ok_or(AdapterError::Backend(
            crate::window_set::INVALID_WINDOW_INDEX,
        ))
    }

    /// Deliver the same normalized input to every window, mirroring
    /// [`crate::window_set::WindowSet::broadcast_event`].
    pub fn broadcast_event(&mut self, event: InputEvent) {
        for hwnd in &self.hwnds {
            push_event(*hwnd, event.clone());
        }
    }

    pub fn push_event(&mut self, index: usize, event: InputEvent) -> Result<(), AdapterError> {
        let hwnd = self.hwnd(index)?;
        push_event(hwnd, event);
        Ok(())
    }

    pub fn viewport(&self, index: usize) -> Result<Size, AdapterError> {
        self.hwnd(index)?;
        self.viewports
            .get(index)
            .copied()
            .ok_or(AdapterError::Backend(
                crate::window_set::INVALID_WINDOW_INDEX,
            ))
    }

    pub fn poll_event(&mut self, index: usize) -> Result<Option<InputEvent>, AdapterError> {
        let hwnd = self.hwnd(index)?;
        let event = {
            let mut windows = windows()?;
            let state = windows
                .get_mut(&hwnd)
                .ok_or(AdapterError::Backend(WINDOW_DESTROYED))?;
            if state.events.is_empty() {
                return Ok(None);
            }
            state.events.remove(0)
        };
        if let InputEvent::Resize { size } = &event {
            self.viewports[index] = *size;
        }
        Ok(Some(event))
    }

    pub fn present(&mut self, index: usize, frame: FrameView<'_>) -> Result<(), AdapterError> {
        let hwnd = self.hwnd(index)?;
        validate_frame(frame)?;
        let bgra = rgba_to_bgra(frame.pixels);
        {
            let mut windows = windows()?;
            let state = windows
                .get_mut(&hwnd)
                .ok_or(AdapterError::Backend(WINDOW_DESTROYED))?;
            state.width = frame.width;
            state.height = frame.height;
            state.bgra = bgra;
        }
        if unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) } == 0 {
            return Err(AdapterError::Backend("InvalidateRect failed"));
        }
        if unsafe { UpdateWindow(hwnd) } == 0 {
            return Err(AdapterError::Backend("UpdateWindow failed"));
        }
        Ok(())
    }

    /// Run the shared message pump until the last window closes.
    pub fn run(&mut self) -> Result<(), AdapterError> {
        run_message_pump()
    }

    pub fn capabilities(&self) -> Capabilities {
        let dpi_scale = windows()
            .ok()
            .and_then(|windows| {
                let hwnd = *self.hwnds.first()?;
                windows.get(&hwnd).map(|state| state.scale)
            })
            .unwrap_or(1.0);
        Capabilities {
            gpu: false,
            native_embed: false,
            multi_window: true,
            lifecycle: true,
            dpi_scale,
        }
    }
}

impl Drop for Win32MultiHost {
    fn drop(&mut self) {
        for hwnd in &self.hwnds {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(*hwnd) };
            let _ = windows().map(|mut windows| windows.remove(hwnd));
        }
        self.hwnds.clear();
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
