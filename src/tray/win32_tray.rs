//! The Windows tray adapter: a real `Shell_NotifyIconW` icon whose menu is a
//! standard popup tracked with `TrackPopupMenu`, dispatching clicks back into
//! the adapter's event queue.
//!
//! The icon lives on a dedicated message-only style window that owns the
//! callback message; `PollIcon` drains that window's message queue from any
//! thread and maps a chosen command id to the same `TrayEvent` the other
//! platforms produce. Registration failure is a typed
//! [`TrayError::Unavailable`] reason, never a silent no-op.

use std::collections::VecDeque;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, LoadIconW, PeekMessageW, PostMessageW, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, MF_DISABLED, MF_ENABLED, MF_GRAYED,
    MF_STRING, MSG, PM_REMOVE, TPM_RETURNCMD, WM_APP, WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPED,
};

use super::{TrayError, TrayEvent, TrayHost, TrayMenu};

const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
const TRAY_CLASS_NAME: &str = "OpuromTrayWindow";

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The live Windows tray host. `new()` touches no Win32; `mount` creates the
/// message window and adds the icon.
#[derive(Default)]
pub struct Win32Tray {
    hwnd: HWND,
    menu: Option<TrayMenu>,
    events: VecDeque<u32>,
}

impl Win32Tray {
    pub fn new() -> Self {
        Self {
            hwnd: 0,
            menu: None,
            events: VecDeque::new(),
        }
    }
}

impl TrayHost for Win32Tray {
    fn mount(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.hwnd != 0 {
            return Err(TrayError::AlreadyMounted);
        }
        menu.validate()?;
        unsafe {
            let class_name = to_wide(TRAY_CLASS_NAME);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(tray_window_proc),
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
                return Err(TrayError::Unavailable(
                    "CreateWindowExW failed for the tray message window",
                ));
            }
            self.hwnd = hwnd;
            if !notify_add(hwnd, &menu) {
                DestroyWindow(hwnd);
                self.hwnd = 0;
                return Err(TrayError::Unavailable(
                    "Shell_NotifyIconW refused to add the tray icon",
                ));
            }
        }
        if let Ok(mut slot) = TRAY_MENU.lock() {
            *slot = Some(menu.clone());
        }
        self.menu = Some(menu);
        Ok(())
    }

    fn update(&mut self, menu: TrayMenu) -> Result<(), TrayError> {
        if self.hwnd == 0 {
            return Err(TrayError::NotMounted);
        }
        menu.validate()?;
        if !unsafe { notify_modify(self.hwnd, &menu) } {
            return Err(TrayError::Backend("Shell_NotifyIconW modify failed"));
        }
        if let Ok(mut slot) = TRAY_MENU.lock() {
            *slot = Some(menu.clone());
        }
        self.menu = Some(menu);
        Ok(())
    }

    fn poll_event(&mut self) -> Option<TrayEvent> {
        unsafe { drain_messages(self.hwnd) };
        if let Ok(mut events) = TRAY_EVENTS.lock() {
            self.events.extend(events.drain(..));
        }
        if let Some(command) = self.events.pop_front() {
            if self.menu.as_ref().and_then(|m| m.quit_id) == Some(command) {
                return Some(TrayEvent::QuitRequested(command));
            }
            return Some(TrayEvent::ItemClicked(command));
        }
        None
    }

    fn unmount(&mut self) -> Result<(), TrayError> {
        if self.hwnd == 0 {
            return Err(TrayError::NotMounted);
        }
        unsafe {
            notify_delete(self.hwnd);
            DestroyWindow(self.hwnd);
        }
        self.hwnd = 0;
        self.menu = None;
        self.events.clear();
        if let Ok(mut slot) = TRAY_MENU.lock() {
            *slot = None;
        }
        Ok(())
    }

    fn is_mounted(&self) -> bool {
        self.hwnd != 0
    }
}

/// Build and add the notify icon. Returns false when Win32 refuses it.
unsafe fn notify_add(hwnd: HWND, menu: &TrayMenu) -> bool {
    let mut data = notify_data(hwnd, menu);
    Shell_NotifyIconW(NIM_ADD, &mut data) != 0
}

unsafe fn notify_modify(hwnd: HWND, menu: &TrayMenu) -> bool {
    let mut data = notify_data(hwnd, menu);
    Shell_NotifyIconW(NIM_MODIFY, &mut data) != 0
}

unsafe fn notify_delete(hwnd: HWND) {
    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    Shell_NotifyIconW(NIM_DELETE, &mut data);
}

unsafe fn notify_data(hwnd: HWND, menu: &TrayMenu) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_TIP | NIF_ICON;
    data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
    data.hIcon = LoadIconW(0, 32512 as *const u16); // IDI_APPLICATION
    let tip = to_wide(&menu.title);
    for (slot, value) in data.szTip.iter_mut().zip(tip.iter()) {
        *slot = *value;
    }
    data
}

/// Drain this window's queued messages, translating each command into a
/// queued event id. Shared with the window procedure's own dispatch.
unsafe fn drain_messages(hwnd: HWND) {
    let mut msg: MSG = std::mem::zeroed();
    while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

/// The tray window procedure: a right-click pops the menu, and a chosen
/// command posts `WM_COMMAND` back with the item id, which the window procedure
/// turns into the adapter's event stream.
unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        TRAY_CALLBACK_MESSAGE => {
            let event = lparam as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                if let Ok(menu) = TRAY_MENU.lock() {
                    if let Some(menu) = menu.as_ref() {
                        show_popup(hwnd, menu);
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            let command = (wparam & 0xffff) as u32;
            if let Ok(mut events) = TRAY_EVENTS.lock() {
                events.push(command);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Process-global state shared with the window procedure, which cannot capture
// the adapter. A single tray surface per process matches `mount`'s guard.
static TRAY_MENU: Mutex<Option<TrayMenu>> = Mutex::new(None);
static TRAY_EVENTS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

unsafe fn show_popup(hwnd: HWND, menu: &TrayMenu) {
    let popup = CreatePopupMenu();
    if popup == 0 {
        return;
    }
    for item in &menu.items {
        let flags = if item.enabled {
            MF_STRING | MF_ENABLED
        } else {
            MF_STRING | MF_DISABLED | MF_GRAYED
        };
        let label = to_wide(&item.label);
        AppendMenuW(popup, flags, item.id as usize, label.as_ptr());
    }
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    // A foreground window is required so the popup dismisses on an outside click.
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        popup,
        TPM_RETURNCMD,
        point.x,
        point.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    DestroyMenu(popup);
    if command != 0 {
        PostMessageW(hwnd, WM_COMMAND, command as WPARAM, 0);
    }
}
