//! Contract test for the per-channel codec table.
//!
//! Flutter baseline: 3.50.0 (`flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `system_channels.dart` declares each framework channel with its codec.
//! The codec fixes the wire format, so the pairing of name, kind and codec
//! is part of the interoperability contract, not an implementation detail.
//! This test pins that pairing against the Flutter source literals and
//! proves that every bridged channel is encoded and decoded through its own
//! official codec.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:385-388` —
//!   `flutter/lifecycle` is a `BasicMessageChannel<String?>` with
//!   `StringCodec`.
//! - `packages/flutter/lib/src/services/system_channels.dart:401-404` —
//!   `flutter/system` is a `BasicMessageChannel<Object?>` with
//!   `JSONMessageCodec`.
//! - `packages/flutter/lib/src/services/system_channels.dart:181-184` —
//!   `flutter/platform` is an `OptionalMethodChannel` with
//!   `JSONMethodCodec`.
//! - `packages/flutter/lib/src/services/system_channels.dart:423,430` —
//!   `flutter/platform_views` and `flutter/platform_views_2` are
//!   `MethodChannel`s with the default `StandardMethodCodec`
//!   (`services/platform_channel.dart:291-299`).
//! - `packages/flutter/lib/src/services/platform_channel.dart:202-256,291-299,
//!   622-633` — channel kinds, default codec and the optional-channel
//!   difference.
//! - `packages/flutter/lib/src/services/message_codecs.dart:44-62,88-114,
//!   120-200,263-324,580-596` — the five codecs and their wire formats.
//!
//! No ABI/IR change: the test only reads the compile-time contract table
//! and the existing encoders.

use std::collections::BTreeSet;

use opurom_platform::channel_contract::{
    contract, decode_channel_envelope, decode_channel_message, decode_channel_method_call,
    encode_channel_envelope, encode_channel_message, encode_channel_method_call, ChannelCodec,
    BRIDGED_CHANNELS,
};
use opurom_platform::channel_event_bridge::{decode_lifecycle_message, decode_system_message};
use opurom_platform::framework_channels::parse_lifecycle_message;
use opurom_platform::platform_channel::{
    decode_message, encode_message, error_codes, system_channels, MethodCall, Reply, Value,
};

/// The declaration of one channel, as read from `system_channels.dart`.
struct Declared {
    name: &'static str,
    pin: &'static str,
    kind: &'static str,
    codec: ChannelCodec,
    optional: bool,
}

const DECLARED: &[Declared] = &[
    Declared {
        name: "flutter/lifecycle",
        pin: "system_channels.dart:385-388",
        kind: "BasicMessageChannel<String?>",
        codec: ChannelCodec::String,
        optional: false,
    },
    Declared {
        name: "flutter/system",
        pin: "system_channels.dart:401-404",
        kind: "BasicMessageChannel<Object?>",
        codec: ChannelCodec::Json,
        optional: false,
    },
    Declared {
        name: "flutter/platform",
        pin: "system_channels.dart:181-184",
        kind: "OptionalMethodChannel",
        codec: ChannelCodec::Json,
        optional: true,
    },
    Declared {
        name: "flutter/platform_views",
        pin: "system_channels.dart:423",
        kind: "MethodChannel",
        codec: ChannelCodec::Standard,
        optional: false,
    },
    Declared {
        name: "flutter/platform_views_2",
        pin: "system_channels.dart:430",
        kind: "MethodChannel",
        codec: ChannelCodec::Standard,
        optional: false,
    },
];

#[test]
fn contract_table_matches_the_flutter_declarations() {
    assert_eq!(BRIDGED_CHANNELS.len(), DECLARED.len());
    for declared in DECLARED {
        let entry = contract(declared.name)
            .unwrap_or_else(|| panic!("{} is not in the contract table", declared.name));
        assert_eq!(entry.name, declared.name);
        assert_eq!(entry.codec, declared.codec, "{}", declared.pin);
        assert_eq!(entry.optional, declared.optional, "{}", declared.pin);
        match (declared.kind, entry.kind) {
            ("BasicMessageChannel<Object?>", _) | ("BasicMessageChannel<String?>", _) => {
                assert_eq!(
                    entry.kind,
                    opurom_platform::platform_channel::ChannelKind::BasicMessage,
                    "{}",
                    declared.pin
                );
            }
            ("OptionalMethodChannel", _) => {
                assert_eq!(
                    entry.kind,
                    opurom_platform::platform_channel::ChannelKind::Method { optional: true },
                    "{}",
                    declared.pin
                );
            }
            ("MethodChannel", _) => {
                assert_eq!(
                    entry.kind,
                    opurom_platform::platform_channel::ChannelKind::Method { optional: false },
                    "{}",
                    declared.pin
                );
            }
            other => panic!("unknown channel kind {}", other.0),
        }
    }
}

#[test]
fn contract_names_are_unique_and_use_the_official_namespaces() {
    let names: BTreeSet<&str> = BRIDGED_CHANNELS.iter().map(|entry| entry.name).collect();
    assert_eq!(names.len(), BRIDGED_CHANNELS.len());
    for entry in BRIDGED_CHANNELS {
        assert!(
            entry.name.starts_with("flutter/"),
            "{} is outside the framework namespace",
            entry.name
        );
    }
    // The frozen names are the ones the crate already publishes.
    assert!(names.contains(system_channels::LIFECYCLE));
    assert!(names.contains(system_channels::SYSTEM));
    assert!(names.contains(system_channels::PLATFORM));
    assert!(names.contains(system_channels::PLATFORM_VIEWS));
    assert!(names.contains(system_channels::PLATFORM_VIEWS_2));
}

#[test]
fn the_lifecycle_channel_carries_only_raw_utf8() {
    let message = "AppLifecycleState.paused";
    let bytes = encode_channel_message(
        system_channels::LIFECYCLE,
        &Value::String(message.to_owned()),
    )
    .expect("lifecycle message encodes")
    .expect("a string message has bytes");
    assert_eq!(bytes, message.as_bytes().to_vec());
    assert_eq!(
        decode_channel_message(system_channels::LIFECYCLE, Some(&bytes)),
        Ok(Value::String(message.to_owned()))
    );
    // The framework parses the same payload on the same channel.
    assert_eq!(
        parse_lifecycle_message(&bytes),
        Ok(opurom_platform::Lifecycle::Paused)
    );
    assert_eq!(
        decode_lifecycle_message(&bytes),
        Ok(opurom_platform::Lifecycle::Paused)
    );
    // A standard-codec payload is not a valid lifecycle string.
    let standard = encode_message(&Value::String(message.to_owned())).expect("standard encodes");
    assert!(parse_lifecycle_message(standard.as_deref().unwrap()).is_err());
}

#[test]
fn the_system_channel_carries_json_only() {
    let event = Value::Map(std::collections::BTreeMap::from([(
        Value::String("type".to_owned()),
        Value::String("memoryPressure".to_owned()),
    )]));
    let bytes = encode_channel_message(system_channels::SYSTEM, &event)
        .expect("system message encodes")
        .expect("a map message has bytes");
    assert_eq!(bytes, br#"{"type":"memoryPressure"}"#.to_vec());
    assert_eq!(
        decode_channel_message(system_channels::SYSTEM, Some(&bytes)),
        Ok(event)
    );
    // The framework event bridge reads the same payload on the same channel.
    assert_eq!(
        decode_system_message(&bytes),
        Ok(opurom_platform::system_events::SystemEvent::MemoryPressure)
    );
    // A standard-codec payload is not valid JSON.
    let standard = encode_message(&Value::String("memoryPressure".to_owned())).expect("encodes");
    assert!(decode_system_message(standard.as_deref().unwrap()).is_err());
}

#[test]
fn the_platform_channel_is_a_json_method_channel() {
    let call = MethodCall {
        method: "HapticFeedback.vibrate".to_owned(),
        arguments: Value::Null,
    };
    let bytes = encode_channel_method_call(system_channels::PLATFORM, &call).expect("encodes");
    assert_eq!(
        bytes,
        br#"{"method":"HapticFeedback.vibrate","args":null}"#.to_vec()
    );
    assert_eq!(
        decode_channel_method_call(system_channels::PLATFORM, &bytes),
        Ok(call)
    );
}

#[test]
fn the_platform_view_channels_are_binary_method_channels() {
    for name in [
        system_channels::PLATFORM_VIEWS,
        system_channels::PLATFORM_VIEWS_2,
    ] {
        let call = MethodCall {
            method: "create".to_owned(),
            arguments: Value::I32(3),
        };
        let bytes = encode_channel_method_call(name, &call).expect("encodes");
        assert_eq!(bytes[0], 7, "{name} must start with the string tag");
        assert_eq!(decode_channel_method_call(name, &bytes), Ok(call));
    }
}

#[test]
fn a_payload_encoded_for_one_codec_never_decodes_on_another() {
    let call = MethodCall {
        method: "create".to_owned(),
        arguments: Value::Null,
    };
    let json = encode_channel_method_call(system_channels::PLATFORM, &call).expect("encodes");
    let binary =
        encode_channel_method_call(system_channels::PLATFORM_VIEWS, &call).expect("encodes");
    assert_ne!(json, binary);
    assert!(decode_channel_method_call(system_channels::PLATFORM_VIEWS, &json).is_err());
    assert!(decode_channel_method_call(system_channels::PLATFORM, &binary).is_err());

    let message = Value::String("AppLifecycleState.resumed".to_owned());
    let lifecycle = encode_channel_message(system_channels::LIFECYCLE, &message).expect("encodes");
    assert!(decode_channel_message(system_channels::SYSTEM, lifecycle.as_deref()).is_err());
}

#[test]
fn envelopes_and_missing_plugins_follow_the_channel_contract() {
    let error = Reply::PlatformError {
        code: error_codes::ERROR.to_owned(),
        message: Some("failed".to_owned()),
        details: Value::Null,
    };
    let json = encode_channel_envelope(system_channels::PLATFORM, &error)
        .expect("encodes")
        .expect("an error envelope has bytes");
    assert_eq!(json, br#"["error","failed",null]"#.to_vec());
    assert_eq!(
        decode_channel_envelope(system_channels::PLATFORM, Some(&json)),
        Ok(error.clone())
    );

    let binary = encode_channel_envelope(system_channels::PLATFORM_VIEWS, &error)
        .expect("encodes")
        .expect("an error envelope has bytes");
    assert_eq!(binary[0], 1, "the first byte distinguishes an error");
    assert_eq!(
        decode_channel_envelope(system_channels::PLATFORM_VIEWS, Some(&binary)),
        Ok(error)
    );

    // A null reply is the missing-plugin signal on every method channel.
    for name in [
        system_channels::PLATFORM,
        system_channels::PLATFORM_VIEWS,
        system_channels::PLATFORM_VIEWS_2,
    ] {
        assert_eq!(
            encode_channel_envelope(name, &Reply::MissingPlugin),
            Ok(None)
        );
        assert_eq!(
            decode_channel_envelope(name, None),
            Ok(Reply::MissingPlugin)
        );
    }
}

#[test]
fn a_null_message_is_never_put_on_the_wire() {
    // Every Flutter codec maps a null message to no bytes
    // (`message_codecs.dart:48-53,56-61,99-104`).
    for name in [system_channels::LIFECYCLE, system_channels::SYSTEM] {
        assert_eq!(encode_channel_message(name, &Value::Null), Ok(None));
        assert_eq!(decode_channel_message(name, None), Ok(Value::Null));
    }
}

#[test]
fn the_contract_table_is_the_only_source_of_channel_truth() {
    // The standard decoder is not reachable through a JSON or string
    // channel, and vice versa: a contract decides the codec.
    let standard = encode_message(&Value::I32(1)).expect("encodes");
    assert!(decode_message(standard.as_deref()).is_ok());
    assert!(decode_channel_message(system_channels::SYSTEM, standard.as_deref()).is_err());
    // `StringCodec` accepts any UTF-8 byte sequence, so it decodes the
    // binary payload into a different string rather than the integer.
    assert_ne!(
        decode_channel_message(system_channels::LIFECYCLE, standard.as_deref())
            .expect("raw UTF-8 decodes"),
        Value::I32(1)
    );
}
