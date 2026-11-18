//! Conformance tests for the Flutter-compatible channel contract.
//!
//! Every expected byte sequence is derived from the frozen Flutter 3.50
//! standard codec (`packages/flutter/lib/src/services/message_codecs.dart:267-347`),
//! not from an Opurom-invented format. Endian-sensitive expectations use
//! `to_ne_bytes()` because Flutter deliberately uses host endianness.

use std::collections::BTreeMap;

use opurom_platform::platform_channel::{
    decode_envelope, decode_message, decode_method_call, encode_envelope, encode_message,
    encode_method_call, error_codes, ChannelBuffer, ChannelKind, ChannelSpec, CodecError, Handler,
    MethodCall, Reply, Value, EVENT_CANCEL, EVENT_LISTEN,
};

fn ne_i32(n: i32) -> Vec<u8> {
    n.to_ne_bytes().to_vec()
}

fn ne_i64(n: i64) -> Vec<u8> {
    n.to_ne_bytes().to_vec()
}

fn ne_f64(n: f64) -> Vec<u8> {
    n.to_ne_bytes().to_vec()
}

#[test]
fn null_message_is_absent_not_zero_bytes() {
    assert_eq!(encode_message(&Value::Null).unwrap(), None);
    assert_eq!(decode_message(None).unwrap(), Value::Null);
}

#[test]
fn scalar_tags_match_flutter_wire_format() {
    // message_codecs.dart:310-324,386-408
    let cases: Vec<(Value, Vec<u8>)> = vec![
        (Value::Bool(true), vec![1]),
        (Value::Bool(false), vec![2]),
        (Value::I32(0), [vec![3], ne_i32(0)].concat()),
        (Value::I32(-1), [vec![3], ne_i32(-1)].concat()),
        (Value::I32(i32::MAX), [vec![3], ne_i32(i32::MAX)].concat()),
        (
            Value::I64(i32::MAX as i64 + 1),
            [vec![4], ne_i64(i32::MAX as i64 + 1)].concat(),
        ),
        (Value::I64(i64::MIN), [vec![4], ne_i64(i64::MIN)].concat()),
    ];
    for (value, expected) in cases {
        assert_eq!(
            encode_message(&value).unwrap(),
            Some(expected.clone()),
            "{value:?}"
        );
        assert_eq!(decode_message(Some(&expected)).unwrap(), value);
    }
}

#[test]
fn double_is_aligned_to_absolute_position() {
    // message_codecs.dart:289-290: zero bytes precede a double until the
    // absolute buffer position is 8-byte aligned. Here tag(1)+i32(4)=5, so
    // three zero bytes are required before the 8-byte IEEE-754 payload.
    let value = Value::List(vec![Value::I32(7), Value::F64(1.5)]);
    let bytes = encode_message(&value).unwrap().unwrap();
    // List header (tag 12, size 2) = 2 bytes, then tag 3 + i32 = 5 bytes,
    // then tag 6 at position 7; the writer pads one zero byte to place the
    // double payload at the 8-byte boundary (message_codecs.dart:396).
    let mut expected = vec![12, 2, 3];
    expected.extend_from_slice(&ne_i32(7)); // positions 2..6
    expected.push(6); // double tag at position 7
    expected.extend_from_slice(&ne_f64(1.5)); // payload at aligned position 8
    assert_eq!(bytes, expected);
    assert_eq!(decode_message(Some(&bytes)).unwrap(), value);
}

#[test]
fn expanding_size_boundaries_match_flutter() {
    // message_codecs.dart:275-281,545-562: 0..253 = 1 byte, 254..65535 =
    // 0xFE + u16, else 0xFF + u32.
    let small = Value::String("x".repeat(253));
    let bytes = encode_message(&small).unwrap().unwrap();
    assert_eq!(bytes[0], 7);
    assert_eq!(bytes[1], 253);
    assert_eq!(decode_message(Some(&bytes)).unwrap(), small);

    let medium = Value::String("y".repeat(254));
    let bytes = encode_message(&medium).unwrap().unwrap();
    assert_eq!(&bytes[1..4], &[254, 254, 0]);
    assert_eq!(decode_message(Some(&bytes)).unwrap(), medium);

    let large = Value::String("z".repeat(65_536));
    let bytes = encode_message(&large).unwrap().unwrap();
    assert_eq!(bytes[1], 255);
    let len = u32::from_ne_bytes(bytes[2..6].try_into().unwrap());
    assert_eq!(len, 65_536);
    assert_eq!(decode_message(Some(&bytes)).unwrap(), large);
}

#[test]
fn utf8_strings_and_typed_lists_round_trip() {
    let mut map = BTreeMap::new();
    map.insert(Value::String("名字".to_owned()), Value::I32(42));
    let value = Value::List(vec![
        Value::String("Flutter 中文".to_owned()),
        Value::Uint8List(vec![0, 1, 255]),
        Value::Int32List(vec![i32::MIN, 0, i32::MAX]),
        Value::Int64List(vec![i64::MIN, i64::MAX]),
        Value::Float32List(vec![1.5, -2.25]),
        Value::Float64List(vec![1.5, -2.25]),
        Value::Map(map),
    ]);
    let bytes = encode_message(&value).unwrap().unwrap();
    assert_eq!(decode_message(Some(&bytes)).unwrap(), value);
}

#[test]
fn malformed_messages_are_rejected() {
    assert_eq!(
        decode_message(Some(&[])).unwrap_err(),
        CodecError::Corrupt("truncated")
    );
    assert_eq!(
        decode_message(Some(&[99])).unwrap_err(),
        CodecError::Corrupt("unknown tag")
    );
    // Trailing byte after a complete value: message_codecs.dart:341-345.
    assert_eq!(
        decode_message(Some(&[1, 0])).unwrap_err(),
        CodecError::Corrupt("trailing bytes")
    );
    // Invalid UTF-8.
    assert_eq!(
        decode_message(Some(&[7, 1, 0xff])).unwrap_err(),
        CodecError::Corrupt("invalid UTF-8")
    );
}

#[test]
fn method_call_envelope_round_trip() {
    let call = MethodCall {
        method: "getDeviceInfo".to_owned(),
        arguments: Value::Map(BTreeMap::from([(
            Value::String("keys".to_owned()),
            Value::List(vec![Value::String("model".to_owned())]),
        )])),
    };
    let bytes = encode_method_call(&call).unwrap();
    assert_eq!(decode_method_call(&bytes).unwrap(), call);
    // First value is the method name string tag; method_codecs.dart:602-603.
    assert_eq!(bytes[0], 7);
}

#[test]
fn reply_envelopes_match_flutter_error_semantics() {
    let success = Reply::Success(Value::Bool(true));
    let bytes = encode_envelope(&success).unwrap().unwrap();
    assert_eq!(bytes[0], 0);
    assert_eq!(decode_envelope(Some(&bytes)).unwrap(), success);

    let error = Reply::PlatformError {
        code: error_codes::INVALID_ARGUMENT.to_owned(),
        message: Some("missing key".to_owned()),
        details: Value::Null,
    };
    let bytes = encode_envelope(&error).unwrap().unwrap();
    assert_eq!(bytes[0], 1);
    assert_eq!(decode_envelope(Some(&bytes)).unwrap(), error);

    // A null reply is not a success envelope: platform_channel.dart:350-366.
    assert_eq!(encode_envelope(&Reply::MissingPlugin).unwrap(), None);
    assert_eq!(decode_envelope(None).unwrap(), Reply::MissingPlugin);
    assert_eq!(
        decode_envelope(Some(&[])).unwrap_err(),
        CodecError::Corrupt("truncated")
    );
}

#[test]
fn error_envelope_with_optional_stacktrace_is_accepted() {
    // message_codecs.dart:653-655: a fourth String value is an optional
    // stacktrace. Opurom consumes it but does not expose it.
    let mut bytes = encode_envelope(&Reply::PlatformError {
        code: "error".to_owned(),
        message: None,
        details: Value::Null,
    })
    .unwrap()
    .unwrap();
    let stack = encode_message(&Value::String("stack".to_owned()))
        .unwrap()
        .unwrap();
    bytes.extend_from_slice(&stack);
    assert_eq!(
        decode_envelope(Some(&bytes)).unwrap(),
        Reply::PlatformError {
            code: "error".to_owned(),
            message: Some(String::new()),
            details: Value::Null,
        }
    );
}

static HANDLER_STATE: &[u8] = b"device-info";

/// Test handler echoing a success envelope. This mirrors the shape of
/// Flutter's `_handleAsMethodCall` wrapping (`platform_channel.dart:594-614`)
/// without installing a runtime callback.
unsafe fn echo_handler(state: *mut u8, message: &[u8]) -> Result<Option<Vec<u8>>, CodecError> {
    assert_eq!(state, HANDLER_STATE.as_ptr() as *mut u8);
    let call = decode_method_call(message)?;
    encode_envelope(&Reply::Success(Value::String(format!(
        "echo:{}",
        call.method
    ))))
}

unsafe fn error_handler(_state: *mut u8, _message: &[u8]) -> Result<Option<Vec<u8>>, CodecError> {
    encode_envelope(&Reply::PlatformError {
        code: error_codes::UNIMPLEMENTED.to_owned(),
        message: None,
        details: Value::Null,
    })
}

#[test]
fn channel_table_dispatches_by_name_and_missing_returns_null_reply() {
    let table = opurom_platform::platform_channel::ChannelTable::new()
        .with(
            ChannelSpec {
                name: "opurom/test",
                kind: ChannelKind::Method { optional: false },
            },
            Handler {
                function: echo_handler,
                state: HANDLER_STATE.as_ptr() as *mut u8,
            },
        )
        .with(
            ChannelSpec {
                name: "opurom/unimplemented",
                kind: ChannelKind::Method { optional: true },
            },
            Handler {
                function: error_handler,
                state: std::ptr::null_mut(),
            },
        );
    assert_eq!(table.len(), 2);
    assert_eq!(
        table.spec("opurom/test").unwrap().kind,
        ChannelKind::Method { optional: false }
    );

    let call = encode_method_call(&MethodCall {
        method: "ping".to_owned(),
        arguments: Value::Null,
    })
    .unwrap();
    let reply = unsafe { table.dispatch("opurom/test", &call) }
        .unwrap()
        .unwrap();
    assert_eq!(
        decode_envelope(Some(&reply)).unwrap(),
        Reply::Success(Value::String("echo:ping".to_owned()))
    );
    // Missing channel => null reply => MissingPluginException on a strict
    // MethodChannel, exactly like Flutter's engine behaviour.
    assert_eq!(
        unsafe { table.dispatch("opurom/unknown", &call) }.unwrap(),
        None
    );
}

#[test]
fn channel_buffer_drops_oldest_and_tracks_overflow() {
    // channel_buffers.dart:75-81,131-144,321: capacity 1, FIFO, drop oldest.
    let mut buffer = ChannelBuffer::new("opurom/event");
    assert_eq!(ChannelBuffer::<u8>::DEFAULT_CAPACITY, 1);
    assert!(buffer.push_stamped(1, 10));
    assert!(buffer.push_stamped(2, 11));
    assert_eq!(buffer.pop_stamped(), Some((2, 11)));
    assert_eq!(buffer.pop_stamped(), None);
    assert_eq!(buffer.dropped(), 1);

    let mut buffer = ChannelBuffer::with_capacity("opurom/messages", 2);
    for n in 0..4 {
        buffer.push(n);
    }
    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.pop(), Some(2));
    assert_eq!(buffer.pop(), Some(3));
    assert_eq!(buffer.dropped(), 2);
}

#[test]
fn event_channel_control_names_match_flutter() {
    // platform_channel.dart:710,725: listen on 0→1 listener, cancel on 1→0.
    assert_eq!(EVENT_LISTEN, "listen");
    assert_eq!(EVENT_CANCEL, "cancel");
}

#[test]
fn distinct_numeric_tags_are_distinct_map_keys() {
    // I32 and I64 are different Flutter tags (message_codecs.dart:310-312),
    // so they must never collapse into one BTreeMap key.
    let mut map = BTreeMap::new();
    map.insert(Value::I32(1), Value::String("i32".to_owned()));
    map.insert(Value::I64(1), Value::String("i64".to_owned()));
    assert_eq!(map.len(), 2);
    let value = Value::Map(map);
    let bytes = encode_message(&value).unwrap().unwrap();
    assert_eq!(decode_message(Some(&bytes)).unwrap(), value);
}
