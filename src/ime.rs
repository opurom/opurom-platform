//! The IME text-input half: per-window readiness reporting and the pure rules
//! that turn platform text events into the existing `InputEvent::Input`
//! channel. The Win32 UTF-16 assembly, the composition flags and the Wayland
//! text-input request/event framing are protocol-faithful and testable on
//! every host, while each adapter only wires them to its window.

use std::fmt;

use crate::wayland_wire::{decode_string, encode_message};
use crate::AdapterError;

/// Whether this window can compose text through the operating system's input
/// method. Probed per window or connection, never assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeStatus {
    /// The input-method objects are live; composed text arrives as
    /// `InputEvent::Input`.
    Ready,
    /// The platform or environment offers no input method here.
    Unavailable(&'static str),
}

impl fmt::Display for ImeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImeStatus::Ready => f.write_str("ime ready"),
            ImeStatus::Unavailable(why) => write!(f, "ime unavailable: {why}"),
        }
    }
}

/// Win32 `WM_IME_COMPOSITION` carries its flags in `lParam`; only this bit
/// means committed text (the same value the platform calls GCS_RESULTSTR,
/// verified against its own binding table).
pub const RESULT_STRING: u32 = 0x0800;

pub fn ime_result(lparam: u32) -> bool {
    lparam & RESULT_STRING != 0
}

/// Win32 `WM_CHAR` delivers one UTF-16 unit per message: assemble surrogate
/// pairs so an astral-plane character arrives as one `Input`, and map a unit
/// that cannot form a scalar to U+FFFD instead of panicking. Returns the
/// finished text, or `None` while a high surrogate is pending.
///
/// shortcut: a high surrogate that is not followed by a low one is dropped,
/// when the platform batches units, emit both halves.
pub fn utf16_push(pending: &mut Option<u16>, unit: u16) -> Option<String> {
    let is_high = |value: u16| (0xd800..0xdc00).contains(&value);
    let scalar: u32 = match pending.take() {
        Some(high) if is_high(high) && (0xdc00..0xe000).contains(&unit) => {
            ((u32::from(high) - 0xd800) << 10) + (u32::from(unit) - 0xdc00) + 0x10000
        }
        Some(_) if is_high(unit) => {
            *pending = Some(unit);
            return None;
        }
        Some(_) => u32::from(unit),
        None if is_high(unit) => {
            *pending = Some(unit);
            return None;
        }
        None => u32::from(unit),
    };
    Some(char::from_u32(scalar).unwrap_or('\u{FFFD}').to_string())
}

/// `zwp_text_input_manager_v3.get_text_input(new_id, seat)` — the only
/// non-destructor request the manager carries.
pub fn encode_get_text_input(
    manager_id: u32,
    text_id: u32,
    seat_id: u32,
) -> Result<Vec<u8>, AdapterError> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&text_id.to_ne_bytes());
    body.extend_from_slice(&seat_id.to_ne_bytes());
    encode_message(manager_id, TEXT_INPUT_GET, &body)
}

/// `enable` immediately followed by `commit`: state only applies on commit,
/// so the pair is what arms text delivery.
pub fn encode_enable_commit(text_id: u32) -> Result<Vec<u8>, AdapterError> {
    let enable = encode_message(text_id, TEXT_INPUT_ENABLE, &[])?;
    let commit = encode_message(text_id, TEXT_INPUT_COMMIT, &[])?;
    let mut bytes = enable;
    bytes.extend(commit);
    Ok(bytes)
}

/// Double-buffered text arriving on a `zwp_text_input_v3` object:
/// `commit_string` buffers and `done` applies it, per the protocol's ordering.
///
/// shortcut: preedit previews, surrounding-text deletions and the v2
/// action/hint events have no ABI channel yet and are dropped; an ABI
/// composing-text channel is when to carry them.
#[derive(Default)]
pub struct TextComposer {
    commit: Option<String>,
}

impl TextComposer {
    /// Feed one frame of the text-input object. Returns the committed text
    /// exactly when this frame completes one (that is, `done`).
    pub fn feed(&mut self, opcode: u16, body: &[u8]) -> Result<Option<String>, AdapterError> {
        const BAD: &str = "zwp_text_input_v3 event body does not match its opcode";
        match opcode {
            TEXT_INPUT_COMMIT_STRING => {
                // allow-null: the protocol encodes the null string as a zero
                // length, meaning "nothing to commit".
                if body.len() < 4 {
                    return Err(AdapterError::Backend(BAD));
                }
                let len = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]) as usize;
                if len == 0 {
                    self.commit = None;
                } else {
                    let (text, used) = decode_string(body)?;
                    if used != body.len() {
                        return Err(AdapterError::Backend(BAD));
                    }
                    self.commit = Some(text.to_string());
                }
                Ok(None)
            }
            TEXT_INPUT_DONE => {
                // The serial word is ignored: the host tracks no pending state
                // requests, so every done applies the same way.
                if body.len() != 4 {
                    return Err(AdapterError::Backend(BAD));
                }
                Ok(self.commit.take().filter(|text| !text.is_empty()))
            }
            // enter/leave/preedit/delete and the v2 action, language and hint
            // events: recognized, but nothing the ABI carries yet.
            _ => Ok(None),
        }
    }
}

// `zwp_text_input_manager_v3`: destroy, get_text_input.
const TEXT_INPUT_GET: u16 = 1;
// `zwp_text_input_v3` requests: destroy, enable, disable, set_surrounding_text,
// set_text_change_cause, set_content_type, set_cursor_rectangle, commit.
const TEXT_INPUT_ENABLE: u16 = 1;
const TEXT_INPUT_COMMIT: u16 = 7;
// Events: enter, leave, preedit_string, commit_string, delete_surrounding_text,
// done, then the v2 additions.
const TEXT_INPUT_COMMIT_STRING: u16 = 3;
const TEXT_INPUT_DONE: u16 = 5;
