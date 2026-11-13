//! Wayland client host (TRANSPORT_AND_LANGUAGE_ADDITIONS.md section 4).
//!
//! libwayland-client is loaded at runtime with `dlopen`/`dlsym` instead of a
//! link-time `-lwayland-client` dependency, so a Linux CI runner needs neither
//! Wayland headers nor Wayland development libraries to build this crate. A
//! missing `libwayland-client.so.0` or a missing symbol is a documented
//! `AdapterError`, never a fake connection. The wire encoding itself is pure
//! Rust in `crate::wayland_wire`, so its unit tests run on every host.

use std::ffi::{c_char, c_int, c_short, c_ulong, c_void, CString};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use crate::wayland_wire::{
    decode_string, next_event, setup_handshake, ObjectIds, DISPLAY_ID, WL_CALLBACK_DONE,
    WL_DISPLAY_ERROR_EVENT, WL_REGISTRY_GLOBAL,
};
use crate::{AdapterError, WAYLAND_DISPLAY_ENV};

type WlDisplayConnectFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type WlDisplayDisconnectFn = unsafe extern "C" fn(*mut c_void);
type WlDisplayGetErrorFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type WlDisplayGetFdFn = unsafe extern "C" fn(*mut c_void) -> c_int;

const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;
const POLLIN: c_short = 0x001;
const MSG_NOSIGNAL: c_int = 0x4000;
const WAYLAND_SONAMES: [&[u8]; 2] = [b"libwayland-client.so.0\0", b"libwayland-client.so\0"];
const WAYLAND_LOAD_ERROR: &str = "Wayland unavailable: dlopen(libwayland-client.so.0/libwayland-client.so) failed; install the Wayland client runtime or use a headless adapter";
const WAYLAND_SYMBOL_ERROR: &str =
    "Wayland unavailable: libwayland-client is missing a required symbol";
const WAYLAND_NO_DISPLAY: &str = "WAYLAND_DISPLAY is not set";
const WAYLAND_CONNECT_ERROR: &str = "wl_display_connect failed: no Wayland compositor is reachable";
const WAYLAND_FD_ERROR: &str = "wl_display_get_fd failed: the Wayland connection has no socket";
const WAYLAND_WRITE_ERROR: &str = "the Wayland socket could not be written";
const WAYLAND_READ_ERROR: &str = "the Wayland socket could not be read";
const WAYLAND_POLL_ERROR: &str = "poll failed on the Wayland socket";
const WAYLAND_TIMEOUT: &str = "the Wayland compositor did not answer the handshake";
const WAYLAND_DISPLAY_ERROR: &str = "the Wayland compositor reported a wl_display error";
const WAYLAND_HANDSHAKE_ERROR: &str = "the Wayland handshake did not complete";
/// The handshake waits at most this long for the compositor's reply.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

// Modern glibc exports these from libc; there is deliberately no -ldl link.
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *mut c_char;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn send(fd: c_int, buffer: *const c_void, len: usize, flags: c_int) -> isize;
    fn recv(fd: c_int, buffer: *mut c_void, len: usize, flags: c_int) -> isize;
    fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
}

/// All libwayland-client entry points resolved from one runtime library handle.
struct WaylandLibrary {
    handle: *mut c_void,
    connect: WlDisplayConnectFn,
    disconnect: WlDisplayDisconnectFn,
    get_error: WlDisplayGetErrorFn,
    get_fd: WlDisplayGetFdFn,
}

impl WaylandLibrary {
    fn load() -> Result<Self, AdapterError> {
        let handle = unsafe { open_wayland_library() };
        if handle.is_null() {
            return Err(AdapterError::Backend(WAYLAND_LOAD_ERROR));
        }
        let loaded = unsafe { Self::from_handle(handle) };
        if loaded.is_err() {
            // SAFETY: no libwayland value owns the handle after a failed resolution.
            unsafe { dlclose(handle) };
        }
        loaded
    }

    unsafe fn from_handle(handle: *mut c_void) -> Result<Self, AdapterError> {
        Ok(Self {
            handle,
            connect: symbol(handle, b"wl_display_connect\0")?,
            disconnect: symbol(handle, b"wl_display_disconnect\0")?,
            get_error: symbol(handle, b"wl_display_get_error\0")?,
            get_fd: symbol(handle, b"wl_display_get_fd\0")?,
        })
    }
}

impl Drop for WaylandLibrary {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: the handle came from dlopen and is closed exactly once.
            unsafe { dlclose(self.handle) };
            self.handle = null_mut();
        }
    }
}

unsafe fn open_wayland_library() -> *mut c_void {
    for name in WAYLAND_SONAMES {
        let handle = dlopen(name.as_ptr().cast::<c_char>(), RTLD_NOW | RTLD_LOCAL);
        if !handle.is_null() {
            return handle;
        }
        let _ = dlerror();
    }
    null_mut()
}

unsafe fn symbol<T>(handle: *mut c_void, name: &[u8]) -> Result<T, AdapterError> {
    let _ = dlerror();
    let pointer = dlsym(handle, name.as_ptr().cast::<c_char>());
    let error = dlerror();
    if pointer.is_null() || !error.is_null() {
        return Err(AdapterError::Backend(WAYLAND_SYMBOL_ERROR));
    }
    // POSIX permits a void* symbol address to be used as a function pointer;
    // on Linux both representations have the same size.
    Ok(std::mem::transmute_copy::<*mut c_void, T>(&pointer))
}

/// What the compositor answered during the setup handshake.
#[derive(Clone, Debug, Default)]
pub struct Handshake {
    /// The interfaces the compositor announced in its registry events.
    pub globals: Vec<String>,
    /// Whether the `wl_display.sync` callback completed the round trip.
    pub callback_done: bool,
}

/// A live Wayland connection. The wire bytes are written to the compositor's
/// socket with the encoding from `crate::wayland_wire`.
pub struct WaylandHost {
    display: *mut c_void,
    fd: c_int,
    registry_id: u32,
    ids: ObjectIds,
    lib: WaylandLibrary,
}

impl WaylandHost {
    /// Connect to the compositor named by `WAYLAND_DISPLAY`.
    pub fn connect() -> Result<Self, AdapterError> {
        let name = match std::env::var_os(WAYLAND_DISPLAY_ENV) {
            Some(name) => CString::new(name.as_encoded_bytes()).map_err(|_| {
                AdapterError::Backend("WAYLAND_DISPLAY is not a valid display name")
            })?,
            None => return Err(AdapterError::Backend(WAYLAND_NO_DISPLAY)),
        };
        let lib = WaylandLibrary::load()?;
        let display = unsafe { (lib.connect)(name.as_ptr()) };
        if display.is_null() {
            return Err(AdapterError::Backend(WAYLAND_CONNECT_ERROR));
        }
        let fd = unsafe { (lib.get_fd)(display) };
        if fd < 0 {
            // SAFETY: the display came from wl_display_connect above.
            unsafe { (lib.disconnect)(display) };
            return Err(AdapterError::Backend(WAYLAND_FD_ERROR));
        }
        Ok(Self {
            display,
            fd,
            registry_id: 0,
            ids: ObjectIds::new(),
            lib,
        })
    }

    /// Run the setup handshake: request the registry, ask for a sync callback
    /// and collect the globals the compositor announces.
    pub fn handshake(&mut self) -> Result<Handshake, AdapterError> {
        let setup = setup_handshake(&mut self.ids)?;
        self.registry_id = setup.registry_id;
        self.write_all(&setup.bytes)?;

        let mut buffer: Vec<u8> = Vec::new();
        let mut report = Handshake::default();
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            // Consume every complete message the buffer holds, in order.
            let mut offset = 0usize;
            while let Some((header, body, size)) = next_event(&buffer[offset..])? {
                if header.object_id == self.registry_id && header.opcode == WL_REGISTRY_GLOBAL {
                    // wl_registry.global carries a name word before the interface.
                    if body.len() < 4 {
                        return Err(AdapterError::Backend(WAYLAND_HANDSHAKE_ERROR));
                    }
                    let (interface, _) = decode_string(&body[4..])?;
                    report.globals.push(interface.to_string());
                } else if header.object_id == setup.callback_id && header.opcode == WL_CALLBACK_DONE
                {
                    report.callback_done = true;
                } else if header.object_id == DISPLAY_ID && header.opcode == WL_DISPLAY_ERROR_EVENT
                {
                    return Err(AdapterError::Backend(WAYLAND_DISPLAY_ERROR));
                }
                offset += size;
            }
            if offset > 0 {
                buffer.drain(..offset);
            }
            if report.callback_done {
                break;
            }
            if Instant::now() >= deadline {
                return Err(AdapterError::Backend(WAYLAND_TIMEOUT));
            }
            self.read_more(&mut buffer)?;
        }
        if unsafe { (self.lib.get_error)(self.display) } != 0 {
            return Err(AdapterError::Backend(WAYLAND_DISPLAY_ERROR));
        }
        if report.globals.is_empty() {
            return Err(AdapterError::Backend(WAYLAND_HANDSHAKE_ERROR));
        }
        Ok(report)
    }

    fn write_all(&self, bytes: &[u8]) -> Result<(), AdapterError> {
        let mut written = 0usize;
        while written < bytes.len() {
            // SAFETY: the buffer is a valid slice of the remaining bytes.
            let count = unsafe {
                send(
                    self.fd,
                    bytes[written..].as_ptr().cast::<c_void>(),
                    bytes.len() - written,
                    MSG_NOSIGNAL,
                )
            };
            if count <= 0 {
                return Err(AdapterError::Backend(WAYLAND_WRITE_ERROR));
            }
            written += count as usize;
        }
        Ok(())
    }

    fn read_more(&self, buffer: &mut Vec<u8>) -> Result<(), AdapterError> {
        let mut fds = PollFd {
            fd: self.fd,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: the pollfd array holds one live descriptor of the connection.
        let ready = unsafe { poll(&mut fds, 1, 100) };
        if ready < 0 {
            return Err(AdapterError::Backend(WAYLAND_POLL_ERROR));
        }
        if ready == 0 {
            return Ok(());
        }
        let mut chunk = [0u8; 4096];
        // SAFETY: the chunk is a valid writable buffer of the given length.
        let count = unsafe { recv(self.fd, chunk.as_mut_ptr().cast::<c_void>(), chunk.len(), 0) };
        if count < 0 {
            return Err(AdapterError::Backend(WAYLAND_READ_ERROR));
        }
        if count > 0 {
            buffer.extend_from_slice(&chunk[..count as usize]);
        }
        Ok(())
    }
}

impl Drop for WaylandHost {
    fn drop(&mut self) {
        if !self.display.is_null() {
            // SAFETY: the display came from wl_display_connect and is closed once.
            unsafe { (self.lib.disconnect)(self.display) };
            self.display = null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WaylandLibrary;

    #[test]
    fn runtime_loader_reports_missing_wayland_or_resolves_all_symbols() {
        match WaylandLibrary::load() {
            Ok(lib) => assert!(!lib.handle.is_null()),
            Err(error) => assert!(error.to_string().contains("Wayland unavailable"), "{error}"),
        }
    }
}
