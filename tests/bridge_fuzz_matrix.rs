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
