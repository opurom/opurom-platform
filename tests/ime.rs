//! Tests for the IME text-input half: UTF-16 assembly, composition flags and
//! the Wayland text-input request/event framing — the layer every host tests
//! headlessly, without a window server.

use opurom_platform::ime::{
    encode_enable_commit, encode_get_text_input, ime_result, utf16_push, ImeStatus, TextComposer,
};
use opurom_platform::wayland_wire::{decode_header, next_event};
use opurom_platform::AdapterError;

// ---------------------------------------------------------------- win32

#[test]
fn utf16_units_assemble_into_scalars() {
    let mut pending = None;
    assert_eq!(utf16_push(&mut pending, 'a' as u16).as_deref(), Some("a"));
    assert!(pending.is_none());
}

#[test]
fn an_astral_character_arrives_as_one_input() {
    let mut pending = None;
    assert_eq!(utf16_push(&mut pending, 0xd83d), None, "high half waits");
    assert_eq!(pending, Some(0xd83d));
    let text = utf16_push(&mut pending, 0xde00).expect("low half completes");
    assert_eq!(pending, None);
    assert_eq!(text, "\u{1f600}");
}

#[test]
fn broken_surrogate_streams_never_panic() {
    // A lone low surrogate has no scalar: it degrades to U+FFFD.
    let mut pending = None;
    assert_eq!(
        utf16_push(&mut pending, 0xdc00).as_deref(),
        Some("\u{fffd}")
    );
    // A dangling high surrogate followed by plain text drops the half.
    let mut pending = Some(0xd83d);
    assert_eq!(utf16_push(&mut pending, 'b' as u16).as_deref(), Some("b"));
    assert!(pending.is_none());
    // A new high surrogate replaces a dangling one instead of stacking.
    let mut pending = Some(0xd800);
    assert_eq!(utf16_push(&mut pending, 0xd83d), None);
    assert_eq!(pending, Some(0xd83d));
}

#[test]
fn only_the_result_flag_means_committed_text() {
    assert!(ime_result(0x0800));
    assert!(ime_result(0x0800 | 0x0001));
    assert!(!ime_result(0));
    assert!(!ime_result(0x0001));
}

// ---------------------------------------------------------------- wayland

#[test]
fn get_text_input_carries_the_new_id_and_the_seat() {
    let bytes = encode_get_text_input(9, 4, 3).expect("encode");
    let parsed = decode_header(&bytes).expect("header");
    assert_eq!(parsed.object_id, 9);
    assert_eq!(parsed.opcode, 1);
    assert_eq!(usize::from(parsed.size), bytes.len());
    let body = &bytes[8..];
    let mut expected = Vec::new();
    expected.extend_from_slice(&4u32.to_ne_bytes());
    expected.extend_from_slice(&3u32.to_ne_bytes());
    assert_eq!(body, expected);
}

#[test]
fn enable_and_commit_travel_as_two_messages() {
    let bytes = encode_enable_commit(7).expect("encode");
    let mut frames = Vec::new();
    let mut offset = 0usize;
    while let Some((parsed, _, size)) = next_event(&bytes[offset..]).expect("frame") {
        frames.push((parsed.object_id, parsed.opcode));
        offset += size;
    }
    assert_eq!(offset, bytes.len(), "no trailing bytes");
    assert_eq!(frames, vec![(7, 1), (7, 7)]);
}

#[test]
fn committed_text_applies_at_done() {
    let mut composer = TextComposer::default();
    // commit_string "ok": a word-aligned length word, then the NUL-terminated bytes.
    let mut body = 3u32.to_ne_bytes().to_vec();
    body.extend_from_slice(b"ok\0\0");
    assert_eq!(composer.feed(3, &body).expect("commit_string"), None);
    // preedit and delete_surrounding_text frames do not disturb the commit.
    assert_eq!(composer.feed(2, &body).expect("preedit"), None);
    assert_eq!(composer.feed(4, &[0u8; 8]).expect("delete"), None);
    assert_eq!(
        composer.feed(5, &1u32.to_ne_bytes()).expect("done"),
        Some("ok".to_string())
    );
    // The buffer resets: a second done with nothing pending delivers nothing.
    assert_eq!(composer.feed(5, &2u32.to_ne_bytes()).expect("done"), None);
}

#[test]
fn the_null_commit_string_commits_nothing() {
    let mut composer = TextComposer::default();
    assert_eq!(composer.feed(3, &0u32.to_ne_bytes()).expect("null"), None);
    assert_eq!(composer.feed(5, &0u32.to_ne_bytes()).expect("done"), None);
}

#[test]
fn malformed_text_input_frames_are_typed_errors() {
    let mut composer = TextComposer::default();
    // A commit body shorter than its length word.
    assert!(matches!(
        composer.feed(3, &[2, 0, 0]),
        Err(AdapterError::Backend(_))
    ));
    // A commit body with trailing bytes past its string.
    let mut body = 3u32.to_ne_bytes().to_vec();
    body.extend_from_slice(b"ok\0\0");
    body.push(0);
    assert!(matches!(
        composer.feed(3, &body),
        Err(AdapterError::Backend(_))
    ));
    // A done frame without its serial word.
    assert!(matches!(
        composer.feed(5, &[]),
        Err(AdapterError::Backend(_))
    ));
    // Unknown opcodes are ignored, never fabricated into input.
    assert_eq!(
        composer.feed(6, &1u32.to_ne_bytes()).expect("v2 action"),
        None
    );
}

// ---------------------------------------------------------------- status

#[test]
fn the_status_reports_its_reason_without_faking_readiness() {
    assert_eq!(ImeStatus::Ready.to_string(), "ime ready");
    let status = ImeStatus::Unavailable("no manager global");
    assert_eq!(status.to_string(), "ime unavailable: no manager global");
}
