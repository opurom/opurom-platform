//! Pure Wayland wire-format encoding (transport additions, section 4).
//!
//! Kept outside the Linux-gated `wayland` module so the encodings are verified
//! by `cargo test` on every host, including macOS workstations without Wayland.
//! The Wayland wire protocol uses native-endian 32-bit words: a message starts
//! with the object ID, then a word whose upper 16 bits are the total message
//! size (header included) and whose lower 16 bits are the opcode. Strings carry
//! a length word, their NUL-terminated bytes and zero padding to a word boundary.

use crate::AdapterError;

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

/// `wl_shm.format`: 32-bit native-endian A:R:G:B 8:8:8:8, four bytes per pixel.
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
pub fn encode_create_pool(shm_id: u32, pool_id: u32, size: i32) -> Result<Vec<u8>, AdapterError> {
    let mut body = pool_id.to_ne_bytes().to_vec();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_platforms, spec_by_name, wayland_skip_reason, Status, WAYLAND_DISPLAY_ENV};

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
        // wl_shm@3.create_pool(wl_shm_pool@4, fd, size=24): size=16. The fd is
        // passed through SCM_RIGHTS, so it consumes no bytes.
        assert_eq!(
            encode_create_pool(3, 4, 24).unwrap(),
            native(
                &[0x03, 0, 0, 0, 0x00, 0x00, 0x10, 0x00, 0x04, 0, 0, 0, 0x18, 0, 0, 0,],
                &[0, 0, 0, 0x03, 0x00, 0x10, 0x00, 0x00, 0, 0, 0, 0x04, 0, 0, 0, 0x18,],
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
            encode_create_pool(3, 4, 24).unwrap(),
            encode_create_buffer(4, 5, 0, 2, 3, 8, WL_SHM_FORMAT_ARGB8888).unwrap(),
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
        let spec = spec_by_name("wayland").expect("the Wayland host has a registry row");
        assert_eq!(spec.name, "wayland");
        assert_eq!(spec.status, Status::Planned);
        assert!(!spec.capabilities.gpu);
        assert!(
            !host_platforms().iter().any(|host| host.name == "wayland"),
            "a Planned host is not a Verified host"
        );
    }
}
