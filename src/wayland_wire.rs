//! Pure Wayland wire-format encoding (transport additions, framework rules).
//!
//! Kept outside the Linux-gated `wayland` module so the encodings are verified
//! by `cargo test` on every host, including macOS workstations without Wayland.
//! The Wayland wire protocol uses native-endian 32-bit words: a message starts
//! with the object ID, then a word whose upper 16 bits are the total message
//! size (header included) and whose lower 16 bits are the opcode. Strings carry
//! a length word, their NUL-terminated bytes and zero padding to a word boundary.
//! Native host events are framed with the same layout so a recorded batch can
//! be replayed without a live window. Opcode 5 extends that event stream with
//! a nine-word display-geometry message; opcode 6 carries its resolved layout
//! patch; opcode 7 carries a selected graphics backend capability row.

use crate::display_geometry::{
    DisplayGeometry, FoldRegion, GeometryError, LayoutDecision, LayoutTier, Posture, Rect,
    WindowRole,
};
use crate::graphics_backend::{
    BackendError, Capabilities as GraphicsCapabilities, GraphicsBackend,
    GraphicsBackendCapabilities, HostOs, SelectionReason,
};
use crate::viewport_layout::LayoutPatch;
use crate::{AdapterError, Size};

/// Every message starts with a 32-bit object ID and a 32-bit size/opcode word.
pub const HEADER_LEN: usize = 8;
/// `wl_display` is the singleton object with ID 1.
pub const DISPLAY_ID: u32 = 1;
/// Client-allocated IDs start at 2 because the display object owns ID 1.
pub const FIRST_CLIENT_ID: u32 = 2;
/// The server allocates object IDs from 0xff000000 upwards.
pub const SERVER_ID_START: u32 = 0xff00_0000;

pub const WL_DISPLAY_SYNC: u16 = 0;
pub const WL_DISPLAY_GET_REGISTRY: u16 = 1;
pub const WL_DISPLAY_ERROR_EVENT: u16 = 0;
pub const WL_REGISTRY_BIND: u16 = 0;
pub const WL_REGISTRY_GLOBAL: u16 = 0;
pub const WL_CALLBACK_DONE: u16 = 0;
pub const WL_SHM_CREATE_POOL: u16 = 0;
pub const WL_SHM_POOL_CREATE_BUFFER: u16 = 0;
// `wl_output` events: geometry, mode, done, scale, then the v2 names.
pub const WL_OUTPUT_DONE: u16 = 2;
pub const WL_OUTPUT_SCALE: u16 = 3;
// wl_compositor requests: create_surface, create_region.
pub const WL_COMPOSITOR_CREATE_SURFACE: u16 = 0;
// wl_surface events: enter, leave; each carries one wl_output object id.
pub const WL_SURFACE_ENTER: u16 = 0;
pub const WL_SURFACE_LEAVE: u16 = 1;
// wl_surface requests: destroy, attach, damage, frame, set_opaque_region,
// set_input_region, commit.
pub const WL_SURFACE_ATTACH: u16 = 1;
pub const WL_SURFACE_DAMAGE: u16 = 2;
pub const WL_SURFACE_COMMIT: u16 = 6;
// wl_buffer events: release.
pub const WL_BUFFER_RELEASE: u16 = 0;
// xdg_wm_base requests: destroy, create_positioner, get_xdg_surface, pong;
// events: ping.
pub const XDG_WM_BASE_GET_SURFACE: u16 = 2;
pub const XDG_WM_BASE_PONG: u16 = 3;
pub const XDG_WM_BASE_PING: u16 = 0;
// xdg_surface requests: destroy, get_toplevel, get_popup,
// set_window_geometry, ack_configure; events: configure.
pub const XDG_SURFACE_GET_TOPLEVEL: u16 = 1;
pub const XDG_SURFACE_ACK_CONFIGURE: u16 = 4;
pub const XDG_SURFACE_CONFIGURE: u16 = 0;
// xdg_toplevel requests: destroy, set_parent, set_title, set_app_id, ...;
// events: configure, close, then the v4/v5 additions.
pub const XDG_TOPLEVEL_SET_TITLE: u16 = 2;
pub const XDG_TOPLEVEL_SET_APP_ID: u16 = 3;
pub const XDG_TOPLEVEL_CONFIGURE: u16 = 0;
pub const XDG_TOPLEVEL_CLOSE: u16 = 1;

/// `wl_shm.format`: 32-bit native-endian A:R:G:B 8:8:8:8, four bytes per pixel.
// wl_seat requests: get_pointer, get_keyboard, get_touch, then release (v5).
pub const WL_SEAT_GET_POINTER: u16 = 0;
pub const WL_SEAT_GET_KEYBOARD: u16 = 1;
// wl_pointer events: enter, leave, motion, button, axis, frame (v5), ...
pub const WL_POINTER_ENTER: u16 = 0;
pub const WL_POINTER_LEAVE: u16 = 1;
pub const WL_POINTER_MOTION: u16 = 2;
pub const WL_POINTER_BUTTON: u16 = 3;
pub const WL_POINTER_AXIS: u16 = 4;
pub const WL_POINTER_FRAME: u16 = 5;
// wl_keyboard events: keymap, enter, leave, key, modifiers, ...
pub const WL_KEYBOARD_KEYMAP: u16 = 0;
pub const WL_KEYBOARD_ENTER: u16 = 1;
pub const WL_KEYBOARD_LEAVE: u16 = 2;
pub const WL_KEYBOARD_KEY: u16 = 3;
// wl_pointer.axis enum: 0 is vertical, 1 horizontal; state enums: 1 is down.
pub const WL_POINTER_AXIS_VERTICAL: u32 = 0;
pub const WL_BUTTON_PRESSED: u32 = 1;
pub const WL_KEY_PRESSED: u32 = 1;
pub const WL_POINTER_PRIMARY: u32 = 0x110;

pub const WL_SHM_FORMAT_ARGB8888: u32 = 0;
/// `wl_shm.format`: 32-bit native-endian x:R:G:B 8:8:8:8, four bytes per pixel.
pub const WL_SHM_FORMAT_XRGB8888: u32 = 1;

const BAD_LENGTH: &str = "Wayland message does not fit the 16-bit size field";
const BAD_STRING: &str = "Wayland string argument is invalid";
const BAD_EVENT: &str = "Wayland event header or argument is invalid";
const BAD_FORMAT: &str = "unsupported wl_shm format";
const BAD_STRIDE: &str = "wl_shm stride must cover width * bytes-per-pixel and be word aligned";
const IDS_EXHAUSTED: &str = "Wayland client object IDs are exhausted";

/// Round a byte count up to the next 32-bit word boundary.
pub fn word_align(len: usize) -> Result<usize, AdapterError> {
    let padding = (4 - len % 4) % 4;
    len.checked_add(padding)
        .ok_or(AdapterError::Backend(BAD_LENGTH))
}

/// The 16-bit size field counts the 8-byte header and all padding bytes.
pub fn message_size(body_len: usize) -> Result<u16, AdapterError> {
    let size = word_align(body_len)?
        .checked_add(HEADER_LEN)
        .ok_or(AdapterError::Backend(BAD_LENGTH))?;
    u16::try_from(size).map_err(|_| AdapterError::Backend(BAD_LENGTH))
}

/// Encode the 8-byte header in the host's byte order.
pub fn header(object_id: u32, opcode: u16, size: u16) -> [u8; 8] {
    let size_and_opcode = (u32::from(size) << 16) | u32::from(opcode);
    let mut bytes = [0u8; HEADER_LEN];
    bytes[..4].copy_from_slice(&object_id.to_ne_bytes());
    bytes[4..].copy_from_slice(&size_and_opcode.to_ne_bytes());
    bytes
}

/// Encode one request or event and pad its body with zero bytes.
pub fn encode_message(object_id: u32, opcode: u16, body: &[u8]) -> Result<Vec<u8>, AdapterError> {
    let size = message_size(body.len())?;
    let mut message = Vec::with_capacity(usize::from(size));
    message.extend_from_slice(&header(object_id, opcode, size));
    message.extend_from_slice(body);
    message.resize(usize::from(size), 0);
    Ok(message)
}

/// A string argument: uint32 length (including the NUL), the bytes and zero
/// padding. Wire padding is value-agnostic; zero is what libwayland writes.
pub fn encode_string(value: &str) -> Result<Vec<u8>, AdapterError> {
    if value.contains('\0') {
        return Err(AdapterError::Backend(BAD_STRING));
    }
    let nul_terminated = value
        .len()
        .checked_add(1)
        .ok_or(AdapterError::Backend(BAD_STRING))?;
    let len = u32::try_from(nul_terminated).map_err(|_| AdapterError::Backend(BAD_STRING))?;
    let padded = word_align(nul_terminated)?;
    let mut bytes = len.to_ne_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    bytes.resize(4 + padded, 0);
    Ok(bytes)
}

/// A parsed message header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageHeader {
    pub object_id: u32,
    pub opcode: u16,
    pub size: u16,
}

/// Decode an 8-byte header from native-endian wire bytes.
pub fn decode_header(bytes: &[u8]) -> Result<MessageHeader, AdapterError> {
    let Some(head) = bytes.get(..HEADER_LEN) else {
        return Err(AdapterError::Backend(BAD_EVENT));
    };
    let object_id = u32::from_ne_bytes([head[0], head[1], head[2], head[3]]);
    let size_and_opcode = u32::from_ne_bytes([head[4], head[5], head[6], head[7]]);
    let size = (size_and_opcode >> 16) as u16;
    let opcode = size_and_opcode as u16;
    if usize::from(size) < HEADER_LEN || usize::from(size) % 4 != 0 {
        return Err(AdapterError::Backend(BAD_EVENT));
    }
    Ok(MessageHeader {
        object_id,
        opcode,
        size,
    })
}

/// Take the first complete message from a byte stream. Returns `None` when the
/// buffer holds only a partial message; the third tuple field is its full size.
pub fn next_event(bytes: &[u8]) -> Result<Option<(MessageHeader, &[u8], usize)>, AdapterError> {
    if bytes.len() < HEADER_LEN {
        return Ok(None);
    }
    let parsed = decode_header(bytes)?;
    let size = usize::from(parsed.size);
    if bytes.len() < size {
        return Ok(None);
    }
    Ok(Some((parsed, &bytes[HEADER_LEN..size], size)))
}

/// Decode a string argument, returning its text and the consumed byte count.
pub fn decode_string(bytes: &[u8]) -> Result<(&str, usize), AdapterError> {
    if bytes.len() < 4 {
        return Err(AdapterError::Backend(BAD_STRING));
    }
    let len = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if len == 0 || len > bytes.len() - 4 {
        return Err(AdapterError::Backend(BAD_STRING));
    }
    let padded = word_align(len)?;
    if 4 + padded > bytes.len() {
        return Err(AdapterError::Backend(BAD_STRING));
    }
    let raw = &bytes[4..4 + len];
    if raw[len - 1] != 0 || raw[..len - 1].contains(&0) {
        return Err(AdapterError::Backend(BAD_STRING));
    }
    let text =
        std::str::from_utf8(&raw[..len - 1]).map_err(|_| AdapterError::Backend(BAD_STRING))?;
    Ok((text, 4 + padded))
}

/// Client-side object ID allocator. IDs grow from 2 and never enter the range
/// reserved for server-created objects.
#[derive(Clone, Copy, Debug)]
pub struct ObjectIds {
    next: u32,
}

impl ObjectIds {
    pub fn new() -> Self {
        Self {
            next: FIRST_CLIENT_ID,
        }
    }

    pub fn allocate(&mut self) -> Result<u32, AdapterError> {
        if self.next >= SERVER_ID_START {
            return Err(AdapterError::Backend(IDS_EXHAUSTED));
        }
        let id = self.next;
        self.next += 1;
        Ok(id)
    }
}

impl Default for ObjectIds {
    fn default() -> Self {
        Self::new()
    }
}

/// The setup handshake: object 1 (`wl_display`) gets a registry, then a sync
/// callback so the client can wait until all initial globals have arrived.
#[derive(Clone, Debug)]
pub struct SetupHandshake {
    pub registry_id: u32,
    pub callback_id: u32,
    pub bytes: Vec<u8>,
}

pub fn setup_handshake(ids: &mut ObjectIds) -> Result<SetupHandshake, AdapterError> {
    let registry_id = ids.allocate()?;
    let callback_id = ids.allocate()?;
    let mut bytes = encode_message(
        DISPLAY_ID,
        WL_DISPLAY_GET_REGISTRY,
        &registry_id.to_ne_bytes(),
    )?;
    bytes.extend_from_slice(&encode_message(
        DISPLAY_ID,
        WL_DISPLAY_SYNC,
        &callback_id.to_ne_bytes(),
    )?);
    Ok(SetupHandshake {
        registry_id,
        callback_id,
        bytes,
    })
}

/// `wl_registry.bind(uint name, string interface, uint version, new_id id)`.
/// The interface name and version are part of the wire signature; the fd-free
/// argument list is `usun`.
pub fn encode_registry_bind(
    registry_id: u32,
    name: u32,
    interface: &str,
    version: u32,
    new_id: u32,
) -> Result<Vec<u8>, AdapterError> {
    let mut body = name.to_ne_bytes().to_vec();
    body.extend_from_slice(&encode_string(interface)?);
    body.extend_from_slice(&version.to_ne_bytes());
    body.extend_from_slice(&new_id.to_ne_bytes());
    encode_message(registry_id, WL_REGISTRY_BIND, &body)
}

/// `wl_seat.get_pointer(new_id)` and `wl_seat.get_keyboard(new_id)`.
pub fn encode_seat_device(
    seat_id: u32,
    opcode: u16,
    device_id: u32,
) -> Result<Vec<u8>, AdapterError> {
    encode_message(seat_id, opcode, &device_id.to_ne_bytes())
}

fn word_at(body: &[u8], at: usize) -> Result<u32, AdapterError> {
    let slice = body
        .get(at..at + 4)
        .ok_or(AdapterError::Backend(BAD_INPUT_BODY))?;
    Ok(u32::from_ne_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Wayland fixed-point 24.8 coordinates and scroll amounts are signed and
/// divided by 256 to become logical pixels or notches.
pub fn fixed_to_f32(raw: i32) -> f32 {
    raw as f32 / 256.0
}

/// `wl_pointer.enter(serial, surface, surface_x, surface_y)`.
pub fn decode_pointer_enter(body: &[u8]) -> Result<(u32, u32, i32, i32), AdapterError> {
    if body.len() != 16 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((
        word_at(body, 0)?,
        word_at(body, 4)?,
        word_at(body, 8)? as i32,
        word_at(body, 12)? as i32,
    ))
}

/// `wl_pointer.leave(serial, surface)` returns the surface that lost focus.
pub fn decode_pointer_leave(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() != 8 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    word_at(body, 4)
}

/// `wl_pointer.motion(time, surface_x, surface_y)`.
pub fn decode_pointer_motion(body: &[u8]) -> Result<(i32, i32), AdapterError> {
    if body.len() != 8 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((word_at(body, 0)? as i32, word_at(body, 4)? as i32))
}

/// `wl_pointer.button(serial, time, button, state)`.
pub fn decode_pointer_button(body: &[u8]) -> Result<(u32, u32), AdapterError> {
    if body.len() != 16 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((word_at(body, 8)?, word_at(body, 12)?))
}

/// `wl_pointer.axis(time, axis, value)`: the axis enum and the signed 24.8
/// value. A positive vertical value scrolls down, away from the ABI's
/// positive wheel direction.
pub fn decode_pointer_axis(body: &[u8]) -> Result<(u32, i32), AdapterError> {
    if body.len() != 12 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((word_at(body, 4)?, word_at(body, 8)? as i32))
}

/// `wl_keyboard.keymap(format, fd, size)` returns format and size; the fd
/// arrives as SCM_RIGHTS and is owned by the stream layer.
pub fn decode_keyboard_keymap(body: &[u8]) -> Result<(u32, u32), AdapterError> {
    if body.len() != 12 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((word_at(body, 0)?, word_at(body, 8)?))
}

/// `wl_keyboard.enter(serial, surface, keys[])` returns the focused surface
/// and validates the array's word padding.
pub fn decode_keyboard_enter(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() < 12 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    let keys_len = word_at(body, 8)? as usize;
    if body.len() != 12 + word_align(keys_len)? {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    word_at(body, 4)
}

/// `wl_keyboard.leave(serial, surface)` returns the surface that lost focus.
pub fn decode_keyboard_leave(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() != 8 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    word_at(body, 4)
}

/// `wl_keyboard.key(serial, time, key, state)` returns the raw key code and
/// the press state (1 is pressed).
pub fn decode_keyboard_key(body: &[u8]) -> Result<(u32, u32), AdapterError> {
    if body.len() != 16 {
        return Err(AdapterError::Backend(BAD_INPUT_BODY));
    }
    Ok((word_at(body, 8)?, word_at(body, 12)?))
}

/// `wl_compositor.create_surface(new_id)`.
pub fn encode_create_surface(compositor_id: u32, surface_id: u32) -> Result<Vec<u8>, AdapterError> {
    encode_message(
        compositor_id,
        WL_COMPOSITOR_CREATE_SURFACE,
        &surface_id.to_ne_bytes(),
    )
}

/// `xdg_wm_base.get_xdg_surface(new_id, surface)`.
pub fn encode_xdg_get_surface(
    wm_base_id: u32,
    xdg_surface_id: u32,
    surface_id: u32,
) -> Result<Vec<u8>, AdapterError> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&xdg_surface_id.to_ne_bytes());
    body.extend_from_slice(&surface_id.to_ne_bytes());
    encode_message(wm_base_id, XDG_WM_BASE_GET_SURFACE, &body)
}

/// `xdg_surface.get_toplevel(new_id)`.
pub fn encode_xdg_get_toplevel(
    xdg_surface_id: u32,
    toplevel_id: u32,
) -> Result<Vec<u8>, AdapterError> {
    encode_message(
        xdg_surface_id,
        XDG_SURFACE_GET_TOPLEVEL,
        &toplevel_id.to_ne_bytes(),
    )
}

/// `xdg_toplevel.set_title(string)` and `set_app_id(string)` carry one string.
pub fn encode_toplevel_string(
    toplevel_id: u32,
    opcode: u16,
    value: &str,
) -> Result<Vec<u8>, AdapterError> {
    encode_message(toplevel_id, opcode, &encode_string(value)?)
}

/// `xdg_surface.ack_configure(uint serial)`.
pub fn encode_ack_configure(xdg_surface_id: u32, serial: u32) -> Result<Vec<u8>, AdapterError> {
    encode_message(
        xdg_surface_id,
        XDG_SURFACE_ACK_CONFIGURE,
        &serial.to_ne_bytes(),
    )
}

/// `wl_surface.attach(buffer, 0, 0)` — 0.1.0 always attaches at the origin.
pub fn encode_surface_attach(surface_id: u32, buffer_id: u32) -> Result<Vec<u8>, AdapterError> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&buffer_id.to_ne_bytes());
    body.extend_from_slice(&0i32.to_ne_bytes());
    body.extend_from_slice(&0i32.to_ne_bytes());
    encode_message(surface_id, WL_SURFACE_ATTACH, &body)
}

/// `wl_surface.damage(0, 0, width, height)` — the full buffer, in bytes.
pub fn encode_surface_damage(
    surface_id: u32,
    width: i32,
    height: i32,
) -> Result<Vec<u8>, AdapterError> {
    let mut body = Vec::with_capacity(16);
    body.extend_from_slice(&0i32.to_ne_bytes());
    body.extend_from_slice(&0i32.to_ne_bytes());
    body.extend_from_slice(&width.to_ne_bytes());
    body.extend_from_slice(&height.to_ne_bytes());
    encode_message(surface_id, WL_SURFACE_DAMAGE, &body)
}

/// `wl_surface.commit()`.
pub fn encode_surface_commit(surface_id: u32) -> Result<Vec<u8>, AdapterError> {
    encode_message(surface_id, WL_SURFACE_COMMIT, &[])
}

/// `xdg_wm_base.pong(uint serial)` — the mandatory answer to a ping.
pub fn encode_pong(wm_base_id: u32, serial: u32) -> Result<Vec<u8>, AdapterError> {
    encode_message(wm_base_id, XDG_WM_BASE_PONG, &serial.to_ne_bytes())
}

/// `xdg_surface.configure(uint serial)`.
pub fn decode_xdg_configure_serial(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() != 4 {
        return Err(AdapterError::Backend(BAD_XDG_BODY));
    }
    Ok(u32::from_ne_bytes([body[0], body[1], body[2], body[3]]))
}

/// `xdg_toplevel.configure(int width, int height, array states)`: a zero
/// width or height means the client decides its own dimension. The states
/// array is validated and otherwise ignored — no state changes the ABI yet.
pub fn decode_toplevel_configure(body: &[u8]) -> Result<(i32, i32), AdapterError> {
    if body.len() < 12 {
        return Err(AdapterError::Backend(BAD_XDG_BODY));
    }
    let width = i32::from_ne_bytes([body[0], body[1], body[2], body[3]]);
    let height = i32::from_ne_bytes([body[4], body[5], body[6], body[7]]);
    let states_len = u32::from_ne_bytes([body[8], body[9], body[10], body[11]]) as usize;
    let padded = word_align(states_len)?;
    if body.len() != 12 + padded {
        return Err(AdapterError::Backend(BAD_XDG_BODY));
    }
    Ok((width, height))
}

/// `xdg_toplevel.close` and `wl_buffer.release` carry no arguments.
pub fn decode_empty_event(body: &[u8]) -> Result<(), AdapterError> {
    if !body.is_empty() {
        return Err(AdapterError::Backend(BAD_XDG_BODY));
    }
    Ok(())
}

/// Both 32-bit wl_shm formats use four bytes per pixel; other fourcc values are
/// not part of this adapter's buffer contract.
pub fn shm_bytes_per_pixel(format: u32) -> Option<u32> {
    match format {
        WL_SHM_FORMAT_ARGB8888 | WL_SHM_FORMAT_XRGB8888 => Some(4),
        _ => None,
    }
}

/// The minimum legal stride for a 32-bit wl_shm buffer.
pub fn shm_stride(width: u32, format: u32) -> Result<i32, AdapterError> {
    let bytes_per_pixel = shm_bytes_per_pixel(format).ok_or(AdapterError::Backend(BAD_FORMAT))?;
    let stride = width
        .checked_mul(bytes_per_pixel)
        .ok_or(AdapterError::Backend(BAD_STRIDE))?;
    i32::try_from(stride).map_err(|_| AdapterError::Backend(BAD_STRIDE))
}

/// `wl_shm.create_pool`'s size must cover `offset + stride * height`.
pub fn shm_pool_size(offset: i32, stride: i32, height: i32) -> Result<i32, AdapterError> {
    if offset < 0 || stride <= 0 || height <= 0 {
        return Err(AdapterError::Backend(BAD_STRIDE));
    }
    let size = i64::from(stride)
        .checked_mul(i64::from(height))
        .and_then(|bytes| bytes.checked_add(i64::from(offset)))
        .ok_or(AdapterError::Backend(BAD_STRIDE))?;
    i32::try_from(size).map_err(|_| AdapterError::Backend(BAD_STRIDE))
}

/// `wl_shm.create_pool(new_id pool, fd, int size)`. The fd travels in
/// `SCM_RIGHTS` ancillary data and therefore takes no bytes in the message.
pub fn encode_create_pool(
    shm_id: u32,
    pool_id: u32,
    fd: i32,
    size: i32,
) -> Result<Vec<u8>, AdapterError> {
    let mut body = pool_id.to_ne_bytes().to_vec();
    // The fd word is the number in the client's table; the real handle rides
    // the SCM_RIGHTS ancillary data of the same send.
    body.extend_from_slice(&fd.to_ne_bytes());
    body.extend_from_slice(&size.to_ne_bytes());
    encode_message(shm_id, WL_SHM_CREATE_POOL, &body)
}

/// `wl_shm_pool.create_buffer(new_id buffer, int offset, int width, int height,
/// int stride, uint format)`.
pub fn encode_create_buffer(
    pool_id: u32,
    buffer_id: u32,
    offset: i32,
    width: i32,
    height: i32,
    stride: i32,
    format: u32,
) -> Result<Vec<u8>, AdapterError> {
    let bytes_per_pixel = shm_bytes_per_pixel(format).ok_or(AdapterError::Backend(BAD_FORMAT))?;
    if width <= 0 || height <= 0 || stride <= 0 || stride % 4 != 0 {
        return Err(AdapterError::Backend(BAD_STRIDE));
    }
    let minimum = i64::from(width) * i64::from(bytes_per_pixel);
    if i64::from(stride) < minimum {
        return Err(AdapterError::Backend(BAD_STRIDE));
    }
    let _ = shm_pool_size(offset, stride, height)?;
    let mut body = Vec::with_capacity(24);
    for value in [
        buffer_id,
        offset as u32,
        width as u32,
        height as u32,
        stride as u32,
        format,
    ] {
        body.extend_from_slice(&value.to_ne_bytes());
    }
    encode_message(pool_id, WL_SHM_POOL_CREATE_BUFFER, &body)
}

/// Host-independent framing for the native hosts' normalized events. Win32
/// packs a resize and signed 16-bit pointer coordinates into `lparam`; X11
/// reports them as `c_int`. Both fit the 32-bit fields below, so the same
/// message bytes are produced on every host. `Geometry` carries the larger
/// nine-word display-geometry payload on the same server-owned object;
/// `LayoutPatch` carries the deterministic decision derived from it;
/// `GraphicsBackend` carries a validated backend capability row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireEvent {
    Resize {
        width: u32,
        height: u32,
    },
    Press {
        x: i32,
        y: i32,
    },
    Release {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
    Focus {
        focused: bool,
    },
    Geometry(DisplayGeometry),
    LayoutPatch(LayoutPatch),
    /// Selected graphics backend capability row and its typed reason.
    GraphicsBackend(GraphicsBackendCapabilities),
}

/// The server-created object that owns the event stream.
pub const EVENT_OBJECT_ID: u32 = SERVER_ID_START;
pub const EVENT_RESIZE: u16 = 0;
pub const EVENT_PRESS: u16 = 1;
pub const EVENT_RELEASE: u16 = 2;
pub const EVENT_MOVE: u16 = 3;
pub const EVENT_FOCUS: u16 = 4;
/// A full display geometry event: nine 32-bit words plus the shared header.
pub const EVENT_GEOMETRY: u16 = 5;
pub const GEOMETRY_BODY_LEN: usize = 36;
pub const GEOMETRY_MESSAGE_LEN: usize = HEADER_LEN + GEOMETRY_BODY_LEN;
/// Word 8 of a geometry event: bit 0 means words 6 and 7 carry a fold region.
pub const GEOMETRY_FLAG_FOLD: u32 = 1;
/// A resolved layout patch: 23 words plus the shared header.
pub const EVENT_LAYOUT_PATCH: u16 = 6;
pub const LAYOUT_PATCH_MAGIC: u32 = 0x4f50_4c31;
pub const LAYOUT_PATCH_BODY_LEN: usize = 23 * 4;
pub const LAYOUT_PATCH_MESSAGE_LEN: usize = HEADER_LEN + LAYOUT_PATCH_BODY_LEN;
/// A selected graphics backend capability row: nine words plus the header.
pub const EVENT_GRAPHICS_BACKEND: u16 = 7;
pub const GRAPHICS_BACKEND_MAGIC: u32 = 0x4f50_4742;
pub const GRAPHICS_BACKEND_BODY_LEN: usize = 9 * 4;
pub const GRAPHICS_BACKEND_MESSAGE_LEN: usize = HEADER_LEN + GRAPHICS_BACKEND_BODY_LEN;

const LAYOUT_PATCH_WORD_COUNT: usize = LAYOUT_PATCH_BODY_LEN / 4;
const GRAPHICS_BACKEND_WORD_COUNT: usize = GRAPHICS_BACKEND_BODY_LEN / 4;

const BAD_EVENT_BODY: &str = "Wayland event body does not match its opcode";
const BAD_FOCUS: &str = "Wayland focus event argument is invalid";
const BAD_BATCH: &str = "Wayland event batch exceeds the event limit";
const BAD_TRAILING: &str = "Wayland event batch ends with a partial message";
const BAD_GEOMETRY: &str = "Wayland display geometry event is invalid";
const BAD_LAYOUT_PATCH: &str = "Wayland layout patch event is invalid";
const BAD_GRAPHICS_BACKEND: &str = "graphics backend capability event is invalid";
const BAD_XDG_BODY: &str = "the xdg-shell event body does not match its opcode";
const BAD_INPUT_BODY: &str = "the wl_pointer/wl_keyboard event body does not match its opcode";

impl WireEvent {
    /// Two-word input/resize messages need no padding; geometry and its layout
    /// patch use their own fixed-word encoders.
    pub fn encode(self) -> Result<Vec<u8>, AdapterError> {
        let (opcode, first, second) = match self {
            WireEvent::Geometry(geometry) => {
                return encode_geometry_event(geometry)
                    .map_err(|_| AdapterError::Backend(BAD_GEOMETRY));
            }
            WireEvent::LayoutPatch(patch) => {
                return encode_layout_patch_event(patch)
                    .map_err(|_| AdapterError::Backend(BAD_LAYOUT_PATCH));
            }
            WireEvent::GraphicsBackend(capabilities) => {
                return encode_graphics_backend_event(capabilities)
                    .map_err(|_| AdapterError::Backend(BAD_GRAPHICS_BACKEND));
            }
            WireEvent::Resize { width, height } => {
                (EVENT_RESIZE, width.to_ne_bytes(), height.to_ne_bytes())
            }
            WireEvent::Press { x, y } => (EVENT_PRESS, x.to_ne_bytes(), y.to_ne_bytes()),
            WireEvent::Release { x, y } => (EVENT_RELEASE, x.to_ne_bytes(), y.to_ne_bytes()),
            WireEvent::Move { x, y } => (EVENT_MOVE, x.to_ne_bytes(), y.to_ne_bytes()),
            WireEvent::Focus { focused } => (
                EVENT_FOCUS,
                (focused as i32).to_ne_bytes(),
                0i32.to_ne_bytes(),
            ),
        };
        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&first);
        body.extend_from_slice(&second);
        encode_message(EVENT_OBJECT_ID, opcode, &body)
    }
}

const GEOMETRY_WORD_COUNT: usize = GEOMETRY_BODY_LEN / 4;

fn geometry_words(geometry: DisplayGeometry) -> [u32; GEOMETRY_WORD_COUNT] {
    let (posture_tag, hinge_angle) = match geometry.posture {
        Posture::Flat => (0, 0),
        Posture::HalfOpened => (1, 0),
        Posture::Book => (2, 0),
        Posture::Hinge { angle } => (3, angle as u32),
    };
    let role_tag = match geometry.window_role {
        WindowRole::Primary => 0,
        WindowRole::Secondary => 1,
        WindowRole::Split => 2,
    };
    let (fold_x, fold_width, flags) = match geometry.fold {
        Some(fold) => (fold.x, fold.width, GEOMETRY_FLAG_FOLD),
        None => (0, 0, 0),
    };
    [
        geometry.width,
        geometry.height,
        geometry.dpi_scale,
        posture_tag,
        hinge_angle,
        role_tag,
        fold_x,
        fold_width,
        flags,
    ]
}

/// Encode a validated `DisplayGeometry` as opcode 5 with nine native-endian
/// words: width, height, DPI scale, posture tag, hinge angle, role tag,
/// fold x, fold width and flags.
pub fn encode_geometry_event(geometry: DisplayGeometry) -> Result<Vec<u8>, GeometryError> {
    geometry.validate()?;
    let mut body = Vec::with_capacity(GEOMETRY_BODY_LEN);
    for word in geometry_words(geometry) {
        body.extend_from_slice(&word.to_ne_bytes());
    }
    encode_message(EVENT_OBJECT_ID, EVENT_GEOMETRY, &body)
        .map_err(|_| GeometryError::InvalidWireLength)
}

fn geometry_from_words(
    words: [u32; GEOMETRY_WORD_COUNT],
) -> Result<DisplayGeometry, GeometryError> {
    let flags = words[8];
    if flags & !GEOMETRY_FLAG_FOLD != 0 {
        return Err(GeometryError::InvalidWireFlags);
    }
    let fold = if flags & GEOMETRY_FLAG_FOLD != 0 {
        Some(FoldRegion {
            x: words[6],
            width: words[7],
        })
    } else {
        if words[6] != 0 || words[7] != 0 {
            return Err(GeometryError::InvalidWireFlags);
        }
        None
    };
    let angle = words[4];
    let posture = match words[3] {
        0..=2 => {
            if angle != 0 {
                return Err(GeometryError::UnexpectedHingeAngle);
            }
            match words[3] {
                0 => Posture::Flat,
                1 => Posture::HalfOpened,
                _ => Posture::Book,
            }
        }
        3 => Posture::Hinge {
            angle: i32::try_from(angle).map_err(|_| GeometryError::HingeAngleOutOfRange)?,
        },
        _ => return Err(GeometryError::InvalidPostureTag),
    };
    let role = match words[5] {
        0 => WindowRole::Primary,
        1 => WindowRole::Secondary,
        2 => WindowRole::Split,
        _ => return Err(GeometryError::InvalidRoleTag),
    };
    DisplayGeometry::new(words[0], words[1], words[2], posture, role, fold)
}

fn decode_geometry_body(body: &[u8]) -> Result<DisplayGeometry, GeometryError> {
    if body.len() != GEOMETRY_BODY_LEN {
        return Err(GeometryError::InvalidWireLength);
    }
    let mut words = [0u32; GEOMETRY_WORD_COUNT];
    for (word, chunk) in words.iter_mut().zip(body.chunks_exact(4)) {
        *word = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    geometry_from_words(words)
}

/// Decode only a geometry event, preserving the concrete typed reason for a
/// malformed header, object, opcode, flags, length or field. A partial message
/// is `Ok(None)`.
pub fn decode_geometry_event(
    bytes: &[u8],
) -> Result<Option<(DisplayGeometry, usize)>, GeometryError> {
    let Some((parsed, body, size)) =
        next_event(bytes).map_err(|_| GeometryError::InvalidWireHeader)?
    else {
        return Ok(None);
    };
    if parsed.object_id != EVENT_OBJECT_ID {
        return Err(GeometryError::InvalidWireObject);
    }
    if parsed.opcode != EVENT_GEOMETRY {
        return Err(GeometryError::InvalidWireOpcode);
    }
    Ok(Some((decode_geometry_body(body)?, size)))
}

const PATCH_GEOMETRY_START: usize = 1;
const PATCH_DECISION_START: usize = PATCH_GEOMETRY_START + GEOMETRY_WORD_COUNT;
const PATCH_AVOID_START: usize = PATCH_DECISION_START + 4;
const PATCH_CONTENT_START: usize = PATCH_AVOID_START + 5;

fn tier_tag(tier: LayoutTier) -> u32 {
    match tier {
        LayoutTier::Compact => 0,
        LayoutTier::Medium => 1,
        LayoutTier::Expanded => 2,
        LayoutTier::Wide => 3,
        LayoutTier::Ultra => 4,
    }
}

fn tier_from_tag(tag: u32) -> Result<LayoutTier, GeometryError> {
    Ok(match tag {
        0 => LayoutTier::Compact,
        1 => LayoutTier::Medium,
        2 => LayoutTier::Expanded,
        3 => LayoutTier::Wide,
        4 => LayoutTier::Ultra,
        _ => return Err(GeometryError::InvalidLayoutPatch),
    })
}

fn layout_patch_words(patch: LayoutPatch) -> [u32; LAYOUT_PATCH_WORD_COUNT] {
    let geometry = geometry_words(patch.geometry);
    let (fold_flag, fold_x, fold_y, fold_width, fold_height) = match patch.decision.avoid_fold {
        Some(rect) => (1u32, rect.x, rect.y, rect.width, rect.height),
        None => (0u32, 0, 0, 0, 0),
    };
    let mut words = [0u32; LAYOUT_PATCH_WORD_COUNT];
    words[0] = LAYOUT_PATCH_MAGIC;
    words[PATCH_GEOMETRY_START..PATCH_DECISION_START].copy_from_slice(&geometry);
    words[PATCH_DECISION_START] = tier_tag(patch.decision.tier);
    words[PATCH_DECISION_START + 1] = patch.decision.columns;
    words[PATCH_DECISION_START + 2] = patch.decision.gutter;
    words[PATCH_DECISION_START + 3] = patch.decision.split_ratio;
    words[PATCH_AVOID_START] = fold_flag;
    words[PATCH_AVOID_START + 1] = fold_x;
    words[PATCH_AVOID_START + 2] = fold_y;
    words[PATCH_AVOID_START + 3] = fold_width;
    words[PATCH_AVOID_START + 4] = fold_height;
    words[PATCH_CONTENT_START] = patch.content.x;
    words[PATCH_CONTENT_START + 1] = patch.content.y;
    words[PATCH_CONTENT_START + 2] = patch.content.width;
    words[PATCH_CONTENT_START + 3] = patch.content.height;
    words
}

/// Encode opcode 6 with 23 native-endian words: magic, nine geometry words,
/// tier/columns/gutter/split ratio, fold-rectangle fields and content rect.
pub fn encode_layout_patch_event(patch: LayoutPatch) -> Result<Vec<u8>, GeometryError> {
    patch.validate()?;
    let mut body = Vec::with_capacity(LAYOUT_PATCH_BODY_LEN);
    for word in layout_patch_words(patch) {
        body.extend_from_slice(&word.to_ne_bytes());
    }
    encode_message(EVENT_OBJECT_ID, EVENT_LAYOUT_PATCH, &body)
        .map_err(|_| GeometryError::InvalidWireLength)
}

fn decode_layout_patch_body(body: &[u8]) -> Result<LayoutPatch, GeometryError> {
    if body.len() != LAYOUT_PATCH_BODY_LEN {
        return Err(GeometryError::InvalidWireLength);
    }
    let mut words = [0u32; LAYOUT_PATCH_WORD_COUNT];
    for (word, chunk) in words.iter_mut().zip(body.chunks_exact(4)) {
        *word = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    if words[0] != LAYOUT_PATCH_MAGIC {
        return Err(GeometryError::InvalidLayoutPatch);
    }
    let mut geometry = [0u32; GEOMETRY_WORD_COUNT];
    geometry.copy_from_slice(&words[PATCH_GEOMETRY_START..PATCH_DECISION_START]);
    let geometry = geometry_from_words(geometry)?;
    let avoid_fold = match words[PATCH_AVOID_START] {
        0 if words[PATCH_AVOID_START + 1..PATCH_CONTENT_START]
            .iter()
            .all(|&word| word == 0) =>
        {
            None
        }
        0 => return Err(GeometryError::InvalidLayoutPatch),
        1 => Some(Rect {
            x: words[PATCH_AVOID_START + 1],
            y: words[PATCH_AVOID_START + 2],
            width: words[PATCH_AVOID_START + 3],
            height: words[PATCH_AVOID_START + 4],
        }),
        _ => return Err(GeometryError::InvalidLayoutPatch),
    };
    let patch = LayoutPatch {
        geometry,
        frame: Size {
            width: geometry.width,
            height: geometry.height,
        },
        content: Rect {
            x: words[PATCH_CONTENT_START],
            y: words[PATCH_CONTENT_START + 1],
            width: words[PATCH_CONTENT_START + 2],
            height: words[PATCH_CONTENT_START + 3],
        },
        decision: LayoutDecision {
            tier: tier_from_tag(words[PATCH_DECISION_START])?,
            columns: words[PATCH_DECISION_START + 1],
            gutter: words[PATCH_DECISION_START + 2],
            avoid_fold,
            split_ratio: words[PATCH_DECISION_START + 3],
        },
    };
    patch.validate()?;
    Ok(patch)
}

/// Decode only a layout patch event, preserving the concrete typed reason for
/// a malformed header, object, opcode or non-canonical payload.
pub fn decode_layout_patch_event(
    bytes: &[u8],
) -> Result<Option<(LayoutPatch, usize)>, GeometryError> {
    let Some((parsed, body, size)) =
        next_event(bytes).map_err(|_| GeometryError::InvalidWireHeader)?
    else {
        return Ok(None);
    };
    if parsed.object_id != EVENT_OBJECT_ID {
        return Err(GeometryError::InvalidWireObject);
    }
    if parsed.opcode != EVENT_LAYOUT_PATCH {
        return Err(GeometryError::InvalidWireOpcode);
    }
    Ok(Some((decode_layout_patch_body(body)?, size)))
}

fn graphics_backend_words(
    event: GraphicsBackendCapabilities,
) -> [u32; GRAPHICS_BACKEND_WORD_COUNT] {
    [
        GRAPHICS_BACKEND_MAGIC,
        event.host_os.tag(),
        event.backend.tag(),
        event.capabilities.max_texture_size,
        event.capabilities.msaa_samples,
        event.capabilities.supported_effects,
        event.capabilities.requires_live_device as u32,
        event.capabilities.max_command_buffers,
        event.reason.tag(),
    ]
}

/// Encode opcode 7 with nine native-endian words: `OPGB` magic, host OS tag,
/// backend tag, the five capability fields and the selection reason tag.
pub fn encode_graphics_backend_event(
    event: GraphicsBackendCapabilities,
) -> Result<Vec<u8>, BackendError> {
    event.validate()?;
    let mut body = Vec::with_capacity(GRAPHICS_BACKEND_BODY_LEN);
    for word in graphics_backend_words(event) {
        body.extend_from_slice(&word.to_ne_bytes());
    }
    encode_message(EVENT_OBJECT_ID, EVENT_GRAPHICS_BACKEND, &body)
        .map_err(|_| BackendError::InvalidWireLength)
}

fn graphics_backend_from_words(
    words: [u32; GRAPHICS_BACKEND_WORD_COUNT],
) -> Result<GraphicsBackendCapabilities, BackendError> {
    if words[0] != GRAPHICS_BACKEND_MAGIC {
        return Err(BackendError::InvalidWireMagic);
    }
    let requires_live_device = match words[6] {
        0 => false,
        1 => true,
        _ => return Err(BackendError::InvalidWireFlag),
    };
    let event = GraphicsBackendCapabilities {
        host_os: HostOs::from_tag(words[1])?,
        backend: GraphicsBackend::from_tag(words[2])?,
        capabilities: GraphicsCapabilities {
            max_texture_size: words[3],
            msaa_samples: words[4],
            supported_effects: words[5],
            requires_live_device,
            max_command_buffers: words[7],
        },
        reason: SelectionReason::from_tag(words[8])?,
    };
    event.validate()?;
    Ok(event)
}

fn decode_graphics_backend_body(body: &[u8]) -> Result<GraphicsBackendCapabilities, BackendError> {
    if body.len() != GRAPHICS_BACKEND_BODY_LEN {
        return Err(BackendError::InvalidWireLength);
    }
    let mut words = [0u32; GRAPHICS_BACKEND_WORD_COUNT];
    for (word, chunk) in words.iter_mut().zip(body.chunks_exact(4)) {
        *word = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    graphics_backend_from_words(words)
}

/// Decode only a graphics backend capability event, preserving the concrete
/// typed reason for malformed tags, capabilities, header, object or opcode.
/// A partial message is `Ok(None)`.
pub fn decode_graphics_backend_event(
    bytes: &[u8],
) -> Result<Option<(GraphicsBackendCapabilities, usize)>, BackendError> {
    let Some((parsed, body, size)) =
        next_event(bytes).map_err(|_| BackendError::InvalidWireHeader)?
    else {
        return Ok(None);
    };
    if parsed.object_id != EVENT_OBJECT_ID {
        return Err(BackendError::InvalidWireObject);
    }
    if parsed.opcode != EVENT_GRAPHICS_BACKEND {
        return Err(BackendError::InvalidWireOpcode);
    }
    Ok(Some((decode_graphics_backend_body(body)?, size)))
}

/// Take the first complete event from a stream. A partial message is
/// `Ok(None)`; a malformed one is a concrete `Err`, never a panic.
pub fn decode_event(bytes: &[u8]) -> Result<Option<(WireEvent, usize)>, AdapterError> {
    let Some((parsed, body, size)) = next_event(bytes)? else {
        return Ok(None);
    };
    if parsed.object_id != EVENT_OBJECT_ID {
        return Err(AdapterError::Backend(BAD_EVENT_BODY));
    }
    if parsed.opcode == EVENT_GEOMETRY {
        let geometry =
            decode_geometry_body(body).map_err(|_| AdapterError::Backend(BAD_GEOMETRY))?;
        return Ok(Some((WireEvent::Geometry(geometry), size)));
    }
    if parsed.opcode == EVENT_LAYOUT_PATCH {
        let patch =
            decode_layout_patch_body(body).map_err(|_| AdapterError::Backend(BAD_LAYOUT_PATCH))?;
        return Ok(Some((WireEvent::LayoutPatch(patch), size)));
    }
    if parsed.opcode == EVENT_GRAPHICS_BACKEND {
        let capabilities = decode_graphics_backend_body(body)
            .map_err(|_| AdapterError::Backend(BAD_GRAPHICS_BACKEND))?;
        return Ok(Some((WireEvent::GraphicsBackend(capabilities), size)));
    }
    if body.len() != 8 {
        return Err(AdapterError::Backend(BAD_EVENT_BODY));
    }
    let first = [body[0], body[1], body[2], body[3]];
    let second = [body[4], body[5], body[6], body[7]];
    let event = match parsed.opcode {
        EVENT_RESIZE => WireEvent::Resize {
            width: u32::from_ne_bytes(first),
            height: u32::from_ne_bytes(second),
        },
        EVENT_PRESS => WireEvent::Press {
            x: i32::from_ne_bytes(first),
            y: i32::from_ne_bytes(second),
        },
        EVENT_RELEASE => WireEvent::Release {
            x: i32::from_ne_bytes(first),
            y: i32::from_ne_bytes(second),
        },
        EVENT_MOVE => WireEvent::Move {
            x: i32::from_ne_bytes(first),
            y: i32::from_ne_bytes(second),
        },
        EVENT_FOCUS => {
            let flag = i32::from_ne_bytes(first);
            if flag != 0 && flag != 1 {
                return Err(AdapterError::Backend(BAD_FOCUS));
            }
            WireEvent::Focus { focused: flag == 1 }
        }
        _ => return Err(AdapterError::Backend(BAD_EVENT_BODY)),
    };
    Ok(Some((event, size)))
}

/// `wl_output.scale(factor)`: the integer content scale of one output.
pub fn decode_output_scale(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() != 4 {
        return Err(AdapterError::Backend(BAD_EVENT_BODY));
    }
    Ok(u32::from_ne_bytes([body[0], body[1], body[2], body[3]]))
}

/// `wl_surface.enter`/`leave`: the single `wl_output` object id.
pub fn decode_surface_output(body: &[u8]) -> Result<u32, AdapterError> {
    if body.len() != 4 {
        return Err(AdapterError::Backend(BAD_EVENT_BODY));
    }
    Ok(u32::from_ne_bytes([body[0], body[1], body[2], body[3]]))
}

/// Decode a whole batch, refusing more than `limit` events. A batch must end
/// on a message boundary: trailing partial bytes are an error, not a short read.
pub fn decode_batch(bytes: &[u8], limit: usize) -> Result<Vec<WireEvent>, AdapterError> {
    let mut events = Vec::new();
    let mut offset = 0usize;
    loop {
        match decode_event(&bytes[offset..])? {
            Some((event, size)) => {
                if events.len() >= limit {
                    return Err(AdapterError::Backend(BAD_BATCH));
                }
                events.push(event);
                offset += size;
            }
            None if offset == bytes.len() => return Ok(events),
            None => return Err(AdapterError::Backend(BAD_TRAILING)),
        }
    }
}

/// One SCM_RIGHTS message: the descriptor sits immediately after its header.
#[cfg(unix)]
#[repr(C)]
struct FdCmsg {
    header: libc::cmsghdr,
    fd: i32,
}

#[cfg(unix)]
fn fd_control_space() -> usize {
    let align = std::mem::size_of::<usize>();
    let raw = std::mem::size_of::<libc::cmsghdr>() + std::mem::size_of::<i32>();
    (raw + align - 1) & !(align - 1)
}

/// Send `bytes` on a Unix socket and pass `fd` in the same message's
/// SCM_RIGHTS ancillary data. Used for `wl_shm.create_pool`'s fd and, later,
/// `wl_keyboard.keymap`; without this the compositor receives a meaningless
/// number and the mapping is leaked.
#[cfg(unix)]
pub fn send_with_fd(socket: i32, bytes: &[u8], fd: i32) -> Result<(), AdapterError> {
    const BAD_FD_SEND: &str = "the file descriptor could not be sent";
    let iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    let mut control: FdCmsg = unsafe { std::mem::zeroed() };
    control.header.cmsg_len =
        (std::mem::size_of::<libc::cmsghdr>() + std::mem::size_of::<i32>()) as _;
    control.header.cmsg_level = libc::SOL_SOCKET;
    control.header.cmsg_type = libc::SCM_RIGHTS;
    control.fd = fd;
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &iov as *const libc::iovec as *mut libc::iovec;
    message.msg_iovlen = 1;
    message.msg_control = (&mut control as *mut FdCmsg).cast();
    message.msg_controllen = fd_control_space() as _;
    let sent = unsafe { libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) };
    if sent == bytes.len() as isize {
        Ok(())
    } else {
        Err(AdapterError::Backend(BAD_FD_SEND))
    }
}

/// Receive one SCM_RIGHTS descriptor alongside a message and hand back the
/// owned fd (the caller closes it exactly once).
#[cfg(unix)]
/// Receive one message and every SCM_RIGHTS descriptor it carries. The caller
/// owns the returned fds and must close each one exactly once.
#[cfg(unix)]
pub fn recvmsg_with_fds(socket: i32, bytes: &mut [u8]) -> Result<(usize, Vec<i32>), AdapterError> {
    const BAD_FD_RECV: &str = "the file descriptor could not be received";
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: bytes.len(),
    };
    // 256 bytes of aligned control space: several one-fd cmsgs fit, and any
    // descriptor that arrives here is owned by the caller.
    let mut control = [0u64; 32];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast::<libc::c_void>();
    message.msg_controllen = std::mem::size_of_val(&control) as _;
    let read = unsafe { libc::recvmsg(socket, &mut message, 0) };
    if read <= 0 {
        return Err(AdapterError::Backend(BAD_FD_RECV));
    }
    let header_size = std::mem::size_of::<libc::cmsghdr>();
    let start = message.msg_control as *const u8;
    let end = unsafe { start.add(message.msg_controllen as usize) };
    let mut cursor = start;
    let mut fds = Vec::new();
    while (cursor as usize) + header_size <= end as usize {
        let header = unsafe { &*(cursor as *const libc::cmsghdr) };
        let total = header.cmsg_len as usize;
        if total < header_size || (cursor as usize) + total > end as usize {
            break;
        }
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            let data = unsafe { cursor.add(header_size) };
            let count = (total - header_size) / std::mem::size_of::<i32>();
            for index in 0..count {
                fds.push(unsafe { *(data.add(index * 4) as *const i32) });
            }
        }
        let align = std::mem::size_of::<usize>();
        cursor = unsafe { cursor.add((total + align - 1) & !(align - 1)) };
    }
    Ok((read as usize, fds))
}

/// Receive one message carrying exactly one fd. Any other fd count is an
/// error, and every descriptor received is closed before returning it.
#[cfg(unix)]
pub fn recv_with_fd(socket: i32, bytes: &mut [u8]) -> Result<(usize, i32), AdapterError> {
    const BAD_FD_RECV: &str = "the file descriptor could not be received";
    let (read, fds) = recvmsg_with_fds(socket, bytes)?;
    if fds.len() == 1 {
        return Ok((read, fds[0]));
    }
    for fd in fds {
        unsafe { libc::close(fd) };
    }
    Err(AdapterError::Backend(BAD_FD_RECV))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_platforms, spec_by_name, wayland_skip_reason, Status, WAYLAND_DISPLAY_ENV};

    #[test]
    fn output_scale_decodes_the_factor_word() {
        assert_eq!(decode_output_scale(&2u32.to_ne_bytes()).unwrap(), 2);
        assert!(decode_output_scale(&[1, 2, 3]).is_err());
        assert!(decode_output_scale(&[0u8; 8]).is_err());
    }

    #[test]
    fn surface_output_decodes_enter_and_leave_words() {
        assert_eq!(WL_SURFACE_ENTER, 0);
        assert_eq!(WL_SURFACE_LEAVE, 1);
        let output = 0xff00_0003u32;
        assert_eq!(
            decode_surface_output(&output.to_ne_bytes()).unwrap(),
            output
        );
        assert_eq!(decode_surface_output(&0u32.to_ne_bytes()).unwrap(), 0);
        for bad in [&b""[..], &[1u8, 2, 3][..], &[0u8; 8][..]] {
            assert!(decode_surface_output(bad).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn scm_rights_carries_a_real_descriptor_with_the_message() {
        use std::os::fd::AsRawFd;
        let mut pair = [0i32; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) },
            0
        );
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        let message = b"fd-trip";
        send_with_fd(pair[0], message, file.as_raw_fd()).expect("send fd");
        let mut buffer = [0u8; 16];
        let (used, passed) = recv_with_fd(pair[1], &mut buffer).expect("recv fd");
        assert_eq!(&buffer[..used], message);
        assert!(passed >= 0, "SCM_RIGHTS must name a live descriptor");
        unsafe {
            libc::close(passed);
            libc::close(pair[0]);
            libc::close(pair[1]);
        }
    }

    #[test]
    fn seat_device_requests_carry_the_new_id() {
        let pointer = encode_seat_device(4, WL_SEAT_GET_POINTER, 5).unwrap();
        let parsed = decode_header(&pointer).unwrap();
        assert_eq!((parsed.object_id, parsed.opcode), (4, 0));
        assert_eq!(&pointer[8..], 5u32.to_ne_bytes());
        let keyboard = encode_seat_device(4, WL_SEAT_GET_KEYBOARD, 6).unwrap();
        assert_eq!(decode_header(&keyboard).unwrap().opcode, 1);
        assert_eq!(&keyboard[8..], 6u32.to_ne_bytes());
    }

    #[test]
    fn pointer_events_decode_word_for_word() {
        let mut body = Vec::new();
        for word in [9u32, 3, 256, 512] {
            body.extend_from_slice(&word.to_ne_bytes());
        }
        assert_eq!(decode_pointer_enter(&body).unwrap(), (9, 3, 256, 512));
        assert!(decode_pointer_enter(&body[..12]).is_err());

        let mut body = Vec::new();
        body.extend_from_slice(&9u32.to_ne_bytes());
        body.extend_from_slice(&3u32.to_ne_bytes());
        assert_eq!(decode_pointer_leave(&body).unwrap(), 3);
        assert_eq!(decode_pointer_motion(&body).unwrap(), (9i32, 3i32));
        assert!(decode_pointer_leave(&[0u8; 12]).is_err());

        let mut body = Vec::new();
        for word in [1u32, 2, WL_POINTER_PRIMARY, WL_BUTTON_PRESSED] {
            body.extend_from_slice(&word.to_ne_bytes());
        }
        assert_eq!(
            decode_pointer_button(&body).unwrap(),
            (WL_POINTER_PRIMARY, WL_BUTTON_PRESSED)
        );
        assert!(decode_pointer_button(&body[..12]).is_err());

        let mut body = Vec::new();
        body.extend_from_slice(&2u32.to_ne_bytes());
        body.extend_from_slice(&WL_POINTER_AXIS_VERTICAL.to_ne_bytes());
        body.extend_from_slice(&(-256i32).to_ne_bytes());
        assert_eq!(decode_pointer_axis(&body).unwrap(), (0, -256));
        assert_eq!(fixed_to_f32(256), 1.0);
        assert_eq!(fixed_to_f32(-128), -0.5);
    }

    fn keyboard_key_body() -> Vec<u8> {
        let mut body = Vec::new();
        for word in [7u32, 2, 48, WL_KEY_PRESSED] {
            body.extend_from_slice(&word.to_ne_bytes());
        }
        body
    }

    #[test]
    fn keyboard_events_decode_word_for_word() {
        let mut keymap = Vec::new();
        for word in [1u32, 9, 48] {
            keymap.extend_from_slice(&word.to_ne_bytes());
        }
        assert_eq!(decode_keyboard_keymap(&keymap).unwrap(), (1, 48));
        assert_eq!(decode_keyboard_key(&keyboard_key_body()).unwrap(), (48, 1));
        assert!(decode_keyboard_keymap(&keymap[..8]).is_err());

        let mut enter = Vec::new();
        enter.extend_from_slice(&7u32.to_ne_bytes());
        enter.extend_from_slice(&3u32.to_ne_bytes());
        enter.extend_from_slice(&0u32.to_ne_bytes()); // no pressed keys
        assert_eq!(decode_keyboard_enter(&enter).unwrap(), 3);
        enter.extend_from_slice(&[1]); // trailing padding breaks the length
        assert!(decode_keyboard_enter(&enter).is_err());

        let mut leave = Vec::new();
        leave.extend_from_slice(&7u32.to_ne_bytes());
        leave.extend_from_slice(&3u32.to_ne_bytes());
        assert_eq!(decode_keyboard_leave(&leave).unwrap(), 3);
        assert!(decode_keyboard_leave(&[0u8; 12]).is_err());
    }

    #[test]
    fn xdg_requests_and_configures_match_the_wire() {
        let surface = encode_create_surface(2, 3).unwrap();
        let parsed = decode_header(&surface).unwrap();
        assert_eq!(
            (parsed.object_id, parsed.opcode),
            (2, WL_COMPOSITOR_CREATE_SURFACE)
        );
        assert_eq!(&surface[8..], 3u32.to_ne_bytes());

        let xdg = encode_xdg_get_surface(4, 5, 3).unwrap();
        assert_eq!(&xdg[8..], [5u32.to_ne_bytes(), 3u32.to_ne_bytes()].concat());
        let top = encode_xdg_get_toplevel(5, 6).unwrap();
        assert_eq!(&top[8..], 6u32.to_ne_bytes());
        let title = encode_toplevel_string(6, XDG_TOPLEVEL_SET_TITLE, "opurom").unwrap();
        let (text, _) = decode_string(&title[8..]).unwrap();
        assert_eq!(text, "opurom");
        let ack = encode_ack_configure(5, 91).unwrap();
        assert_eq!(&ack[8..], 91u32.to_ne_bytes());

        let attach = encode_surface_attach(3, 7).unwrap();
        assert_eq!(attach.len(), 20);
        assert_eq!(&attach[8..12], 7u32.to_ne_bytes());
        let damage = encode_surface_damage(3, 4, 2).unwrap();
        assert_eq!(damage.len(), 24);
        assert_eq!(&damage[16..20], 4i32.to_ne_bytes());
        assert_eq!(&damage[20..24], 2i32.to_ne_bytes());
        assert_eq!(
            decode_header(&encode_surface_commit(3).unwrap())
                .unwrap()
                .opcode,
            WL_SURFACE_COMMIT
        );
    }

    #[test]
    fn xdg_configure_bodies_decode_without_guessing() {
        assert_eq!(decode_xdg_configure_serial(&7u32.to_ne_bytes()).unwrap(), 7);
        assert!(decode_xdg_configure_serial(&[]).is_err());
        let mut body = Vec::new();
        body.extend_from_slice(&320i32.to_ne_bytes());
        body.extend_from_slice(&200i32.to_ne_bytes());
        body.extend_from_slice(&0u32.to_ne_bytes()); // no states
        assert_eq!(decode_toplevel_configure(&body).unwrap(), (320, 200));
        body.extend_from_slice(&[4]); // trailing padding breaks the length
        assert!(decode_toplevel_configure(&body).is_err());
        assert!(decode_empty_event(&[0u8; 4]).is_err());
        assert_eq!(decode_empty_event(&[]), Ok(()));
    }

    /// Concrete wire bytes on both host byte orders. Wayland integers are
    /// native-endian; string bytes and padding bytes keep their order.
    fn native<'a>(little: &'a [u8], big: &'a [u8]) -> &'a [u8] {
        if cfg!(target_endian = "little") {
            little
        } else {
            big
        }
    }

    #[test]
    fn setup_handshake_bytes_are_the_two_display_requests() {
        let mut ids = ObjectIds::new();
        let setup = setup_handshake(&mut ids).unwrap();
        assert_eq!((setup.registry_id, setup.callback_id), (2, 3));
        // wl_display@1.get_registry(wl_registry@2): size=12, opcode=1.
        // wl_display@1.sync(wl_callback@3): size=12, opcode=0.
        assert_eq!(
            setup.bytes.as_slice(),
            native(
                &[
                    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x03, 0x00, 0x00, 0x00,
                ],
                &[
                    0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00,
                    0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03,
                ],
            )
        );
        assert_eq!(ids.allocate(), Ok(4), "the next client object is ID 4");
    }

    #[test]
    fn header_packs_size_and_opcode_and_pads_the_body() {
        assert_eq!(message_size(4), Ok(12));
        assert_eq!(message_size(3), Ok(12));
        assert_eq!(message_size(24), Ok(32));
        // The 16-bit size field covers the header, so the largest body is
        // 65535 - 8 = 65527 bytes; the body must also be word aligned.
        assert_eq!(message_size(65524), Ok(65532));
        assert!(message_size(65525).is_err(), "size must fit in 16 bits");
        assert!(message_size(65529).is_err(), "size must fit in 16 bits");
        assert_eq!(
            header(0x0102_0304, 0x0102, 0x0020),
            native(
                &[0x04, 0x03, 0x02, 0x01, 0x02, 0x01, 0x20, 0x00],
                &[0x01, 0x02, 0x03, 0x04, 0x00, 0x20, 0x01, 0x02],
            ),
        );
        // A 3-byte body is padded with one zero byte to a 12-byte message.
        assert_eq!(
            encode_message(9, 7, &[0xaa, 0xbb, 0xcc]).unwrap(),
            native(
                &[0x09, 0, 0, 0, 0x07, 0x00, 0x0c, 0x00, 0xaa, 0xbb, 0xcc, 0x00],
                &[0, 0, 0, 0x09, 0x00, 0x0c, 0x00, 0x07, 0xaa, 0xbb, 0xcc, 0x00],
            ),
        );
    }

    #[test]
    fn strings_are_length_prefixed_nul_terminated_and_word_padded() {
        assert_eq!(
            encode_string("wl_shm").unwrap(),
            native(
                &[0x07, 0, 0, 0, 0x77, 0x6c, 0x5f, 0x73, 0x68, 0x6d, 0x00, 0x00],
                &[0, 0, 0, 0x07, 0x77, 0x6c, 0x5f, 0x73, 0x68, 0x6d, 0x00, 0x00],
            ),
        );
        // "wl_compositor\0" is 14 bytes, so two zero pad bytes follow it.
        assert_eq!(
            encode_string("wl_compositor").unwrap(),
            native(
                &[
                    0x0e, 0, 0, 0, 0x77, 0x6c, 0x5f, 0x63, 0x6f, 0x6d, 0x70, 0x6f, 0x73, 0x69,
                    0x74, 0x6f, 0x72, 0x00, 0x00, 0x00,
                ],
                &[
                    0, 0, 0, 0x0e, 0x77, 0x6c, 0x5f, 0x63, 0x6f, 0x6d, 0x70, 0x6f, 0x73, 0x69,
                    0x74, 0x6f, 0x72, 0x00, 0x00, 0x00,
                ],
            ),
        );
        assert!(encode_string("wl\0shm").is_err());
        assert!(decode_string(&encode_string("wl_shm").unwrap()).unwrap() == ("wl_shm", 12));
        assert!(decode_string(&[0, 0, 0, 0]).is_err(), "length 0 is invalid");
    }

    #[test]
    fn registry_bind_carries_the_interface_name_and_version() {
        // wl_registry@2.bind(name=7, "wl_shm", version=1, wl_shm@4): size=32.
        let message = encode_registry_bind(2, 7, "wl_shm", 1, 4).unwrap();
        assert_eq!(
            message.as_slice(),
            native(
                &[
                    0x02, 0, 0, 0, 0x00, 0x00, 0x20, 0x00, // header: size 32, opcode 0
                    0x07, 0, 0, 0, // name
                    0x07, 0, 0, 0, // string length incl. NUL
                    0x77, 0x6c, 0x5f, 0x73, 0x68, 0x6d, 0x00, 0x00, // "wl_shm" + pad
                    0x01, 0, 0, 0, // version
                    0x04, 0, 0, 0, // new_id
                ],
                &[
                    0, 0, 0, 0x02, 0x00, 0x20, 0x00, 0x00, 0, 0, 0, 0x07, 0, 0, 0, 0x07, 0x77,
                    0x6c, 0x5f, 0x73, 0x68, 0x6d, 0x00, 0x00, 0, 0, 0, 0x01, 0, 0, 0, 0x04,
                ],
            ),
        );
        // A longer interface name grows the padded string and hence the size.
        assert_eq!(
            encode_registry_bind(2, 7, "wl_compositor", 5, 4)
                .unwrap()
                .len(),
            40
        );
    }

    #[test]
    fn shm_pool_and_buffer_messages_match_the_wire_bytes() {
        // wl_shm@3.create_pool(wl_shm_pool@4, fd=8, size=24): size=20. The
        // descriptor also travels as SCM_RIGHTS ancillary data.
        assert_eq!(
            encode_create_pool(3, 4, 8, 24).unwrap(),
            native(
                &[
                    0x03, 0, 0, 0, 0x00, 0x00, 0x14, 0x00, 0x04, 0, 0, 0, 0x08, 0, 0, 0, 0x18, 0,
                    0, 0,
                ],
                &[
                    0, 0, 0, 0x03, 0x00, 0x14, 0x00, 0x00, 0, 0, 0, 0x04, 0, 0, 0, 0x08, 0, 0, 0,
                    0x18,
                ],
            ),
        );
        // wl_shm_pool@4.create_buffer(wl_buffer@5, offset=0, 2x3, stride=8,
        // ARGB8888): six 32-bit arguments, size=32.
        assert_eq!(
            encode_create_buffer(4, 5, 0, 2, 3, 8, WL_SHM_FORMAT_ARGB8888).unwrap(),
            native(
                &[
                    0x04, 0, 0, 0, 0x00, 0x00, 0x20, 0x00, // header: size 32, opcode 0
                    0x05, 0, 0, 0, // new_id
                    0x00, 0, 0, 0, // offset
                    0x02, 0, 0, 0, // width
                    0x03, 0, 0, 0, // height
                    0x08, 0, 0, 0, // stride
                    0x00, 0, 0, 0, // format ARGB8888 = 0
                ],
                &[
                    0, 0, 0, 0x04, 0x00, 0x20, 0x00, 0x00, 0, 0, 0, 0x05, 0, 0, 0, 0x00, 0, 0, 0,
                    0x02, 0, 0, 0, 0x03, 0, 0, 0, 0x08, 0, 0, 0, 0x00,
                ],
            ),
        );
    }

    #[test]
    fn shm_stride_and_pool_size_enforce_the_buffer_contract() {
        assert_eq!(WL_SHM_FORMAT_ARGB8888, 0);
        assert_eq!(WL_SHM_FORMAT_XRGB8888, 1);
        assert_eq!(shm_bytes_per_pixel(WL_SHM_FORMAT_ARGB8888), Some(4));
        assert_eq!(shm_bytes_per_pixel(WL_SHM_FORMAT_XRGB8888), Some(4));
        assert_eq!(shm_bytes_per_pixel(0x34325241), None, "unknown fourcc");
        assert_eq!(shm_stride(2, WL_SHM_FORMAT_ARGB8888), Ok(8));
        assert_eq!(shm_stride(0, WL_SHM_FORMAT_ARGB8888), Ok(0));
        assert_eq!(shm_pool_size(0, 8, 3), Ok(24));
        assert_eq!(shm_pool_size(16, 8, 3), Ok(40));
        assert!(shm_pool_size(-1, 8, 3).is_err());
        assert!(shm_stride(u32::MAX, WL_SHM_FORMAT_ARGB8888).is_err());
        assert!(encode_create_buffer(4, 5, 0, 2, 3, 7, WL_SHM_FORMAT_ARGB8888).is_err());
        assert!(encode_create_buffer(4, 5, 0, 2, 3, 6, WL_SHM_FORMAT_ARGB8888).is_err());
        assert!(encode_create_buffer(4, 5, -1, 2, 3, 8, WL_SHM_FORMAT_ARGB8888).is_err());
        assert!(encode_create_buffer(4, 5, 0, 2, 3, 8, 0x34325241).is_err());
        // A wider stride is legal as long as it stays word aligned and covered.
        assert_eq!(
            encode_create_buffer(4, 5, 0, 2, 3, 12, WL_SHM_FORMAT_XRGB8888)
                .unwrap()
                .len(),
            32
        );
    }

    #[test]
    fn registry_global_and_callback_events_decode_from_wire_bytes() {
        // wl_registry@2.global(name=1, "wl_shm", version=1): size=28, opcode=0.
        let global = [
            0x02, 0, 0, 0, 0x00, 0x00, 0x1c, 0x00, 0x01, 0, 0, 0, 0x07, 0, 0, 0, 0x77, 0x6c, 0x5f,
            0x73, 0x68, 0x6d, 0x00, 0x00, 0x01, 0, 0, 0,
        ];
        let global_be = [
            0, 0, 0, 0x02, 0x00, 0x1c, 0x00, 0x00, 0, 0, 0, 0x01, 0, 0, 0, 0x07, 0x77, 0x6c, 0x5f,
            0x73, 0x68, 0x6d, 0x00, 0x00, 0, 0, 0, 0x01,
        ];
        let bytes = native(&global, &global_be);
        let (parsed, body, size) = next_event(bytes).unwrap().unwrap();
        assert_eq!(
            parsed,
            MessageHeader {
                object_id: 2,
                opcode: WL_REGISTRY_GLOBAL,
                size: 28,
            }
        );
        assert_eq!(size, bytes.len());
        let name = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]);
        assert_eq!(name, 1);
        let (interface, used) = decode_string(&body[4..]).unwrap();
        assert_eq!(interface, "wl_shm");
        assert_eq!(used, 12);
        let version = u32::from_ne_bytes([body[16], body[17], body[18], body[19]]);
        assert_eq!(version, 1);

        // wl_callback@3.done(serial): size=12, opcode=0.
        let done = [
            0x03, 0, 0, 0, 0x00, 0x00, 0x0c, 0x00, 0x44, 0x33, 0x22, 0x11,
        ];
        let done_be = [
            0, 0, 0, 0x03, 0x00, 0x0c, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44,
        ];
        let done_bytes = native(&done, &done_be);
        let (parsed, body, size) = next_event(done_bytes).unwrap().unwrap();
        assert_eq!(parsed.opcode, WL_CALLBACK_DONE);
        assert_eq!(parsed.object_id, 3);
        assert_eq!(size, 12);
        assert_eq!(
            u32::from_ne_bytes([body[0], body[1], body[2], body[3]]),
            0x1122_3344
        );

        assert!(next_event(&bytes[..7]).unwrap().is_none(), "partial header");
        assert!(
            next_event(&bytes[..20]).unwrap().is_none(),
            "partial message"
        );
        assert!(decode_header(&[0; 8]).is_err(), "size below the header");
    }

    #[test]
    fn object_ids_stop_before_the_server_range() {
        let mut ids = ObjectIds::new();
        assert_eq!(ids.allocate(), Ok(2));
        assert_eq!(ids.allocate(), Ok(3));
        ids.next = SERVER_ID_START - 1;
        assert_eq!(ids.allocate(), Ok(SERVER_ID_START - 1));
        assert_eq!(ids.allocate(), Err(AdapterError::Backend(IDS_EXHAUSTED)));
        assert_eq!(SERVER_ID_START, 0xff00_0000);
    }

    #[test]
    fn every_encoded_message_satisfies_the_protocol_invariants() {
        // The protocol's own rules: the size field counts the header and the
        // padding, the whole message is word aligned, and the opcode never
        // escapes its 16 bits.
        let mut ids = ObjectIds::new();
        let setup = setup_handshake(&mut ids).unwrap();
        // The handshake is two messages; split them apart so each one is
        // checked on its own.
        let (first, second) = setup.bytes.split_at(12);
        let messages = [
            first.to_vec(),
            second.to_vec(),
            encode_registry_bind(2, 7, "wl_shm", 1, 4).unwrap(),
            encode_registry_bind(2, 9, "wl_output", 4, 6).unwrap(),
            encode_create_pool(3, 4, 8, 24).unwrap(),
            encode_create_buffer(4, 5, 0, 2, 3, 8, WL_SHM_FORMAT_ARGB8888).unwrap(),
            encode_graphics_backend_event(GraphicsBackendCapabilities::software_default()).unwrap(),
        ];
        for message in &messages {
            assert_eq!(message.len() % 4, 0, "a message is word aligned");
            let header = decode_header(message).unwrap();
            assert_eq!(
                usize::from(header.size),
                message.len(),
                "the size field counts the header and the padding"
            );
            assert!(usize::from(header.size) >= HEADER_LEN);
            // The opcode lives in the lower 16 bits of the size word.
            let raw = u32::from_ne_bytes([message[4], message[5], message[6], message[7]]);
            assert_eq!(raw >> 16, u32::from(header.size));
            assert_eq!(raw & 0xffff, u32::from(header.opcode));
        }
    }

    #[test]
    fn a_missing_display_is_a_concrete_skip_reason() {
        assert_eq!(WAYLAND_DISPLAY_ENV, "WAYLAND_DISPLAY");
        assert_eq!(
            wayland_skip_reason(None),
            Some("WAYLAND_DISPLAY is not set")
        );
        assert_eq!(
            wayland_skip_reason(Some("")),
            Some("WAYLAND_DISPLAY is empty")
        );
        assert_eq!(wayland_skip_reason(Some("wayland-0")), None);
    }

    #[test]
    fn wayland_is_planned_until_a_live_compositor_is_available() {
        let contract = spec_by_name("wayland").expect("the Wayland host has a registry row");
        assert_eq!(contract.name, "wayland");
        assert_eq!(contract.status, Status::Planned);
        assert!(!contract.capabilities.gpu);
        assert!(
            !host_platforms().iter().any(|host| host.name == "wayland"),
            "a Planned host is not a Verified host"
        );
    }

    /// Fixed tables only: the same cases run on every host, with no randomness.
    const RESIZE_WIDTHS: [u32; 5] = [0, 1, 320, 1600, 4096];
    const COORDS: [i32; 7] = [
        i32::MIN,
        i16::MIN as i32,
        -1,
        0,
        1,
        i16::MAX as i32,
        i32::MAX,
    ];

    fn event_cases() -> Vec<WireEvent> {
        let mut cases = Vec::new();
        for (index, &width) in RESIZE_WIDTHS.iter().enumerate() {
            // Pair each width with the next one so a width/height swap fails.
            cases.push(WireEvent::Resize {
                width,
                height: RESIZE_WIDTHS[(index + 1) % RESIZE_WIDTHS.len()],
            });
        }
        for (index, &x) in COORDS.iter().enumerate() {
            let y = COORDS[(index + 1) % COORDS.len()];
            cases.push(WireEvent::Press { x, y });
            cases.push(WireEvent::Release { x, y });
            cases.push(WireEvent::Move { x, y });
        }
        cases.push(WireEvent::Focus { focused: false });
        cases.push(WireEvent::Focus { focused: true });
        cases
    }

    fn encode_cases(cases: &[WireEvent]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &event in cases {
            bytes.extend_from_slice(&event.encode().unwrap());
        }
        bytes
    }

    #[test]
    fn resize_and_input_events_round_trip_over_fixed_tables() {
        for &event in &event_cases() {
            let bytes = event.encode().unwrap();
            assert_eq!(bytes.len(), 16, "two words plus the 8-byte header");
            let (decoded, used) = decode_event(&bytes).unwrap().unwrap();
            assert_eq!(decoded, event, "{event:?} did not round-trip");
            assert_eq!(used, bytes.len(), "the whole message is consumed");
        }
        // One concrete layout pins the native-endian word order.
        assert_eq!(
            WireEvent::Resize {
                width: 320,
                height: 1600,
            }
            .encode()
            .unwrap(),
            native(
                &[
                    0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x10, 0x00, 0x40, 0x01, 0x00, 0x00, 0x40,
                    0x06, 0x00, 0x00,
                ],
                &[
                    0xff, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x01, 0x40, 0x00,
                    0x00, 0x06, 0x40,
                ],
            ),
        );
        // Focus is the same two-word framing with a one-word flag.
        assert_eq!(
            WireEvent::Focus { focused: true }.encode().unwrap(),
            native(
                &[
                    0x00, 0x00, 0x00, 0xff, 0x04, 0x00, 0x10, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
                    0x00, 0x00, 0x00,
                ],
                &[
                    0xff, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x00,
                    0x00, 0x00, 0x00,
                ],
            ),
        );
    }

    #[test]
    fn multi_event_batches_round_trip_and_bound_the_decoder() {
        let cases = event_cases();
        let bytes = encode_cases(&cases);
        assert_eq!(bytes.len(), cases.len() * 16);
        assert_eq!(decode_batch(&bytes, cases.len()), Ok(cases.clone()));
        assert_eq!(decode_batch(&[], 0), Ok(Vec::new()));
        // One event over the limit is rejected with a named, clear error.
        let err = decode_batch(&bytes, cases.len() - 1).unwrap_err();
        assert_eq!(err, AdapterError::Backend(BAD_BATCH));
        assert!(
            err.to_string()
                .contains("event batch exceeds the event limit"),
            "{err} is not a clear oversized-batch error"
        );
        assert_eq!(
            decode_batch(&bytes, 0),
            Err(AdapterError::Backend(BAD_BATCH))
        );
    }

    #[test]
    fn truncated_buffers_never_yield_a_partial_event() {
        let cases = event_cases();
        let bytes = encode_cases(&cases);
        for cut in 0..bytes.len() {
            let prefix = &bytes[..cut];
            match decode_event(prefix) {
                Ok(Some((event, used))) => {
                    assert!(cut >= 16, "the first message must be complete");
                    assert_eq!(event, cases[0]);
                    assert_eq!(used, 16);
                }
                Ok(None) => assert!(cut < 16, "a complete first message must decode"),
                Err(_) => {}
            }
            if cut % 16 == 0 {
                assert_eq!(
                    decode_batch(prefix, cases.len()),
                    Ok(cases[..cut / 16].to_vec()),
                    "a batch cut on a message boundary still round-trips"
                );
            } else {
                assert_eq!(
                    decode_batch(prefix, cases.len()),
                    Err(AdapterError::Backend(BAD_TRAILING)),
                    "a trailing partial message is rejected"
                );
            }
        }
        // Truncated string arguments are rejected too, never indexed past end.
        let string = encode_string("wl_compositor").unwrap();
        for cut in 0..string.len() {
            assert!(decode_string(&string[..cut]).is_err(), "cut {cut}");
        }
    }

    #[test]
    fn malformed_event_headers_and_bodies_are_concrete_errors() {
        let good = WireEvent::Move { x: -1, y: 1 }.encode().unwrap();
        let broken = [
            header(EVENT_OBJECT_ID, EVENT_RESIZE, 0), // size below the header
            header(EVENT_OBJECT_ID, EVENT_RESIZE, 9), // not word aligned
            header(DISPLAY_ID, EVENT_RESIZE, 16),     // wrong object
            header(EVENT_OBJECT_ID, 99, 16),          // unknown opcode
        ];
        for head in broken {
            let mut bytes = good.clone();
            bytes[..HEADER_LEN].copy_from_slice(&head);
            assert!(decode_event(&bytes).is_err(), "{head:?} is rejected");
            assert!(decode_batch(&bytes, 8).is_err());
        }
        // A legal size that cuts the two-word body short.
        let mut short = good.clone();
        short[..HEADER_LEN].copy_from_slice(&header(EVENT_OBJECT_ID, EVENT_MOVE, 12));
        assert_eq!(
            decode_event(&short),
            Err(AdapterError::Backend(BAD_EVENT_BODY))
        );
        // The focus flag is either zero or one; another value is malformed.
        let mut invalid_focus = WireEvent::Focus { focused: true }.encode().unwrap();
        invalid_focus[8] = 2;
        assert_eq!(
            decode_event(&invalid_focus),
            Err(AdapterError::Backend(BAD_FOCUS))
        );
        // A claimed size beyond the available bytes is a partial stream.
        let mut long = good.clone();
        long[..HEADER_LEN].copy_from_slice(&header(EVENT_OBJECT_ID, EVENT_MOVE, 20));
        assert_eq!(decode_event(&long), Ok(None));
        assert_eq!(
            decode_batch(&long, 8),
            Err(AdapterError::Backend(BAD_TRAILING))
        );
    }
}
