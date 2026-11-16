//! Wayland client host (the transport implementation framework rules).
//!
//! libwayland-client is loaded at runtime with `dlopen`/`dlsym` instead of a
//! link-time `-lwayland-client` dependency, so a Linux CI runner needs neither
//! Wayland headers nor Wayland development libraries to build this crate. A
//! missing `libwayland-client.so.0` or a missing symbol is a documented
//! `AdapterError`, never a fake connection. The wire encoding itself is pure
//! Rust in `crate::wayland_wire`, so its unit tests run on every host. After
//! the handshake, [`WaylandHost::pump_input`] turns the event object's frames
//! into the ABI's input stream and [`WaylandHost::poll_input`] drains it.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_short, c_ulong, c_void, CString};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use crate::ime::{self, ImeStatus, TextComposer};
use crate::input_map::wire_input;
use crate::wayland_wire::{
    decode_event, decode_output_scale, decode_string, encode_registry_bind, next_event,
    setup_handshake, ObjectIds, DISPLAY_ID, EVENT_OBJECT_ID, WL_CALLBACK_DONE,
    WL_DISPLAY_ERROR_EVENT, WL_OUTPUT_DONE, WL_OUTPUT_SCALE, WL_REGISTRY_GLOBAL,
};
use crate::{AdapterError, InputEvent, WAYLAND_DISPLAY_ENV};

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
const WAYLAND_INPUT_ERROR: &str = "the Wayland event frame did not decode";
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
    /// Bytes read from the socket but not yet consumed by a message loop.
    buffer: Vec<u8>,
    /// Input frames queued for `poll_input`, FIFO.
    events: VecDeque<InputEvent>,
    /// The pointer stream's press bracket: true from `Press` to `Release`.
    pressed: bool,
    /// `(name, version)` of the first announced `wl_seat`, if any.
    seat_global: Option<(u32, u32)>,
    /// `(name, version)` of the text-input manager global, if any.
    text_manager: Option<(u32, u32)>,
    /// The live `zwp_text_input_v3` object, or 0 while unbound.
    text_id: u32,
    composer: TextComposer,
    ime_status: Option<ImeStatus>,
    /// `(name, version)` of the first announced `wl_output`, if any.
    output_global: Option<(u32, u32)>,
    /// The bound output object, or 0 while unbound.
    output_id: u32,
    /// The output scale applied to the stream, once known.
    scale_applied: Option<f32>,
    /// A scale event awaiting its `done` frame.
    scale_pending: Option<f32>,
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
            buffer: Vec::new(),
            events: VecDeque::new(),
            pressed: false,
            seat_global: None,
            text_manager: None,
            text_id: 0,
            composer: TextComposer::default(),
            ime_status: None,
            output_global: None,
            output_id: 0,
            scale_applied: None,
            scale_pending: None,
        })
    }

    /// Run the setup handshake: request the registry, ask for a sync callback
    /// and collect the globals the compositor announces.
    pub fn handshake(&mut self) -> Result<Handshake, AdapterError> {
        let setup = setup_handshake(&mut self.ids)?;
        self.registry_id = setup.registry_id;
        self.write_all(&setup.bytes)?;

        self.buffer.clear();
        let mut report = Handshake::default();
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            // Consume every complete message the buffer holds, in order.
            let mut offset = 0usize;
            while let Some((header, body, size)) = next_event(&self.buffer[offset..])? {
                if header.object_id == self.registry_id && header.opcode == WL_REGISTRY_GLOBAL {
                    // wl_registry.global carries a name word before the interface.
                    if body.len() < 4 {
                        return Err(AdapterError::Backend(WAYLAND_HANDSHAKE_ERROR));
                    }
                    let name = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]);
                    let (interface, used) = decode_string(&body[4..])?;
                    let version_at = 4 + used;
                    let version = if body.len() >= version_at + 4 {
                        u32::from_ne_bytes([
                            body[version_at],
                            body[version_at + 1],
                            body[version_at + 2],
                            body[version_at + 3],
                        ])
                    } else {
                        0
                    };
                    // The text-input half needs the seat and the manager by
                    // name and version to bind them after the handshake.
                    match interface {
                        "wl_seat" if self.seat_global.is_none() => {
                            self.seat_global = Some((name, version));
                        }
                        "zwp_text_input_manager_v3" if self.text_manager.is_none() => {
                            self.text_manager = Some((name, version));
                        }
                        "wl_output" if self.output_global.is_none() => {
                            self.output_global = Some((name, version));
                        }
                        _ => {}
                    }
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
                self.buffer.drain(..offset);
            }
            if report.callback_done {
                break;
            }
            if Instant::now() >= deadline {
                return Err(AdapterError::Backend(WAYLAND_TIMEOUT));
            }
            Self::read_more(self.fd, &mut self.buffer)?;
        }
        if unsafe { (self.lib.get_error)(self.display) } != 0 {
            return Err(AdapterError::Backend(WAYLAND_DISPLAY_ERROR));
        }
        if report.globals.is_empty() {
            return Err(AdapterError::Backend(WAYLAND_HANDSHAKE_ERROR));
        }
        Ok(report)
    }

    /// Drain one read from the connection: queue the input frames the event
    /// object carries, surface a display error as a typed failure and skip the
    /// compositor's own protocol messages. An idle socket waits at most one
    /// 100 ms poll tick. Returns how many input events were queued.
    pub fn pump_input(&mut self) -> Result<usize, AdapterError> {
        Self::read_more(self.fd, &mut self.buffer)?;
        let mut queued = 0usize;
        let mut offset = 0usize;
        while let Some((header, body, size)) = next_event(&self.buffer[offset..])? {
            if header.object_id == DISPLAY_ID && header.opcode == WL_DISPLAY_ERROR_EVENT {
                return Err(AdapterError::Backend(WAYLAND_DISPLAY_ERROR));
            }
            if header.object_id == self.text_id && self.text_id != 0 {
                if let Some(text) = self.composer.feed(header.opcode, body)? {
                    self.events.push_back(InputEvent::Input {
                        node_id: u32::MAX,
                        text,
                    });
                    queued += 1;
                }
            } else if header.object_id == self.output_id && self.output_id != 0 {
                match header.opcode {
                    WL_OUTPUT_SCALE => {
                        self.scale_pending = Some(decode_output_scale(body)? as f32);
                    }
                    WL_OUTPUT_DONE => {
                        if self.scale_pending.is_some() && self.scale_pending != self.scale_applied
                        {
                            self.scale_applied = self.scale_pending;
                            if let Some(scale) = self.scale_applied {
                                self.events.push_back(InputEvent::Scale { scale });
                                queued += 1;
                            }
                        }
                    }
                    // geometry/mode and the v2 name/description frames carry
                    // nothing the ABI's scale channel needs.
                    _ => {}
                }
            } else if header.object_id == EVENT_OBJECT_ID {
                let (frame, _) = decode_event(&self.buffer[offset..])?
                    .ok_or(AdapterError::Backend(WAYLAND_INPUT_ERROR))?;
                let (input, pressed) = wire_input(frame, self.pressed);
                if let Some(event) = input {
                    self.events.push_back(event);
                    queued += 1;
                }
                self.pressed = pressed;
            }
            offset += size;
        }
        if offset > 0 {
            self.buffer.drain(..offset);
        }
        Ok(queued)
    }

    /// Drain queued input events FIFO; never touches the socket, so it cannot
    /// block.
    pub fn poll_input(&mut self) -> Option<InputEvent> {
        self.events.pop_front()
    }

    /// Probe and arm the text-input half on the first call: bind the announced
    /// `wl_seat` and `zwp_text_input_manager_v3`, create the text input and
    /// send the enable/commit pair. Later calls report the stored status; a
    /// compositor that announces neither global degrades with a typed reason.
    ///
    /// shortcut: committed text only arrives while a surface holds keyboard
    /// focus, which 0.1.0's host does not create yet — the mapping is live
    /// from here, and the events begin with the surface work.
    pub fn ime(&mut self) -> Result<ImeStatus, AdapterError> {
        if let Some(status) = self.ime_status {
            return Ok(status);
        }
        let status = self.arm_ime()?;
        self.ime_status = Some(status);
        Ok(status)
    }

    fn arm_ime(&mut self) -> Result<ImeStatus, AdapterError> {
        let Some((seat_name, seat_version)) = self.seat_global else {
            return Ok(ImeStatus::Unavailable(
                "the compositor does not announce wl_seat",
            ));
        };
        let Some((manager_name, manager_version)) = self.text_manager else {
            return Ok(ImeStatus::Unavailable(
                "the compositor does not announce zwp_text_input_manager_v3",
            ));
        };
        let seat_id = self.ids.allocate()?;
        let manager_id = self.ids.allocate()?;
        let text_id = self.ids.allocate()?;
        let mut bytes = encode_registry_bind(
            self.registry_id,
            seat_name,
            "wl_seat",
            seat_version,
            seat_id,
        )?;
        bytes.extend(encode_registry_bind(
            self.registry_id,
            manager_name,
            "zwp_text_input_manager_v3",
            manager_version,
            manager_id,
        )?);
        bytes.extend(ime::encode_get_text_input(manager_id, text_id, seat_id)?);
        bytes.extend(ime::encode_enable_commit(text_id)?);
        self.write_all(&bytes)?;
        self.text_id = text_id;
        Ok(ImeStatus::Ready)
    }

    /// Bind the first announced output and report the content scale it
    /// carries. The value is also queued as `Scale` — once at the first
    /// bind, then again on every change. No announced output, or a v1
    /// output without a scale event, degrades per the ABI: one `Scale{1.0}`
    /// at window build and no further events.
    ///
    /// shortcut: a live window follows the scale of the output its surface
    /// sits on; with no surface yet, the first output stands in.
    pub fn scale(&mut self) -> Result<f32, AdapterError> {
        if self.scale_applied.is_none() && self.output_id == 0 {
            if let Some((name, version)) = self.output_global {
                let id = self.ids.allocate()?;
                let bytes = encode_registry_bind(self.registry_id, name, "wl_output", version, id)?;
                self.write_all(&bytes)?;
                self.output_id = id;
            }
            if self.output_id != 0 {
                // The output's state (geometry, mode, scale) arrives on bind;
                // one read normally carries all of it.
                self.pump_input()?;
            }
            if self.scale_applied.is_none() {
                self.scale_applied = Some(1.0);
                self.events.push_back(InputEvent::Scale { scale: 1.0 });
            }
        }
        Ok(self.scale_applied.expect("applied above"))
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

    fn read_more(fd: c_int, buffer: &mut Vec<u8>) -> Result<(), AdapterError> {
        let mut fds = PollFd {
            fd,
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
        let count = unsafe { recv(fd, chunk.as_mut_ptr().cast::<c_void>(), chunk.len(), 0) };
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
