//! Framework-level channel codecs and platform method contract.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`. Every file:line below refers to
//! that checkout.
//!
//! `platform_channel` implements the standard binary codec. This module adds
//! the two other codecs used by Flutter's framework channels: `StringCodec`
//! for `flutter/lifecycle` and `JSONMessageCodec`/`JSONMethodCodec` for
//! `flutter/system` and `flutter/platform`. It also freezes the framework
//! method names and argument keys so capability handlers can be written
//! without re-deriving them.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:44-62` —
//!   `StringCodec` is raw UTF-8, null maps to no bytes.
//! - `packages/flutter/lib/src/services/message_codecs.dart:88-114` —
//!   `JSONMessageCodec` uses `JsonUtf8Encoder` and
//!   `utf8.decoder.fuse(json.decoder)`.
//! - `packages/flutter/lib/src/services/message_codecs.dart:120-200` —
//!   `JSONMethodCodec` calls are two-key maps, replies are one-element
//!   success lists or three/four-element error lists.
//! - `packages/flutter/lib/src/services/system_channels.dart:181-188,385-388,
//!   395-405` — channel names and codecs.
//! - `packages/flutter/lib/src/services/binding.dart:448-457` — the five
//!   lifecycle strings; `:174-182` — the sole system-event type.
//! - `packages/flutter/lib/src/scheduler/binding.dart:416-430` — the frame
//!   gate for each lifecycle state.
//! - `packages/flutter/lib/src/services/clipboard.dart:32-68`,
//!   `haptic_feedback.dart:19-152`, `system_navigator.dart:39-97`,
//!   `system_chrome.dart:547-552,609-618,651,665,762` — framework method
//!   names on `flutter/platform`.

use std::collections::BTreeMap;

use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};

use crate::platform_channel::{error_codes, CodecError, MethodCall, Reply, Value};
use crate::{frames_enabled, Lifecycle};

// ------------------
// JSON codec
// ------------------

/// Convert a standard-codec value to its JSON representation. Typed lists
/// lose their tag, matching `JSONMessageCodec`, which hands the value to
/// `JsonUtf8Encoder` without special-casing typed data
/// (`message_codecs.dart:96`).
pub fn value_to_json(value: &Value) -> Result<JsonValue, CodecError> {
    Ok(match value {
        Value::Null => JsonValue::Null,
        Value::Bool(v) => JsonValue::Bool(*v),
        Value::I32(v) => JsonValue::Number(JsonNumber::from(*v)),
        Value::I64(v) => JsonValue::Number(JsonNumber::from(*v)),
        Value::F64(v) => JsonNumber::from_f64(*v)
            .map(JsonValue::Number)
            .ok_or_else(|| CodecError::Unsupported("non-finite float in JSON message"))?,
        Value::String(v) => JsonValue::String(v.clone()),
        Value::Uint8List(items) => JsonValue::Array(
            items
                .iter()
                .map(|v| JsonValue::Number(JsonNumber::from(*v)))
                .collect(),
        ),
        Value::Int32List(items) => JsonValue::Array(
            items
                .iter()
                .map(|v| JsonValue::Number(JsonNumber::from(*v)))
                .collect(),
        ),
        Value::Int64List(items) => JsonValue::Array(
            items
                .iter()
                .map(|v| JsonValue::Number(JsonNumber::from(*v)))
                .collect(),
        ),
        Value::Float32List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(
                    JsonNumber::from_f64(f64::from(*item))
                        .map(JsonValue::Number)
                        .ok_or_else(|| CodecError::Unsupported("non-finite float in JSON list"))?,
                );
            }
            JsonValue::Array(out)
        }
        Value::Float64List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(
                    JsonNumber::from_f64(*item)
                        .map(JsonValue::Number)
                        .ok_or_else(|| CodecError::Unsupported("non-finite float in JSON list"))?,
                );
            }
            JsonValue::Array(out)
        }
        Value::List(items) => JsonValue::Array(
            items
                .iter()
                .map(value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Map(entries) => {
            let mut out = JsonMap::new();
            for (key, val) in entries {
                let Value::String(key) = key else {
                    return Err(CodecError::Unsupported("JSON object key is not a string"));
                };
                out.insert(key.clone(), value_to_json(val)?);
            }
            JsonValue::Object(out)
        }
    })
}

/// Reverse of [`value_to_json`]. JSON integers become `I32`/`I64`; `1.0`
/// stays a float when its lexical form carries a decimal point or an
/// exponent, which is how Dart distinguishes `int` from `double` after
/// `json.decoder` (`message_codecs.dart:97`).
pub fn json_to_value(value: &JsonValue) -> Result<Value, CodecError> {
    Ok(match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(v) => Value::Bool(*v),
        JsonValue::Number(number) => {
            if let Some(v) = number.as_i64() {
                if let Ok(v) = i32::try_from(v) {
                    Value::I32(v)
                } else {
                    Value::I64(v)
                }
            } else if let Some(v) = number.as_u64() {
                Value::I64(i64::try_from(v).map_err(|_| CodecError::Corrupt("integer overflow"))?)
            } else {
                Value::F64(
                    number
                        .as_f64()
                        .ok_or_else(|| CodecError::Corrupt("JSON number is not representable"))?,
                )
            }
        }
        JsonValue::String(v) => Value::String(v.clone()),
        JsonValue::Array(items) => Value::List(
            items
                .iter()
                .map(json_to_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        JsonValue::Object(entries) => Value::Map(
            entries
                .iter()
                .map(|(key, val)| Ok((Value::String(key.clone()), json_to_value(val)?)))
                .collect::<Result<BTreeMap<_, _>, CodecError>>()?,
        ),
    })
}

/// `JSONMessageCodec.encodeMessage` (`message_codecs.dart:99-104`).
pub fn encode_json_message(value: &Value) -> Result<Vec<u8>, CodecError> {
    let json = value_to_json(value)?;
    serde_json::to_vec(&json).map_err(|_| CodecError::Unsupported("JSON message encode failed"))
}

fn decode_json_value(bytes: &[u8]) -> Result<JsonValue, CodecError> {
    let mut stream = serde_json::Deserializer::from_slice(bytes).into_iter::<JsonValue>();
    let value = stream
        .next()
        .ok_or_else(|| CodecError::Corrupt("empty JSON message"))?
        .map_err(|_| CodecError::Corrupt("invalid JSON message"))?;
    if stream.next().is_some() {
        return Err(CodecError::Corrupt("trailing JSON message"));
    }
    Ok(value)
}

/// `JSONMessageCodec.decodeMessage` (`message_codecs.dart:106-112`). Trailing
/// JSON values are rejected, as `json.decoder` rejects them.
pub fn decode_json_message(bytes: &[u8]) -> Result<Value, CodecError> {
    json_to_value(&decode_json_value(bytes)?)
}

/// `JSONMethodCodec.encodeMethodCall` (`message_codecs.dart:138-143`).
///
/// Flutter builds the two-key map as `{'method': method, 'args': arguments}`
/// and `JsonUtf8Encoder` preserves map insertion order, so the bytes are
/// written in that order rather than in the sorted order a generic JSON
/// object writer would produce.
pub fn encode_json_method_call(call: &MethodCall) -> Result<Vec<u8>, CodecError> {
    let method = serde_json::to_string(&JsonValue::String(call.method.clone()))
        .map_err(|_| CodecError::Unsupported("JSON method call encode failed"))?;
    let arguments = serde_json::to_string(&value_to_json(&call.arguments)?)
        .map_err(|_| CodecError::Unsupported("JSON method call encode failed"))?;
    let mut out = Vec::with_capacity(method.len() + arguments.len() + 20);
    out.extend_from_slice(b"{\"method\":");
    out.extend_from_slice(method.as_bytes());
    out.extend_from_slice(b",\"args\":");
    out.extend_from_slice(arguments.as_bytes());
    out.push(b'}');
    Ok(out)
}

/// `JSONMethodCodec.decodeMethodCall` (`message_codecs.dart:145-157`). A
/// missing `args` key is `null`; a non-object or a non-string method is a
/// `FormatException`, represented here by `CodecError::Corrupt`.
pub fn decode_json_method_call(bytes: &[u8]) -> Result<MethodCall, CodecError> {
    let JsonValue::Object(mut object) = decode_json_value(bytes)? else {
        return Err(CodecError::Corrupt("expected method call map"));
    };
    let Some(JsonValue::String(method)) = object.remove("method") else {
        return Err(CodecError::Corrupt("invalid method call"));
    };
    let arguments = match object.remove("args") {
        Some(value) => json_to_value(&value)?,
        None => Value::Null,
    };
    Ok(MethodCall { method, arguments })
}

/// `JSONMethodCodec.encodeSuccessEnvelope` / `encodeErrorEnvelope`
/// (`message_codecs.dart:190-200`). `MissingPlugin` has no JSON wire form: a
/// null reply is not an envelope in Flutter either.
pub fn encode_json_envelope(reply: &Reply) -> Result<Vec<u8>, CodecError> {
    let json = match reply {
        Reply::Success(value) => JsonValue::Array(vec![value_to_json(value)?]),
        Reply::PlatformError {
            code,
            message,
            details,
        } => JsonValue::Array(vec![
            JsonValue::String(code.clone()),
            message
                .as_ref()
                .map(|v| JsonValue::String(v.clone()))
                .unwrap_or(JsonValue::Null),
            value_to_json(details)?,
        ]),
        Reply::MissingPlugin => {
            return Err(CodecError::Unsupported("null reply has no JSON envelope"));
        }
    };
    serde_json::to_vec(&json).map_err(|_| CodecError::Unsupported("JSON envelope encode failed"))
}

/// `JSONMethodCodec.decodeEnvelope` (`message_codecs.dart:159-188`). A fourth
/// stacktrace string is accepted and ignored; this crate never synthesizes a
/// native stacktrace.
pub fn decode_json_envelope(bytes: &[u8]) -> Result<Reply, CodecError> {
    let JsonValue::Array(items) = decode_json_value(bytes)? else {
        return Err(CodecError::Corrupt("expected envelope list"));
    };
    match items.len() {
        1 => Ok(Reply::Success(json_to_value(&items[0])?)),
        3 | 4 => {
            let JsonValue::String(code) = &items[0] else {
                return Err(CodecError::Corrupt("invalid envelope code"));
            };
            let message = match &items[1] {
                JsonValue::Null => None,
                JsonValue::String(value) => Some(value.clone()),
                _ => return Err(CodecError::Corrupt("invalid envelope message")),
            };
            if items.len() == 4 && !matches!(items[3], JsonValue::Null | JsonValue::String(_)) {
                return Err(CodecError::Corrupt("invalid envelope stacktrace"));
            }
            Ok(Reply::PlatformError {
                code: code.clone(),
                message,
                details: json_to_value(&items[2])?,
            })
        }
        _ => Err(CodecError::Corrupt("invalid envelope")),
    }
}

// ------------------
// Lifecycle wire
// ------------------

/// `AppLifecycleState.<lowercase>` string sent on `flutter/lifecycle`
/// (`binding.dart:448-457`) with `StringCodec` (`message_codecs.dart:44-62`).
pub fn lifecycle_message(state: Lifecycle) -> &'static str {
    match state {
        Lifecycle::Resumed => "AppLifecycleState.resumed",
        Lifecycle::Inactive => "AppLifecycleState.inactive",
        Lifecycle::Hidden => "AppLifecycleState.hidden",
        Lifecycle::Paused => "AppLifecycleState.paused",
        Lifecycle::Destroyed => "AppLifecycleState.detached",
    }
}

/// Decode a `StringCodec` payload into the exact five states Flutter accepts.
/// Unknown strings are `null` in `binding.dart:455`, reported here as
/// `CodecError::Corrupt`.
pub fn parse_lifecycle_message(bytes: &[u8]) -> Result<Lifecycle, CodecError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| CodecError::Corrupt("lifecycle message is not UTF-8"))?;
    match text {
        "AppLifecycleState.resumed" => Ok(Lifecycle::Resumed),
        "AppLifecycleState.inactive" => Ok(Lifecycle::Inactive),
        "AppLifecycleState.hidden" => Ok(Lifecycle::Hidden),
        "AppLifecycleState.paused" => Ok(Lifecycle::Paused),
        "AppLifecycleState.detached" => Ok(Lifecycle::Destroyed),
        _ => Err(CodecError::Corrupt("unknown lifecycle message")),
    }
}

/// Encode a lifecycle message with `StringCodec`.
pub fn encode_lifecycle_message(state: Lifecycle) -> Vec<u8> {
    lifecycle_message(state).as_bytes().to_vec()
}

/// The intermediate states Flutter synthesizes between two lifecycle
/// messages (`binding.dart:313-381`), exposed for host event producers.
pub fn lifecycle_transitions(previous: Option<Lifecycle>, next: Lifecycle) -> Vec<Lifecycle> {
    crate::app_lifecycle::transitions(previous, next)
}

/// The frame gate from `scheduler/binding.dart:416-430`.
pub fn lifecycle_frames_enabled(state: Lifecycle) -> bool {
    frames_enabled(state)
}

// ------------------
// Framework method names
// ------------------

/// Method names on `flutter/platform` (`system_channels.dart:181-188`,
/// `JSONMethodCodec`). Opurom capability handlers match these strings
/// verbatim; no new method names are invented at this layer.
pub mod platform_methods {
    /// `clipboard.dart:36`.
    pub const CLIPBOARD_SET_DATA: &str = "Clipboard.setData";
    /// `clipboard.dart:49-51`.
    pub const CLIPBOARD_GET_DATA: &str = "Clipboard.getData";
    /// `clipboard.dart:65-67`.
    pub const CLIPBOARD_HAS_STRINGS: &str = "Clipboard.hasStrings";
    /// `clipboard.dart:32`.
    pub const CLIPBOARD_TEXT_PLAIN: &str = "text/plain";
    /// `clipboard.dart:37`.
    pub const ARG_CLIPBOARD_TEXT: &str = "text";

    /// `haptic_feedback.dart:24,42,58,75,92,113,131,147`.
    pub const HAPTIC_VIBRATE: &str = "HapticFeedback.vibrate";
    pub const HAPTIC_LIGHT_IMPACT: &str = "HapticFeedbackType.lightImpact";
    pub const HAPTIC_MEDIUM_IMPACT: &str = "HapticFeedbackType.mediumImpact";
    pub const HAPTIC_HEAVY_IMPACT: &str = "HapticFeedbackType.heavyImpact";
    pub const HAPTIC_SELECTION_CLICK: &str = "HapticFeedbackType.selectionClick";
    pub const HAPTIC_SUCCESS_NOTIFICATION: &str = "HapticFeedbackType.successNotification";
    pub const HAPTIC_WARNING_NOTIFICATION: &str = "HapticFeedbackType.warningNotification";
    pub const HAPTIC_ERROR_NOTIFICATION: &str = "HapticFeedbackType.errorNotification";

    /// `editable_text.dart:3037`, `selectable_region.dart:1600`.
    pub const SHARE_INVOKE: &str = "Share.invoke";
    /// `system_navigator.dart:65`.
    pub const NAVIGATOR_POP: &str = "SystemNavigator.pop";
    /// `binding.dart:525`.
    pub const SYSTEM_EXIT_APPLICATION: &str = "System.exitApplication";
    /// `binding.dart:588`.
    pub const SYSTEM_INITIALIZATION_COMPLETE: &str = "System.initializationComplete";
    /// `system_chrome.dart:614,620,651,665,762`.
    pub const CHROME_SET_UI_OVERLAY_STYLE: &str = "SystemChrome.setSystemUIOverlayStyle";
    pub const CHROME_SET_UI_CHANGE_LISTENER: &str = "SystemChrome.setSystemUIChangeListener";
    pub const CHROME_RESTORE_UI_OVERLAYS: &str = "SystemChrome.restoreSystemUIOverlays";
    pub const CHROME_SYSTEM_UI_CHANGE: &str = "SystemChrome.systemUIChange";
    /// `system_sound.dart` constant names are `SystemSoundType.alert` etc.;
    /// the channel method is the short suffix (`system_sound.dart:26-33`).
    pub const SYSTEM_SOUND_PLAY: &str = "SystemSound.play";
}

/// The sole type on `flutter/system` (`binding.dart:178`).
pub const SYSTEM_EVENT_MEMORY_PRESSURE: &str = "memoryPressure";

/// Map a bare haptic method name from the capability table to the
/// `HapticFeedbackType.*` type constant string that `HapticFeedback.vibrate`
/// carries (`haptic_feedback.dart:24-147`). `None` means the name is not one
/// of the options the capability table declares.
pub fn haptic_type_name(method: &str) -> Option<&'static str> {
    match method {
        "vibrate" => Some(platform_methods::HAPTIC_VIBRATE),
        "lightImpact" => Some(platform_methods::HAPTIC_LIGHT_IMPACT),
        "mediumImpact" => Some(platform_methods::HAPTIC_MEDIUM_IMPACT),
        "heavyImpact" => Some(platform_methods::HAPTIC_HEAVY_IMPACT),
        "selectionClick" => Some(platform_methods::HAPTIC_SELECTION_CLICK),
        "successNotification" => Some(platform_methods::HAPTIC_SUCCESS_NOTIFICATION),
        "warningNotification" => Some(platform_methods::HAPTIC_WARNING_NOTIFICATION),
        "errorNotification" => Some(platform_methods::HAPTIC_ERROR_NOTIFICATION),
        _ => None,
    }
}

/// Map a haptic type string from `HapticFeedback.vibrate` arguments back to
/// its method name. `None` means the type is not one of Flutter's eight
/// (including the bare `vibrate()` call with no argument).
pub fn haptic_method(type_name: Option<&str>) -> Option<&'static str> {
    match type_name {
        None => Some(platform_methods::HAPTIC_VIBRATE),
        Some(platform_methods::HAPTIC_VIBRATE)
        | Some(platform_methods::HAPTIC_LIGHT_IMPACT)
        | Some(platform_methods::HAPTIC_MEDIUM_IMPACT)
        | Some(platform_methods::HAPTIC_HEAVY_IMPACT)
        | Some(platform_methods::HAPTIC_SELECTION_CLICK)
        | Some(platform_methods::HAPTIC_SUCCESS_NOTIFICATION)
        | Some(platform_methods::HAPTIC_WARNING_NOTIFICATION)
        | Some(platform_methods::HAPTIC_ERROR_NOTIFICATION) => {
            Some(platform_methods::HAPTIC_VIBRATE)
        }
        _ => None,
    }
}

/// A typed `unimplemented` reply for methods whose host backend is absent.
/// Flutter's `OptionalMethodChannel` would turn a null reply into `null`;
/// Opurom's capability table keeps the error visible instead of fabricating
/// success (`platform_channel.dart:350-366`).
pub fn unimplemented(method: &str) -> Reply {
    // Build the diagnostic with exactly one owned allocation: the `Vec`
    // returned by `String::from_utf8`. `format!` uses a formatting machinery
    // allocation plus a string allocation for the same bytes.
    let prefix = b"no host handler for ";
    let mut message = Vec::with_capacity(prefix.len() + method.len());
    message.extend_from_slice(prefix);
    message.extend_from_slice(method.as_bytes());
    let message = match String::from_utf8(message) {
        Ok(message) => message,
        // `method` is a `&str` and the prefix is ASCII; this is unreachable.
        Err(_) => format!("no host handler for {method}"),
    };
    Reply::PlatformError {
        code: error_codes::UNIMPLEMENTED.to_owned(),
        message: Some(message),
        details: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_channel::{
        decode_envelope, decode_method_call, encode_envelope, encode_method_call,
    };

    #[test]
    fn json_round_trip_preserves_dart_types() {
        let value = Value::Map(BTreeMap::from([
            (Value::String("count".to_owned()), Value::I32(i32::MAX)),
            (Value::String("ratio".to_owned()), Value::F64(0.5)),
            (
                Value::String("list".to_owned()),
                Value::List(vec![
                    Value::Null,
                    Value::Bool(true),
                    Value::String("x".into()),
                ]),
            ),
        ]));
        let bytes = encode_json_message(&value).unwrap();
        assert_eq!(decode_json_message(&bytes).unwrap(), value);
        assert_eq!(
            decode_json_message(b"1 2"),
            Err(CodecError::Corrupt("trailing JSON message"))
        );
        assert_eq!(
            decode_json_message(b"{"),
            Err(CodecError::Corrupt("invalid JSON message"))
        );
    }

    #[test]
    fn json_method_call_matches_flutter_shape() {
        let call = MethodCall {
            method: platform_methods::CLIPBOARD_SET_DATA.to_owned(),
            arguments: Value::Map(BTreeMap::from([(
                Value::String("text".to_owned()),
                Value::String("hello".to_owned()),
            )])),
        };
        let bytes = encode_json_method_call(&call).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            "{\"method\":\"Clipboard.setData\",\"args\":{\"text\":\"hello\"}}"
        );
        assert_eq!(decode_json_method_call(&bytes).unwrap(), call);
        assert_eq!(
            decode_json_method_call(br#"{"method":"X"}"#).unwrap(),
            MethodCall {
                method: "X".to_owned(),
                arguments: Value::Null
            }
        );
        assert!(decode_json_method_call(br#"{"args":1}"#).is_err());
    }

    #[test]
    fn json_envelope_matches_success_and_error_shape() {
        let success = Reply::Success(Value::Bool(true));
        assert_eq!(
            String::from_utf8(encode_json_envelope(&success).unwrap()).unwrap(),
            "[true]"
        );
        assert_eq!(decode_json_envelope(b"[true]").unwrap(), success);
        let error = Reply::PlatformError {
            code: "oops".to_owned(),
            message: None,
            details: Value::I32(7),
        };
        assert_eq!(
            String::from_utf8(encode_json_envelope(&error).unwrap()).unwrap(),
            "[\"oops\",null,7]"
        );
        assert_eq!(decode_json_envelope(b"[\"oops\",null,7]").unwrap(), error);
        assert_eq!(
            decode_json_envelope(b"[\"oops\",\"msg\",null,\"trace\"]").unwrap(),
            Reply::PlatformError {
                code: "oops".to_owned(),
                message: Some("msg".to_owned()),
                details: Value::Null,
            }
        );
        assert!(decode_json_envelope(b"[]").is_err());
        assert!(encode_json_envelope(&Reply::MissingPlugin).is_err());
    }

    #[test]
    fn unimplemented_reply_uses_one_exact_diagnostic_allocation() {
        let reply = unimplemented("plugins.flutter.io/camera/takePicture");
        assert_eq!(
            reply,
            Reply::PlatformError {
                code: error_codes::UNIMPLEMENTED.to_owned(),
                message: Some(
                    "no host handler for plugins.flutter.io/camera/takePicture".to_owned()
                ),
                details: Value::Null,
            }
        );
        let encoded = encode_envelope(&reply).unwrap().unwrap();
        assert_eq!(decode_envelope(Some(&encoded)).unwrap(), reply);
    }

    #[test]
    fn standard_and_json_codecs_stay_independent() {
        let call = MethodCall {
            method: "get".to_owned(),
            arguments: Value::I32(3),
        };
        let binary = encode_method_call(&call).unwrap();
        assert_eq!(decode_method_call(&binary).unwrap(), call);
        let json = encode_json_method_call(&call).unwrap();
        assert_eq!(decode_json_method_call(&json).unwrap(), call);
        let reply = Reply::Success(Value::String("ok".into()));
        let reply_bytes = encode_envelope(&reply).unwrap();
        assert_eq!(decode_envelope(reply_bytes.as_deref()).unwrap(), reply);
    }

    #[test]
    fn lifecycle_wire_round_trips_all_five_states() {
        let states = [
            Lifecycle::Resumed,
            Lifecycle::Inactive,
            Lifecycle::Hidden,
            Lifecycle::Paused,
            Lifecycle::Destroyed,
        ];
        for state in states {
            let bytes = encode_lifecycle_message(state);
            assert_eq!(parse_lifecycle_message(&bytes).unwrap(), state);
            assert_eq!(lifecycle_message(state), String::from_utf8(bytes).unwrap());
        }
        assert_eq!(
            parse_lifecycle_message(b"AppLifecycleState.unknown"),
            Err(CodecError::Corrupt("unknown lifecycle message"))
        );
        assert!(lifecycle_frames_enabled(Lifecycle::Resumed));
        assert!(!lifecycle_frames_enabled(Lifecycle::Hidden));
        assert_eq!(
            lifecycle_transitions(Some(Lifecycle::Resumed), Lifecycle::Paused),
            vec![Lifecycle::Inactive, Lifecycle::Hidden, Lifecycle::Paused]
        );
    }

    #[test]
    fn json_edge_inputs_are_rejected_or_preserved() {
        // Integers outside i64 are not Dart ints; never fabricate a value.
        // serde_json maps an unsigned value beyond i64 into f64, matching
        // Dart's json.decoder number fallback rather than fabricating an int.
        assert_eq!(
            decode_json_message(br#"18446744073709551616"#),
            Ok(Value::F64(18446744073709551616.0))
        );
        assert_eq!(
            decode_json_message(br#"1e400"#),
            Err(CodecError::Corrupt("invalid JSON message"))
        );
        // A non-string object key cannot map to Dart's String key type.
        let mut object = JsonMap::new();
        object.insert("nested".to_owned(), JsonValue::Number(JsonNumber::from(7)));
        assert_eq!(
            json_to_value(&JsonValue::Object({
                let mut outer = JsonMap::new();
                outer.insert("outer".to_owned(), JsonValue::Object(object));
                outer
            })),
            Ok(Value::Map(BTreeMap::from([(
                Value::String("outer".to_owned()),
                Value::Map(BTreeMap::from([(
                    Value::String("nested".to_owned()),
                    Value::I32(7)
                )]))
            )])))
        );
        assert_eq!(
            decode_json_method_call(br#"[1,2]"#),
            Err(CodecError::Corrupt("expected method call map"))
        );
        assert_eq!(
            decode_json_envelope(br#"{"success":true}"#),
            Err(CodecError::Corrupt("expected envelope list"))
        );
        assert_eq!(
            decode_json_envelope(br#"["code",123,null]"#),
            Err(CodecError::Corrupt("invalid envelope message"))
        );
    }

    #[test]
    fn lifecycle_rejects_non_utf8_and_empty_payloads() {
        assert_eq!(
            parse_lifecycle_message(&[0xff]),
            Err(CodecError::Corrupt("lifecycle message is not UTF-8"))
        );
        assert_eq!(
            parse_lifecycle_message(b""),
            Err(CodecError::Corrupt("unknown lifecycle message"))
        );
    }

    #[test]
    fn haptic_types_fold_into_one_channel_method() {
        for name in [
            None,
            Some(platform_methods::HAPTIC_LIGHT_IMPACT),
            Some(platform_methods::HAPTIC_SUCCESS_NOTIFICATION),
        ] {
            assert_eq!(haptic_method(name), Some(platform_methods::HAPTIC_VIBRATE));
        }
        assert_eq!(haptic_method(Some("unknown")), None);
        assert_eq!(platform_methods::CLIPBOARD_TEXT_PLAIN, "text/plain");
        assert_eq!(SYSTEM_EVENT_MEMORY_PRESSURE, "memoryPressure");
    }
}
