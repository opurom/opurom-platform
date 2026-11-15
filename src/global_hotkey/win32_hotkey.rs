//! The Windows global-hotkey adapter: real `RegisterHotKey` calls on a
//! dedicated message window that receives `WM_HOTKEY`.
//!
//! Each registered chord maps to its application id via the `id` argument of
//! `RegisterHotKey`; the window procedure translates a matching `WM_HOTKEY`
//! into the shared `Triggered(id)` stream. `poll_trigger` drains the window's
//! message queue from any thread. A refused registration is a typed
//! [`HotkeyError::Backend`] reason, never a silent success.

use std::collections::VecDeque;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
    TranslateMessage, MSG, PM_REMOVE, WM_HOTKEY, WNDCLASSW, WS_OVERLAPPED,
};

use super::{Hotkey, HotkeyError, HotkeyHost, Key, Modifiers};

const HOTKEY_CLASS_NAME: &str = "OpuromHotkeyWindow";

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Map a `Modifiers` bitmask to the Win32 `MOD_*` flags.
fn win32_modifiers(modifiers: Modifiers) -> u32 {
    let mut flags = 0;
    if modifiers.contains(Modifiers::SHIFT) {
        flags |= MOD_SHIFT;
    }
    if modifiers.contains(Modifiers::CTRL) {
        flags |= MOD_CONTROL;
    }
    if modifiers.contains(Modifiers::ALT) {
        flags |= MOD_ALT;
    }
    if modifiers.contains(Modifiers::META) {
        flags |= MOD_WIN;
    }
    flags
}

/// Map a named `Key` to its Win32 virtual-key code. The media keys use the
/// documented `VK_MEDIA_*` codes; unknown keys are rejected by the caller.
fn virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::Space => 0x20,
        Key::Left => 0x25,
        Key::Right => 0x27,
        Key::Up => 0x26,
        Key::Down => 0x28,
        Key::Enter => 0x0D,
        Key::Escape => 0x1B,
        Key::MediaPlayPause => 0xB3,
        Key::MediaNext => 0xB0,
        Key::MediaPrev => 0xB1,
    })
}

/// The live Windows hotkey host. `new()` touches no Win32; `mount_hidden`
/// (called lazily by `register`) creates the message window.
#[derive(Default)]
pub struct Win32Hotkey {
    hwnd: HWND,
    triggers: VecDeque<u32>,
    registered: Vec<u32>,
}

impl Win32Hotkey {
    pub fn new() -> Self {
        Self {
            hwnd: 0,
            triggers: VecDeque::new(),
            registered: Vec::new(),
        }
    }

    fn ensure_window(&mut self) -> Result<HWND, HotkeyError> {
        if self.hwnd != 0 {
            return Ok(self.hwnd);
        }
        unsafe {
            let class_name = to_wide(HOTKEY_CLASS_NAME);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(hotkey_window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: 0,
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                0,                // parent HWND (none)
                0,                // HMENU (none)
                0,                // hInstance
                std::ptr::null(), // lpParam
            );
            if hwnd == 0 {
                return Err(HotkeyError::Unavailable(
                    "CreateWindowExW failed for the hotkey window",
                ));
            }
            self.hwnd = hwnd;
            Ok(hwnd)
        }
    }
}

impl HotkeyHost for Win32Hotkey {
    fn register(&mut self, id: u32, hotkey: Hotkey) -> Result<(), HotkeyError> {
        if id == 0 {
            return Err(HotkeyError::Backend("hotkey id 0 is reserved"));
        }
        if self.registered.contains(&id) {
            return Err(HotkeyError::AlreadyRegistered(id));
        }
        let vk = virtual_key(hotkey.key).ok_or(HotkeyError::Backend("unsupported virtual key"))?;
        let hwnd = self.ensure_window()?;
        let ok = unsafe { RegisterHotKey(hwnd, id as i32, win32_modifiers(hotkey.modifiers), vk) };
        if ok == 0 {
            return Err(HotkeyError::Backend(
                "RegisterHotKey refused the chord (already claimed?)",
            ));
        }
        self.registered.push(id);
        Ok(())
    }

    fn unregister(&mut self, id: u32) -> Result<(), HotkeyError> {
        if !self.registered.contains(&id) {
            return Err(HotkeyError::NotRegistered(id));
        }
        unsafe { UnregisterHotKey(self.hwnd, id as i32) };
        self.registered.retain(|key| *key != id);
        Ok(())
    }

    fn poll_trigger(&mut self) -> Option<u32> {
        if self.hwnd != 0 {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        TRIGGERS.with(|t| {
            let mut triggers = t.borrow_mut();
            while let Some(id) = triggers.pop() {
                self.triggers.push_back(id);
            }
        });
        self.triggers.pop_front()
    }

    fn registered(&self) -> Vec<u32> {
        self.registered.clone()
    }
}

thread_local! {
    static TRIGGERS: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(Vec::new());
}

unsafe extern "system" fn hotkey_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_HOTKEY {
        TRIGGERS.with(|t| t.borrow_mut().push(wparam as u32));
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// The hotkey window is destroyed with the process; keep the destroy import used.
#[allow(dead_code)]
fn _keep_destroy(hwnd: HWND) {
    unsafe { DestroyWindow(hwnd) };
}
