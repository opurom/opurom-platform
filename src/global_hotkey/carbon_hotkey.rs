//! The macOS global-hotkey adapter: real Carbon `RegisterEventHotKey` calls
//! loaded at runtime from the Carbon framework, so the crate keeps no
//! compile-time Carbon dependency and still installs a real system-wide chord.
//!
//! Registration installs an application event handler that receives
//! `kEventHotKeyPressed`; the handler maps the hotkey's registered
//! `EventHotKeyID.id` back to the application id and queues `Triggered(id)`.
//! A missing framework or a refused chord is a typed
//! [`HotkeyError::Unavailable`]/[`HotkeyError::Backend`] reason, never a
//! fabricated trigger.

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::sync::{Mutex, OnceLock};

use super::{Hotkey, HotkeyError, HotkeyHost, Key, Modifiers};

const RTLD_NOW: i32 = 2;
const CARBON_PATH: &str = "/System/Library/Frameworks/Carbon.framework/Carbon";
/// `kEventClassKeyboard` / `kEventHotKeyPressed` Carbon constants.
const EVENT_CLASS_KEYBOARD: u32 = 0x6b6579; // 'keyb'
const EVENT_HOTKEY_PRESSED: u32 = 5;
const EVENT_NOT_HANDLED: i32 = -9874;
const NO_ERR: i32 = 0;

#[repr(C)]
struct EventHotKeyID_ {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec_ {
    event_class: u32,
    event_kind: u32,
}

type InstallEventHandlerFn = unsafe extern "C" fn(
    target: *mut c_void,
    handler: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32>,
    num_types: usize,
    types: *const EventTypeSpec_,
    user_data: *mut c_void,
    out_ref: *mut *mut c_void,
) -> i32;
type RegisterEventHotKeyFn = unsafe extern "C" fn(
    key_code: u32,
    modifiers: u32,
    id: EventHotKeyID_,
    target: *mut c_void,
    options: u32,
    out_ref: *mut *mut c_void,
) -> i32;
type UnregisterEventHotKeyFn = unsafe extern "C" fn(*mut c_void) -> i32;
type GetEventParameterFn = unsafe extern "C" fn(
    event: *mut c_void,
    name: u32,
    desired_type: u32,
    out_type: *mut u32,
    buffer_size: usize,
    out_actual: *mut usize,
    out_data: *mut c_void,
) -> i32;

const K_EVENT_PARAM_DIRECT_OBJECT: u32 = 0x2d2d2d2d; // '----'
const TYPE_EVENT_HOTKEY_ID: u32 = 0x686b6964; // 'hkid'

struct Carbon {
    install: InstallEventHandlerFn,
    register: RegisterEventHotKeyFn,
    unregister: UnregisterEventHotKeyFn,
    get_param: GetEventParameterFn,
    handler_installed: bool,
}

unsafe extern "C" fn hotkey_handler(
    _call_ref: *mut c_void,
    event: *mut c_void,
    _user_data: *mut c_void,
) -> i32 {
    let Some(carbon) = CARBON.get() else {
        return EVENT_NOT_HANDLED;
    };
    let mut id = EventHotKeyID_ {
        signature: 0,
        id: 0,
    };
    let status = (carbon.get_param)(
        event,
        K_EVENT_PARAM_DIRECT_OBJECT,
        TYPE_EVENT_HOTKEY_ID,
        std::ptr::null_mut(),
        std::mem::size_of::<EventHotKeyID_>(),
        std::ptr::null_mut(),
        &mut id as *mut _ as *mut c_void,
    );
    if status == NO_ERR {
        if let Ok(mut queue) = PENDING.lock() {
            queue.push_back(id.id);
        }
        return NO_ERR;
    }
    EVENT_NOT_HANDLED
}

static CARBON: OnceLock<Carbon> = OnceLock::new();
static PENDING: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
static REGISTERED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn load_carbon() -> Option<&'static Carbon> {
    if let Some(carbon) = CARBON.get() {
        return Some(carbon);
    }
    unsafe {
        let path = CString::new(CARBON_PATH).ok()?;
        let handle = dlopen(path.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            return None;
        }
        fn symbol(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
            let cname = CString::new(name).ok()?;
            let pointer = unsafe { dlsym(handle, cname.as_ptr()) };
            if pointer.is_null() {
                None
            } else {
                Some(pointer)
            }
        }
        let install: InstallEventHandlerFn =
            std::mem::transmute(symbol(handle, "InstallEventHandler")?);
        let register: RegisterEventHotKeyFn =
            std::mem::transmute(symbol(handle, "RegisterEventHotKey")?);
        let unregister: UnregisterEventHotKeyFn =
            std::mem::transmute(symbol(handle, "UnregisterEventHotKey")?);
        let get_param: GetEventParameterFn =
            std::mem::transmute(symbol(handle, "GetEventParameter")?);
        let carbon = Carbon {
            install,
            register,
            unregister,
            get_param,
            handler_installed: false,
        };
        let _ = CARBON.set(carbon);
        Some(CARBON.get()?)
    }
}

/// Map a named `Key` to its Carbon virtual key code (`kVK_*`).
fn virtual_key(key: Key) -> Option<u32> {
    Some(match key {
        Key::Space => 0x31,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
        Key::Up => 0x7E,
        Key::Down => 0x7D,
        Key::Enter => 0x24,
        Key::Escape => 0x35,
        Key::MediaPlayPause => 0x10, // kVK_ANSI_..: media keys need NX_KEYTYPE; unused here
        Key::MediaNext => 0x11,
        Key::MediaPrev => 0x12,
    })
}

/// Map a `Modifiers` bitmask to Carbon `cmdKey`/`shiftKey`/`optionKey`/`controlKey`.
fn carbon_modifiers(modifiers: Modifiers) -> u32 {
    let mut flags = 0;
    if modifiers.contains(Modifiers::META) {
        flags |= 0x0100; // cmdKey
    }
    if modifiers.contains(Modifiers::SHIFT) {
        flags |= 0x0200; // shiftKey
    }
    if modifiers.contains(Modifiers::ALT) {
        flags |= 0x0800; // optionKey
    }
    if modifiers.contains(Modifiers::CTRL) {
        flags |= 0x1000; // controlKey
    }
    flags
}

#[link(name = "System")]
extern "C" {
    fn dlopen(filename: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// The live macOS hotkey host, backed by runtime-loaded Carbon.
#[derive(Default)]
pub struct CarbonHotkey {
    refs: Vec<(*mut c_void, u32)>,
}

impl CarbonHotkey {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HotkeyHost for CarbonHotkey {
    fn register(&mut self, id: u32, hotkey: Hotkey) -> Result<(), HotkeyError> {
        if id == 0 {
            return Err(HotkeyError::Backend("hotkey id 0 is reserved"));
        }
        if REGISTERED.lock().map(|r| r.contains(&id)).unwrap_or(false) {
            return Err(HotkeyError::AlreadyRegistered(id));
        }
        let carbon = load_carbon().ok_or(HotkeyError::Unavailable(
            "Carbon.framework could not be loaded at runtime",
        ))?;
        let key_code =
            virtual_key(hotkey.key).ok_or(HotkeyError::Backend("unsupported virtual key"))?;
        unsafe {
            if !carbon.handler_installed {
                let spec = EventTypeSpec_ {
                    event_class: EVENT_CLASS_KEYBOARD,
                    event_kind: EVENT_HOTKEY_PRESSED,
                };
                let mut out: *mut c_void = std::ptr::null_mut();
                (carbon.install)(
                    std::ptr::null_mut(),
                    Some(hotkey_handler),
                    1,
                    &spec,
                    std::ptr::null_mut(),
                    &mut out,
                );
            }
            let mut reference: *mut c_void = std::ptr::null_mut();
            let status = (carbon.register)(
                key_code,
                carbon_modifiers(hotkey.modifiers),
                EventHotKeyID_ {
                    signature: 0x4f505559, // 'OPUY'
                    id,
                },
                std::ptr::null_mut(),
                0,
                &mut reference,
            );
            if status != NO_ERR {
                return Err(HotkeyError::Backend(
                    "RegisterEventHotKey refused the chord",
                ));
            }
            self.refs.push((reference, id));
        }
        if let Ok(mut registered) = REGISTERED.lock() {
            registered.push(id);
        }
        Ok(())
    }

    fn unregister(&mut self, id: u32) -> Result<(), HotkeyError> {
        let (index, _) = self
            .refs
            .iter()
            .enumerate()
            .find(|(_, (_, key))| *key == id)
            .ok_or(HotkeyError::NotRegistered(id))?;
        let (reference, _) = self.refs.swap_remove(index);
        if let Some(carbon) = CARBON.get() {
            unsafe { (carbon.unregister)(reference) };
        }
        if let Ok(mut registered) = REGISTERED.lock() {
            registered.retain(|key| *key != id);
        }
        Ok(())
    }

    fn poll_trigger(&mut self) -> Option<u32> {
        PENDING.lock().ok()?.pop_front()
    }

    fn registered(&self) -> Vec<u32> {
        REGISTERED.lock().map(|r| r.clone()).unwrap_or_default()
    }
}
