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

use crate::app_lifecycle::LifecycleTracker;
use crate::ime::{self, ImeStatus, TextComposer};
use crate::input_map::{self, wire_input};
use crate::wayland_scale::OutputScales;
use crate::wayland_wire::{
    decode_empty_event, decode_event, decode_keyboard_enter, decode_keyboard_key,
    decode_keyboard_keymap, decode_keyboard_leave, decode_output_scale, decode_pointer_axis,
    decode_pointer_button, decode_pointer_enter, decode_pointer_leave, decode_pointer_motion,
    decode_string, decode_surface_output, decode_toplevel_configure, decode_xdg_configure_serial,
    encode_ack_configure, encode_create_buffer, encode_create_pool, encode_create_surface,
    encode_pong, encode_registry_bind, encode_seat_device, encode_surface_attach,
    encode_surface_commit, encode_surface_damage, encode_toplevel_string, encode_xdg_get_surface,
    encode_xdg_get_toplevel, fixed_to_f32, next_event, recvmsg_with_fds, send_with_fd,
    setup_handshake, ObjectIds, DISPLAY_ID, EVENT_OBJECT_ID, WL_BUFFER_RELEASE, WL_BUTTON_PRESSED,
    WL_CALLBACK_DONE, WL_DISPLAY_ERROR_EVENT, WL_KEYBOARD_ENTER, WL_KEYBOARD_KEY,
    WL_KEYBOARD_KEYMAP, WL_KEYBOARD_LEAVE, WL_KEY_PRESSED, WL_OUTPUT_DONE, WL_OUTPUT_SCALE,
    WL_POINTER_AXIS, WL_POINTER_AXIS_VERTICAL, WL_POINTER_BUTTON, WL_POINTER_ENTER,
    WL_POINTER_FRAME, WL_POINTER_LEAVE, WL_POINTER_MOTION, WL_POINTER_PRIMARY, WL_REGISTRY_GLOBAL,
    WL_SEAT_GET_KEYBOARD, WL_SEAT_GET_POINTER, WL_SHM_FORMAT_ARGB8888, WL_SURFACE_ENTER,
    WL_SURFACE_LEAVE, XDG_SURFACE_CONFIGURE, XDG_TOPLEVEL_CLOSE, XDG_TOPLEVEL_CONFIGURE,
    XDG_TOPLEVEL_SET_APP_ID, XDG_TOPLEVEL_SET_TITLE, XDG_WM_BASE_PING,
};
use crate::{AdapterError, FrameView, InputEvent, Lifecycle, Size, WAYLAND_DISPLAY_ENV};

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
const WAYLAND_NO_SURFACE: &str = "the compositor does not announce wl_compositor";
const WAYLAND_NO_SHM: &str = "the compositor does not announce wl_shm";
const WAYLAND_NO_SHELL: &str = "the compositor does not announce xdg_wm_base";
const WAYLAND_NO_WINDOW: &str = "no window is open on this connection";
const WAYLAND_WINDOW_OPEN: &str = "a window is already open on this connection";
const WAYLAND_CONFIGURE_TIMEOUT: &str = "the compositor did not configure the window";
const WAYLAND_BUFFER_TIMEOUT: &str = "the compositor did not release a shm buffer";
const WAYLAND_SHM_FAILED: &str = "the shared memory pool could not be created";
const WAYLAND_NO_SEAT: &str = "the compositor does not announce wl_seat";
const WAYLAND_SEAT_TIMEOUT: &str = "the compositor did not report seat capabilities";
/// The handshake waits at most this long for the compositor's reply.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// The configure/pool round trips wait at most this long for the compositor.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(2);
/// Seat capabilities arrive on bind; wait no longer than this for them.
const SEAT_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Pointer events inside one `wl_pointer.frame` are double-buffered by the
/// protocol: position, button transitions and scroll amounts accumulate here
/// and are submitted as ABI events only when `frame` arrives (or immediately
/// on a pre-v5 seat, which has no frame event).
#[derive(Default)]
struct PointerFrame {
    surface: u32,
    position: Option<(f32, f32)>,
    moved: bool,
    buttons: Vec<(u32, bool)>,
    axes: Vec<f32>,
}

impl PointerFrame {
    fn clear(&mut self) {
        self.surface = 0;
        self.position = None;
        self.moved = false;
        self.buttons.clear();
        self.axes.clear();
    }
}

/// The single live window's protocol objects and its double-buffered shm
/// pool. `released` tracks whether a buffer may be reused; the compositor
/// owns a buffer from `wl_surface.commit` until `wl_buffer.release`.
struct WaylandWindow {
    surface_id: u32,
    xdg_surface_id: u32,
    toplevel_id: u32,
    pool_id: u32,
    buffer_ids: [u32; 2],
    released: [bool; 2],
    fd: i32,
    map: *mut u8,
    map_len: usize,
    stride: usize,
    size: Size,
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
    /// Application lifecycle cursor fed by the toplevel configure states.
    lifecycle: LifecycleTracker,
    /// All announced `wl_output` globals, in registry order.
    output_globals: Vec<(u32, u32)>,
    /// Scale state of the bound outputs and of the surface's current output.
    output_scales: OutputScales,
    /// Whether the output globals have been bound once.
    outputs_bound: bool,
    /// `(name, version)` of the first announced `wl_compositor`, if any.
    compositor_global: Option<(u32, u32)>,
    /// `(name, version)` of the first announced `wl_shm`, if any.
    shm_global: Option<(u32, u32)>,
    /// `(name, version)` of the first announced `xdg_wm_base`, if any.
    wm_base_global: Option<(u32, u32)>,
    /// Bound object ids: 0 until the window chain is created.
    server_ids: ServerIds,
    /// The last configure serial awaiting `ack_configure`, and the size the
    /// compositor configured (zero means "client decides").
    configure_serial: Option<u32>,
    configure_size: Option<Size>,
    window: Option<WaylandWindow>,
    /// Seat capabilities bitmask, once announced (pointer=1, keyboard=2).
    seat_capabilities: Option<u32>,
    seat_version: u32,
    keyboard_focus: bool,
    pointer_frame: PointerFrame,
    /// Buttons held between frames, for drag classification.
    pointer_buttons: Vec<u32>,
    /// SCM_RIGHTS descriptors received with stream messages; the stream
    /// layer owns them and closes any the consumer does not take.
    incoming_fds: VecDeque<i32>,
}

/// Bound ids of the window's protocol chain, shared by the event pump and
/// `open_window`; their object ids are known before the window is mapped.
#[derive(Clone, Copy, Default)]
struct ServerIds {
    seat: u32,
    pointer: u32,
    keyboard: u32,
    compositor: u32,
    shm: u32,
    wm_base: u32,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
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
            lifecycle: LifecycleTracker::new(),
            output_globals: Vec::new(),
            output_scales: OutputScales::default(),
            outputs_bound: false,
            compositor_global: None,
            shm_global: None,
            wm_base_global: None,
            server_ids: ServerIds::default(),
            configure_serial: None,
            configure_size: None,
            window: None,
            seat_capabilities: None,
            seat_version: 0,
            keyboard_focus: false,
            pointer_frame: PointerFrame::default(),
            pointer_buttons: Vec::new(),
            incoming_fds: VecDeque::new(),
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
                        "wl_output" => self.output_globals.push((name, version)),
                        "wl_compositor" if self.compositor_global.is_none() => {
                            self.compositor_global = Some((name, version));
                        }
                        "wl_shm" if self.shm_global.is_none() => {
                            self.shm_global = Some((name, version));
                        }
                        "xdg_wm_base" if self.wm_base_global.is_none() => {
                            self.wm_base_global = Some((name, version));
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
            Self::read_more(self.fd, &mut self.buffer, &mut self.incoming_fds)?;
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
        Self::read_more(self.fd, &mut self.buffer, &mut self.incoming_fds)?;
        let mut queued = 0usize;
        let mut offset = 0usize;
        while let Some((header, body, size)) = next_event(&self.buffer[offset..])? {
            if header.object_id == DISPLAY_ID && header.opcode == WL_DISPLAY_ERROR_EVENT {
                return Err(AdapterError::Backend(WAYLAND_DISPLAY_ERROR));
            }
            if header.object_id == self.server_ids.seat && self.server_ids.seat != 0 {
                // wl_seat.capabilities is the seat's first event and carries
                // one bitmask word (pointer=1, keyboard=2).
                if header.opcode == 0 {
                    if body.len() != 4 {
                        return Err(AdapterError::Backend(WAYLAND_INPUT_ERROR));
                    }
                    self.seat_capabilities = Some(decode_xdg_configure_serial(body)?);
                }
            } else if header.object_id == self.server_ids.pointer && self.server_ids.pointer != 0 {
                match header.opcode {
                    WL_POINTER_ENTER => {
                        let (_, surface, x, y) = decode_pointer_enter(body)?;
                        let ours = self.is_our_surface(surface);
                        let frame = &mut self.pointer_frame;
                        frame.surface = surface;
                        frame.position = Some((fixed_to_f32(x), fixed_to_f32(y)));
                        if !ours {
                            frame.surface = 0;
                        }
                    }
                    WL_POINTER_LEAVE => {
                        let surface = decode_pointer_leave(body)?;
                        if self.is_our_surface(surface) {
                            self.pointer_frame.clear();
                            self.pointer_buttons.clear();
                        }
                    }
                    WL_POINTER_MOTION => {
                        let (x, y) = decode_pointer_motion(body)?;
                        if self.pointer_frame.surface != 0 {
                            self.pointer_frame.position = Some((fixed_to_f32(x), fixed_to_f32(y)));
                            self.pointer_frame.moved = true;
                        }
                    }
                    WL_POINTER_BUTTON => {
                        let (button, state) = decode_pointer_button(body)?;
                        if self.pointer_frame.surface != 0 {
                            let down = state == WL_BUTTON_PRESSED;
                            self.pointer_frame.buttons.push((button, down));
                            self.pointer_buttons.retain(|held| *held != button);
                            if down {
                                self.pointer_buttons.push(button);
                            }
                        }
                    }
                    WL_POINTER_AXIS => {
                        let (axis, value) = decode_pointer_axis(body)?;
                        if self.pointer_frame.surface != 0 && axis == WL_POINTER_AXIS_VERTICAL {
                            // Horizontal scroll has no ABI axis yet.
                            self.pointer_frame
                                .axes
                                .push(input_map::wayland_wheel_delta(value));
                        }
                    }
                    WL_POINTER_FRAME => {
                        decode_empty_event(body)?;
                    }
                    _ => {}
                }
                // v5+ groups events into one frame; older seats have no
                // frame event and must be submitted per event.
                if self.seat_version < 5 || header.opcode == WL_POINTER_FRAME {
                    queued += self.submit_pointer_frame();
                }
            } else if header.object_id == self.server_ids.keyboard && self.server_ids.keyboard != 0
            {
                match header.opcode {
                    WL_KEYBOARD_KEYMAP => {
                        let (_format, _size) = decode_keyboard_keymap(body)?;
                        // The keymap fd rides SCM_RIGHTS in this message.
                        // Take it once and close it: real keymap parsing is a
                        // follow-up, and no descriptor may be leaked.
                        if let Some(fd) = self.incoming_fds.pop_front() {
                            unsafe { libc::close(fd) };
                        }
                    }
                    WL_KEYBOARD_ENTER => {
                        let surface = decode_keyboard_enter(body)?;
                        self.keyboard_focus = self.is_our_surface(surface);
                    }
                    WL_KEYBOARD_LEAVE => {
                        let surface = decode_keyboard_leave(body)?;
                        if self.is_our_surface(surface) {
                            self.keyboard_focus = false;
                        }
                    }
                    WL_KEYBOARD_KEY => {
                        let (code, state) = decode_keyboard_key(body)?;
                        if self.keyboard_focus {
                            self.events.push_back(InputEvent::Key {
                                code,
                                pressed: state == WL_KEY_PRESSED,
                            });
                            queued += 1;
                        }
                    }
                    // modifiers and v4 repeat_info have no ABI channel yet.
                    _ => {}
                }
            } else if header.object_id == self.text_id && self.text_id != 0 {
                if let Some(text) = self.composer.feed(header.opcode, body)? {
                    self.events.push_back(InputEvent::Input {
                        node_id: u32::MAX,
                        text,
                    });
                    queued += 1;
                }
            } else if self.output_scales.has(header.object_id) {
                match header.opcode {
                    WL_OUTPUT_SCALE => {
                        let factor = decode_output_scale(body)? as f32;
                        self.output_scales.scale(header.object_id, factor);
                    }
                    WL_OUTPUT_DONE => {
                        decode_empty_event(body)?;
                        if let Some(scale) = self.output_scales.done(header.object_id) {
                            self.events.push_back(InputEvent::Scale { scale });
                            queued += 1;
                        }
                    }
                    // geometry/mode and the v2 name/description frames carry
                    // nothing the ABI's scale channel needs.
                    _ => {}
                }
            } else if self.is_our_surface(header.object_id) {
                match header.opcode {
                    WL_SURFACE_ENTER | WL_SURFACE_LEAVE => {
                        let output = decode_surface_output(body)?;
                        let scale = if header.opcode == WL_SURFACE_ENTER {
                            self.output_scales.enter(output)
                        } else {
                            self.output_scales.leave(output)
                        };
                        if let Some(scale) = scale {
                            self.events.push_back(InputEvent::Scale { scale });
                            queued += 1;
                        }
                    }
                    // v6 preferred_buffer_scale is a second scale source;
                    // output state is authoritative here.
                    _ => {}
                }
            } else if header.object_id == self.server_ids.xdg_surface
                && self.server_ids.xdg_surface != 0
            {
                if header.opcode == XDG_SURFACE_CONFIGURE {
                    self.configure_serial = Some(decode_xdg_configure_serial(body)?);
                }
            } else if header.object_id == self.server_ids.toplevel && self.server_ids.toplevel != 0
            {
                match header.opcode {
                    XDG_TOPLEVEL_CONFIGURE => {
                        let (width, height, activated) = decode_toplevel_configure(body)?;
                        let target = if activated {
                            Lifecycle::Resumed
                        } else {
                            Lifecycle::Inactive
                        };
                        for state in self.lifecycle.report(target) {
                            self.events.push_back(InputEvent::Lifecycle(state));
                            queued += 1;
                        }
                        // Zero or negative means the client decides; the
                        // requested size stands until a real size arrives.
                        if width > 0 && height > 0 {
                            self.configure_size = Some(Size {
                                width: width as u32,
                                height: height as u32,
                            });
                        }
                    }
                    XDG_TOPLEVEL_CLOSE => {
                        for state in self.lifecycle.report(Lifecycle::Destroyed) {
                            self.events.push_back(InputEvent::Lifecycle(state));
                            queued += 1;
                        }
                    }
                    _ => {}
                }
            } else if header.object_id == self.server_ids.wm_base
                && self.server_ids.wm_base != 0
                && header.opcode == XDG_WM_BASE_PING
            {
                // A ping carries the same one-word serial shape as an
                // xdg_surface.configure; the compositor may disconnect an
                // unresponsive client, so always answer it.
                let serial = decode_xdg_configure_serial(body)?;
                self.write_all(&encode_pong(self.server_ids.wm_base, serial)?)?;
            } else if self
                .window
                .as_ref()
                .is_some_and(|window| window.buffer_ids.contains(&header.object_id))
            {
                if header.opcode == WL_BUFFER_RELEASE {
                    decode_empty_event(body)?;
                    if let Some(window) = self.window.as_mut() {
                        if let Some(index) = window
                            .buffer_ids
                            .iter()
                            .position(|id| *id == header.object_id)
                        {
                            window.released[index] = true;
                        }
                    }
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
    /// shortcut: committed text arrives only while the window holds keyboard
    /// focus, and preedit previews have no ABI channel yet so `TextComposer`
    /// still drops them. The committed-text path is live once the window is
    /// mapped; no further protocol work is needed for it.
    pub fn ime(&mut self) -> Result<ImeStatus, AdapterError> {
        if let Some(status) = self.ime_status {
            return Ok(status);
        }
        let status = self.arm_ime()?;
        self.ime_status = Some(status);
        Ok(status)
    }

    fn arm_ime(&mut self) -> Result<ImeStatus, AdapterError> {
        let Some(seat_id) = self.ensure_seat()? else {
            return Ok(ImeStatus::Unavailable(WAYLAND_NO_SEAT));
        };
        let Some((manager_name, manager_version)) = self.text_manager else {
            return Ok(ImeStatus::Unavailable(
                "the compositor does not announce zwp_text_input_manager_v3",
            ));
        };
        let manager_id = self.ids.allocate()?;
        let text_id = self.ids.allocate()?;
        let mut bytes = Vec::new();
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

    /// Bind every announced output and report the scale of the output the
    /// surface currently occupies. `Scale` is queued on the first call, on
    /// surface enter/leave and on the active output's scale change. With no
    /// announced output, or none carrying a scale event, the ABI degrades to
    /// one `Scale{1.0}`.
    pub fn scale(&mut self) -> Result<f32, AdapterError> {
        if !self.outputs_bound {
            self.outputs_bound = true;
            let mut bytes = Vec::new();
            for (name, version) in self.output_globals.clone() {
                let id = self.ids.allocate()?;
                bytes.extend(encode_registry_bind(
                    self.registry_id,
                    name,
                    "wl_output",
                    version,
                    id,
                )?);
                self.output_scales.bind(id);
            }
            self.write_all(&bytes)?;
            if !self.output_globals.is_empty() {
                // The output state arrives on bind; one read normally carries
                // all of it.
                self.pump_input()?;
            }
        }
        if let Some(scale) = self.output_scales.emit_current() {
            self.events.push_back(InputEvent::Scale { scale });
        }
        Ok(self.output_scales.current())
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

    fn read_more(
        fd: c_int,
        buffer: &mut Vec<u8>,
        incoming: &mut VecDeque<i32>,
    ) -> Result<(), AdapterError> {
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
        // recvmsg keeps SCM_RIGHTS descriptors that plain recv() would drop:
        // the wl_keyboard.keymap fd and any future pool fd must be owned here.
        let (count, passed) = recvmsg_with_fds(fd, &mut chunk)
            .map_err(|_| AdapterError::Backend(WAYLAND_READ_ERROR))?;
        incoming.extend(passed);
        if count > 0 {
            buffer.extend_from_slice(&chunk[..count]);
        }
        Ok(())
    }
}

/// Map an anonymous shm pool: the file is passed by fd, never by path, so
/// no filesystem state and no C toolchain enter the picture.
fn anonymous_pool(len: usize) -> Result<(i32, *mut u8), AdapterError> {
    unsafe {
        let fd = libc::memfd_create(b"opurom-shm\0".as_ptr().cast::<c_char>(), 0);
        if fd < 0 {
            return Err(AdapterError::Backend(WAYLAND_SHM_FAILED));
        }
        if libc::ftruncate(fd, len as i64) != 0 {
            libc::close(fd);
            return Err(AdapterError::Backend(WAYLAND_SHM_FAILED));
        }
        let address = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if address == libc::MAP_FAILED {
            libc::close(fd);
            return Err(AdapterError::Backend(WAYLAND_SHM_FAILED));
        }
        Ok((fd, address.cast::<u8>()))
    }
}

impl WaylandHost {
    /// Bind the first announced seat once, shared by text input, pointer and
    /// keyboard. `None` means the compositor announced no seat at all.
    fn ensure_seat(&mut self) -> Result<Option<u32>, AdapterError> {
        if self.server_ids.seat != 0 {
            return Ok(Some(self.server_ids.seat));
        }
        let Some((name, version)) = self.seat_global else {
            return Ok(None);
        };
        let id = self.ids.allocate()?;
        self.write_all(&encode_registry_bind(
            self.registry_id,
            name,
            "wl_seat",
            version,
            id,
        )?)?;
        self.server_ids.seat = id;
        self.seat_version = version;
        Ok(Some(id))
    }

    /// Bind `wl_pointer` and `wl_keyboard` from the seat's announced
    /// capabilities; their events then flow through `poll_input`. A seat
    /// without a device simply produces no events for it.
    pub fn open_input(&mut self) -> Result<(), AdapterError> {
        let Some(seat_id) = self.ensure_seat()? else {
            return Err(AdapterError::Backend(WAYLAND_NO_SEAT));
        };
        let deadline = Instant::now() + SEAT_TIMEOUT;
        while self.seat_capabilities.is_none() {
            if Instant::now() >= deadline {
                return Err(AdapterError::Backend(WAYLAND_SEAT_TIMEOUT));
            }
            self.pump_input()?;
        }
        let capabilities = self.seat_capabilities.expect("checked above");
        // wl_seat.capability: pointer = 1, keyboard = 2.
        if capabilities & 1 != 0 && self.server_ids.pointer == 0 {
            let id = self.ids.allocate()?;
            self.write_all(&encode_seat_device(seat_id, WL_SEAT_GET_POINTER, id)?)?;
            self.server_ids.pointer = id;
        }
        if capabilities & 2 != 0 && self.server_ids.keyboard == 0 {
            let id = self.ids.allocate()?;
            self.write_all(&encode_seat_device(seat_id, WL_SEAT_GET_KEYBOARD, id)?)?;
            self.server_ids.keyboard = id;
        }
        Ok(())
    }

    /// The routing seam for S3: one surface now, a per-surface lookup later.
    fn is_our_surface(&self, surface: u32) -> bool {
        self.window
            .as_ref()
            .is_some_and(|window| window.surface_id == surface)
    }

    /// Submit one frame's accumulated pointer state to the ABI stream.
    fn submit_pointer_frame(&mut self) -> usize {
        let frame = std::mem::take(&mut self.pointer_frame);
        if frame.surface == 0 || !self.is_our_surface(frame.surface) {
            return 0;
        }
        let mut queued = 0;
        let pressed = !self.pointer_buttons.is_empty();
        if let (Some((x, y)), true) = (frame.position, frame.moved) {
            self.events
                .push_back(input_map::pointer_move(pressed, x, y));
            queued += 1;
        }
        for (button, down) in frame.buttons {
            // shortcut: secondary buttons have no ABI code yet; only the
            // primary button maps to Tap/PointerUp.
            if button != WL_POINTER_PRIMARY {
                continue;
            }
            if let Some((x, y)) = frame.position {
                self.events.push_back(if down {
                    InputEvent::Tap {
                        x,
                        y,
                        node_id: u32::MAX,
                    }
                } else {
                    InputEvent::PointerUp { x, y }
                });
                queued += 1;
            }
        }
        for delta in frame.axes {
            let (x, y) = frame.position.unwrap_or((0.0, 0.0));
            self.events.push_back(InputEvent::Wheel { x, y, delta });
            queued += 1;
        }
        queued
    }

    /// Open a real xdg-shell window. The compositor must announce
    /// `wl_compositor`, `wl_shm` and `xdg_wm_base`; without one of them the
    /// host reports a typed reason instead of inventing a surface.
    ///
    /// shortcut: the initial commit carries no buffer, as xdg-shell requires;
    /// only after `configure` is acknowledged may a buffer be attached.
    /// There is no continuous `wl_surface.frame` repaint pace yet — the host
    /// presents on demand, and no server-side decorations are drawn.
    pub fn open_window(&mut self, size: Size, title: &str) -> Result<(), AdapterError> {
        if self.window.is_some() {
            return Err(AdapterError::Backend(WAYLAND_WINDOW_OPEN));
        }
        let (comp_name, comp_version) = self
            .compositor_global
            .ok_or(AdapterError::Backend(WAYLAND_NO_SURFACE))?;
        let (shm_name, shm_version) = self
            .shm_global
            .ok_or(AdapterError::Backend(WAYLAND_NO_SHM))?;
        let (shell_name, shell_version) = self
            .wm_base_global
            .ok_or(AdapterError::Backend(WAYLAND_NO_SHELL))?;
        checked_window_size(size)?;
        let mut ids = ServerIds::default();
        ids.compositor = self.ids.allocate()?;
        ids.shm = self.ids.allocate()?;
        ids.wm_base = self.ids.allocate()?;
        ids.surface = self.ids.allocate()?;
        ids.xdg_surface = self.ids.allocate()?;
        ids.toplevel = self.ids.allocate()?;
        let mut bytes = Vec::new();
        bytes.extend(encode_registry_bind(
            self.registry_id,
            comp_name,
            "wl_compositor",
            comp_version,
            ids.compositor,
        )?);
        bytes.extend(encode_registry_bind(
            self.registry_id,
            shm_name,
            "wl_shm",
            shm_version,
            ids.shm,
        )?);
        bytes.extend(encode_registry_bind(
            self.registry_id,
            shell_name,
            "xdg_wm_base",
            shell_version,
            ids.wm_base,
        )?);
        self.server_ids = ids;
        self.write_all(&bytes)?;

        let pool_id = self.ids.allocate()?;
        let buffer_ids = [self.ids.allocate()?, self.ids.allocate()?];
        self.configure_serial = None;
        self.configure_size = None;
        let mut setup = encode_create_surface(ids.compositor, ids.surface)?;
        setup.extend(encode_xdg_get_surface(
            ids.wm_base,
            ids.xdg_surface,
            ids.surface,
        )?);
        setup.extend(encode_xdg_get_toplevel(ids.xdg_surface, ids.toplevel)?);
        setup.extend(encode_toplevel_string(
            ids.toplevel,
            XDG_TOPLEVEL_SET_TITLE,
            title,
        )?);
        setup.extend(encode_toplevel_string(
            ids.toplevel,
            XDG_TOPLEVEL_SET_APP_ID,
            "opurom",
        )?);
        // The initial commit must carry no buffer: it makes the compositor
        // send the configure sequence we wait for below.
        setup.extend(encode_surface_commit(ids.surface)?);
        self.write_all(&setup)?;
        // The surface exists now, so arm text input before the compositor
        // sends focus; a compositor without the global degrades typed.
        self.arm_ime()?;

        let deadline = Instant::now() + WINDOW_TIMEOUT;
        while self.configure_serial.is_none() {
            if Instant::now() >= deadline {
                return Err(AdapterError::Backend(WAYLAND_CONFIGURE_TIMEOUT));
            }
            self.pump_input()?;
        }
        let serial = self.configure_serial.take().expect("checked above");
        self.write_all(&encode_ack_configure(ids.xdg_surface, serial)?)?;
        let size = self.configure_size.unwrap_or(size);
        checked_window_size(size)?;
        let stride = (size.width as usize)
            .checked_mul(4)
            .filter(|stride| *stride <= i32::MAX as usize)
            .ok_or(AdapterError::InvalidFrame)?;
        let half = stride
            .checked_mul(size.height as usize)
            .ok_or(AdapterError::InvalidFrame)?;
        let map_len = half
            .checked_mul(2)
            .filter(|len| *len <= i32::MAX as usize)
            .ok_or(AdapterError::InvalidFrame)?;
        let (pool_fd, map) = anonymous_pool(map_len)?;
        let pool_bytes = encode_create_pool(ids.shm, pool_id, self.fd, map_len as i32)?;
        // The pool's fd is meaningless without SCM_RIGHTS: the real handle
        // must travel in the same send as the request.
        send_with_fd(self.fd, &pool_bytes, pool_fd)?;
        let mut buffers = Vec::new();
        buffers.extend(encode_create_buffer(
            pool_id,
            buffer_ids[0],
            0,
            size.width as i32,
            size.height as i32,
            stride as i32,
            WL_SHM_FORMAT_ARGB8888,
        )?);
        buffers.extend(encode_create_buffer(
            pool_id,
            buffer_ids[1],
            half as i32,
            size.width as i32,
            size.height as i32,
            stride as i32,
            WL_SHM_FORMAT_ARGB8888,
        )?);
        self.write_all(&buffers)?;
        self.window = Some(WaylandWindow {
            surface_id: ids.surface,
            xdg_surface_id: ids.xdg_surface,
            toplevel_id: ids.toplevel,
            pool_id,
            buffer_ids,
            released: [true, true],
            fd: pool_fd,
            map,
            map_len,
            stride,
            size,
        });
        self.scale()?;
        self.events.push_back(InputEvent::Resize { size });
        Ok(())
    }

    /// The configured size of the open window, if any.
    pub fn window_size(&self) -> Option<Size> {
        self.window.as_ref().map(|window| window.size)
    }

    /// Copy one ABGR/RGBA frame into a released shm buffer and commit it.
    /// A frame that does not match the configured size is rejected before
    /// any bytes are copied.
    pub fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError> {
        let size = self
            .window
            .as_ref()
            .ok_or(AdapterError::Backend(WAYLAND_NO_WINDOW))?
            .size;
        checked_window_size(size)?;
        if frame.width != size.width || frame.height != size.height {
            return Err(AdapterError::InvalidFrame);
        }
        let deadline = Instant::now() + WINDOW_TIMEOUT;
        let index = loop {
            if let Some(window) = &self.window {
                if let Some(index) = window.released.iter().position(|released| *released) {
                    break index;
                }
            }
            if Instant::now() >= deadline {
                return Err(AdapterError::Backend(WAYLAND_BUFFER_TIMEOUT));
            }
            self.pump_input()?;
        };
        let window = self
            .window
            .as_mut()
            .ok_or(AdapterError::Backend(WAYLAND_NO_WINDOW))?;
        let half = window.map_len / 2;
        let offset = index * half;
        let bytes = frame.width as usize * 4;
        for row in 0..frame.height as usize {
            let from = row * bytes;
            let to = offset + row * window.stride;
            if to + bytes > window.map_len {
                return Err(AdapterError::InvalidFrame);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    frame.pixels.as_ptr().add(from),
                    window.map.add(to),
                    bytes,
                );
            }
        }
        window.released[index] = false;
        let buffer_id = window.buffer_ids[index];
        let surface_id = window.surface_id;
        let mut bytes = encode_surface_attach(surface_id, buffer_id)?;
        bytes.extend(encode_surface_damage(
            surface_id,
            size.width as i32,
            size.height as i32,
        )?);
        bytes.extend(encode_surface_commit(surface_id)?);
        self.write_all(&bytes)
    }

    fn teardown_window(&mut self) {
        let Some(window) = self.window.take() else {
            return;
        };
        let mut destroy = Vec::new();
        for id in window.buffer_ids {
            destroy.push(crate::wayland_wire::encode_message(id, 0, &[]));
        }
        destroy.push(crate::wayland_wire::encode_message(window.pool_id, 0, &[]));
        destroy.push(crate::wayland_wire::encode_message(
            window.toplevel_id,
            0,
            &[],
        ));
        destroy.push(crate::wayland_wire::encode_message(
            window.xdg_surface_id,
            0,
            &[],
        ));
        destroy.push(crate::wayland_wire::encode_message(
            window.surface_id,
            0,
            &[],
        ));
        for message in destroy.into_iter().flatten() {
            let _ = self.write_all(&message);
        }
        unsafe {
            libc::munmap(window.map.cast::<c_void>(), window.map_len);
            libc::close(window.fd);
        }
    }
}

fn checked_window_size(size: Size) -> Result<(), AdapterError> {
    if size.width == 0
        || size.height == 0
        || size.width > i32::MAX as u32
        || size.height > i32::MAX as u32
    {
        return Err(AdapterError::InvalidFrame);
    }
    Ok(())
}

impl Drop for WaylandHost {
    fn drop(&mut self) {
        for fd in self.incoming_fds.drain(..) {
            unsafe { libc::close(fd) };
        }
        self.teardown_window();
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
