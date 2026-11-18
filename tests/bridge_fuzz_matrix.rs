//! Deterministic malformed-input matrix for the platform bridge.
//!
//! This is a bounded, reproducible fuzz matrix rather than a random fuzzer:
//! the same values run on every target and every CI job, so a regression is
//! always replayable. Inputs derive from the closed Flutter codec domains:
//! `message_codecs.dart:310-562` (standard tags and expanding sizes) and
//! `message_codecs.dart:88-200` (JSON method/envelope shapes).

use opurom_platform::framework_channels::{
    decode_json_envelope, decode_json_message, decode_json_method_call, parse_lifecycle_message,
};
use opurom_platform::host_injection_audit::{audit_all, missing_handler_reply};
use opurom_platform::native_capabilities::{self, ALL_CAPABILITIES};
use opurom_platform::platform_channel::{
    decode_envelope, decode_message, decode_method_call, error_codes, MethodCall, Reply, Value,
};

#[test]
fn standard_codec_never_panics_on_bounded_byte_matrix() {
    for len in 0..=8usize {
        for byte in [0u8, 1, 2, 3, 6, 12, 13, 14, 253, 254, 255] {
            let bytes = vec![byte; len];
            let _ = decode_message(Some(&bytes));
            let _ = decode_method_call(&bytes);
            let _ = decode_envelope(Some(&bytes));
        }
    }
    // Known corrupt cases must stay typed errors, not panics or fake values.
    assert!(decode_message(Some(&[99])).is_err());
    assert!(decode_envelope(Some(&[3])).is_err());
}

#[test]
fn json_codec_never_panics_on_bounded_text_matrix() {
    let corpus = [
        "",
        " ",
        "null",
        "true",
        "0",
        "-1",
        "1.5",
        "1e400",
        "[]",
        "[1]",
        "[1,2,3,4]",
        "{}",
        "{\"method\":1}",
        "{\"method\":\"m\"}",
        "{\"method\":\"m\",\"args\":1}",
        "{\"type\":\"memoryPressure\"}",
        "{\"type\":\"unknown\"}",
        "nul",
        "{]",
    ];
    for text in corpus {
        let bytes = text.as_bytes();
        let _ = decode_json_message(bytes);
        let _ = decode_json_method_call(bytes);
        let _ = decode_json_envelope(bytes);
        let _ = parse_lifecycle_message(bytes);
    }
}

#[test]
fn standard_and_json_paths_agree_on_logical_values() {
    use opurom_platform::framework_channels::{decode_json_message, encode_json_message};
    use opurom_platform::platform_channel::{decode_message, encode_message};
    use std::collections::BTreeMap;

    // Flutter's two codecs have different byte layouts but must preserve the
    // same logical values (`message_codecs.dart:88-200,263-562`). JSON has a
    // single Dart `int` type, so an i64 that fits i32 is read back as I32;
    // normalize that representational difference before comparing.
    let values = [
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::I32(i32::MIN),
        Value::I32(i32::MAX),
        Value::I64(i64::MIN),
        Value::I64(i64::MAX),
        Value::F64(0.5),
        Value::String("hello".to_owned()),
        Value::String("escaped\nquote\"".to_owned()),
        Value::List(vec![
            Value::Null,
            Value::Bool(true),
            Value::I32(7),
            Value::I64(9),
            Value::F64(1.25),
            Value::String("x".to_owned()),
        ]),
        Value::Map(BTreeMap::from([
            (Value::String("a".to_owned()), Value::I32(1)),
            (Value::String("b".to_owned()), Value::Bool(false)),
            (Value::String("c".to_owned()), Value::Null),
        ])),
    ];
    for value in values {
        let standard = encode_message(&value).unwrap();
        assert_eq!(decode_message(standard.as_deref()).unwrap(), value);
        let json = encode_json_message(&value).unwrap();
        assert_eq!(
            normalize_json_ints(&decode_json_message(&json).unwrap()),
            normalize_json_ints(&value)
        );
    }
}

/// Fold I64 values inside the I32 range into I32, matching Dart's single
/// `int` type after `json.decoder` (`message_codecs.dart:97,391-397`).
fn normalize_json_ints(value: &Value) -> Value {
    match value {
        Value::I64(n) if i32::try_from(*n).is_ok() => Value::I32(*n as i32),
        Value::List(items) => Value::List(items.iter().map(normalize_json_ints).collect()),
        Value::Map(entries) => Value::Map(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), normalize_json_ints(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
fn every_capability_method_has_a_typed_missing_handler_reply() {
    let rows = audit_all();
    assert_eq!(rows.len(), ALL_CAPABILITIES.len());
    for capability in ALL_CAPABILITIES {
        let reply = missing_handler_reply(capability);
        match reply {
            Reply::PlatformError {
                code,
                message,
                details,
            } => {
                assert_eq!(code, error_codes::UNIMPLEMENTED);
                assert!(message.is_some());
                assert_eq!(details, Value::Null);
            }
            other => panic!("capability {capability:?} fabricated {other:?}"),
        }
        for method in native_capabilities::methods_for(capability) {
            let call = MethodCall {
                method: (*method).to_owned(),
                arguments: Value::Null,
            };
            // The qualified/unqualified naming rules are pure and total.
            assert!(!call.method.is_empty());
        }
    }
}
