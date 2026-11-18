//! Per-channel codec contract for the bridged Flutter framework channels.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`. Every file:line below refers
//! to that checkout.
//!
//! `system_channels.dart` declares every framework channel together with the
//! codec it uses. The codec decides the wire format, so a bridge that
//! answers on the right channel name with the wrong codec is not
//! interoperable with the framework. This module freezes that pairing in a
//! compile-time table and routes every channel payload through it, so the
//! name, the channel kind and the codec can no longer drift apart.
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
//! - `packages/flutter/lib/src/services/system_channels.dart:423` —
//!   `flutter/platform_views` is a `MethodChannel`; its default codec is
//!   `StandardMethodCodec` (`services/platform_channel.dart:291-299`).
//! - `packages/flutter/lib/src/services/system_channels.dart:430` —
//!   `flutter/platform_views_2` is a `MethodChannel` with the default codec.
//! - `packages/flutter/lib/src/services/platform_channel.dart:622-633` — an
//!   `OptionalMethodChannel` answers `null` where a `MethodChannel` raises
//!   `MissingPluginException`; the `optional` flag of the contract carries
//!   that distinction.
//! - `packages/flutter/lib/src/services/platform_channel.dart:202-256` — a
//!   `BasicMessageChannel` sends `codec.encodeMessage(message)`, and a null
//!   message is sent as no bytes at all.
//! - `packages/flutter/lib/src/services/message_codecs.dart:44-62` —
//!   `StringCodec` is raw UTF-8 and maps null to no bytes.
//! - `packages/flutter/lib/src/services/message_codecs.dart:88-114` —
//!   `JSONMessageCodec` uses `JsonUtf8Encoder` and
//!   `utf8.decoder.fuse(json.decoder)`; a null message is no bytes.
//! - `packages/flutter/lib/src/services/message_codecs.dart:120-200` —
//!   `JSONMethodCodec` calls are two-key maps, replies are one-element
//!   success lists or three/four-element error lists.
//! - `packages/flutter/lib/src/services/message_codecs.dart:263-324,580-596`
//!   — the standard message and method codecs.
//!
//! There is no runtime registry: the contract is a `const` slice and a lookup
//! is a scan over a closed five-entry list. Nothing here touches the IR, the
//! renderer or the interpreter.

use crate::framework_channels::{
    decode_json_envelope, decode_json_message, decode_json_method_call, encode_json_envelope,
    encode_json_message, encode_json_method_call,
};
use crate::platform_channel::{
    decode_envelope, decode_message, decode_method_call, encode_envelope, encode_message,
    encode_method_call, system_channels, ChannelKind, CodecError, MethodCall, Reply, Value,
};

/// The codec a Flutter channel uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChannelCodec {
    /// `StandardMessageCodec` / `StandardMethodCodec`
    /// (`message_codecs.dart:263-324,580-596`).
    Standard,
    /// `JSONMessageCodec` / `JSONMethodCodec`
    /// (`message_codecs.dart:88-114,120-200`).
    Json,
    /// `StringCodec` (`message_codecs.dart:44-62`).
    String,
}

impl ChannelCodec {
    /// Stable name, used by the audit and diagnostic output.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelCodec::Standard => "standard",
            ChannelCodec::Json => "json",
            ChannelCodec::String => "string",
        }
    }
}

/// The official declaration of one bridged Flutter channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelContract {
    /// The channel name as Flutter declares it.
    pub name: &'static str,
    /// The channel kind: a basic message channel or a method channel.
    pub kind: ChannelKind,
    /// Whether a method channel is optional. An optional channel answers
    /// `null` where a strict one raises `MissingPluginException`
    /// (`platform_channel.dart:622-633`).
    pub optional: bool,
    /// The codec the channel carries.
    pub codec: ChannelCodec,
}

/// The bridged framework channels, in Flutter's declaration order.
pub const BRIDGED_CHANNELS: &[ChannelContract] = &[
    // `services/system_channels.dart:385-388`
    ChannelContract {
        name: system_channels::LIFECYCLE,
        kind: ChannelKind::BasicMessage,
        optional: false,
        codec: ChannelCodec::String,
    },
    // `services/system_channels.dart:401-404`
    ChannelContract {
        name: system_channels::SYSTEM,
        kind: ChannelKind::BasicMessage,
        optional: false,
        codec: ChannelCodec::Json,
    },
    // `services/system_channels.dart:181-184`
    ChannelContract {
        name: system_channels::PLATFORM,
        kind: ChannelKind::Method { optional: true },
        optional: true,
        codec: ChannelCodec::Json,
    },
    // `services/system_channels.dart:423`
    ChannelContract {
        name: system_channels::PLATFORM_VIEWS,
        kind: ChannelKind::Method { optional: false },
        optional: false,
        codec: ChannelCodec::Standard,
    },
    // `services/system_channels.dart:430`
    ChannelContract {
        name: system_channels::PLATFORM_VIEWS_2,
        kind: ChannelKind::Method { optional: false },
        optional: false,
        codec: ChannelCodec::Standard,
    },
];

/// Look up the contract of one bridged channel.
pub fn contract(name: &str) -> Option<&'static ChannelContract> {
    BRIDGED_CHANNELS.iter().find(|entry| entry.name == name)
}

/// The contract of a basic message channel.
fn require_basic_message(name: &str) -> Result<&'static ChannelContract, CodecError> {
    let entry = contract(name).ok_or(CodecError::Unsupported("unknown framework channel"))?;
    if entry.kind != ChannelKind::BasicMessage {
        return Err(CodecError::Unsupported(
            "channel is not a basic message channel",
        ));
    }
    Ok(entry)
}

/// The contract of a method channel.
fn require_method(name: &str) -> Result<&'static ChannelContract, CodecError> {
    let entry = contract(name).ok_or(CodecError::Unsupported("unknown framework channel"))?;
    if !matches!(entry.kind, ChannelKind::Method { .. }) {
        return Err(CodecError::Unsupported("channel is not a method channel"));
    }
    Ok(entry)
}

/// Encode a message for a basic message channel with its official codec.
///
/// A null message is sent as no bytes, which is what every Flutter codec
/// does (`message_codecs.dart:56-61,99-104`).
pub fn encode_channel_message(name: &str, value: &Value) -> Result<Option<Vec<u8>>, CodecError> {
    let entry = require_basic_message(name)?;
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    match entry.codec {
        ChannelCodec::Standard => encode_message(value),
        ChannelCodec::Json => encode_json_message(value).map(Some),
        ChannelCodec::String => {
            let Value::String(text) = value else {
                return Err(CodecError::Unsupported(
                    "string codec payload is not a string",
                ));
            };
            Ok(Some(text.as_bytes().to_vec()))
        }
    }
}

/// Decode a message from a basic message channel with its official codec.
///
/// No bytes decode to a null message, matching every Flutter codec
/// (`message_codecs.dart:48-53,106-112`).
pub fn decode_channel_message(name: &str, bytes: Option<&[u8]>) -> Result<Value, CodecError> {
    let entry = require_basic_message(name)?;
    let Some(bytes) = bytes else {
        return Ok(Value::Null);
    };
    match entry.codec {
        ChannelCodec::Standard => decode_message(Some(bytes)),
        ChannelCodec::Json => decode_json_message(bytes),
        ChannelCodec::String => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| CodecError::Corrupt("invalid UTF-8"))?;
            Ok(Value::String(text.to_owned()))
        }
    }
}

/// Encode a method call for a method channel with its official codec.
pub fn encode_channel_method_call(name: &str, call: &MethodCall) -> Result<Vec<u8>, CodecError> {
    let entry = require_method(name)?;
    match entry.codec {
        ChannelCodec::Standard => encode_method_call(call),
        ChannelCodec::Json => encode_json_method_call(call),
        ChannelCodec::String => Err(CodecError::Unsupported(
            "string codec carries no method calls",
        )),
    }
}

/// Decode a method call from a method channel with its official codec.
pub fn decode_channel_method_call(name: &str, bytes: &[u8]) -> Result<MethodCall, CodecError> {
    let entry = require_method(name)?;
    match entry.codec {
        ChannelCodec::Standard => decode_method_call(bytes),
        ChannelCodec::Json => decode_json_method_call(bytes),
        ChannelCodec::String => Err(CodecError::Unsupported(
            "string codec carries no method calls",
        )),
    }
}

/// Encode a reply envelope for a method channel with its official codec.
///
/// A missing plugin is a null reply, not an envelope, in every Flutter
/// codec (`platform_channel.dart:359-364`).
pub fn encode_channel_envelope(name: &str, reply: &Reply) -> Result<Option<Vec<u8>>, CodecError> {
    let entry = require_method(name)?;
    if matches!(reply, Reply::MissingPlugin) {
        return Ok(None);
    }
    match entry.codec {
        ChannelCodec::Standard => encode_envelope(reply),
        ChannelCodec::Json => encode_json_envelope(reply).map(Some),
        ChannelCodec::String => Err(CodecError::Unsupported(
            "string codec carries no reply envelopes",
        )),
    }
}

/// Decode a reply envelope from a method channel with its official codec.
pub fn decode_channel_envelope(name: &str, bytes: Option<&[u8]>) -> Result<Reply, CodecError> {
    let entry = require_method(name)?;
    match entry.codec {
        ChannelCodec::Standard => decode_envelope(bytes),
        ChannelCodec::Json => match bytes {
            None => Ok(Reply::MissingPlugin),
            Some(bytes) => decode_json_envelope(bytes),
        },
        ChannelCodec::String => Err(CodecError::Unsupported(
            "string codec carries no reply envelopes",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_channel::Value;

    #[test]
    fn contract_table_matches_flutter_declarations() {
        assert_eq!(BRIDGED_CHANNELS.len(), 5);
        let mut names: Vec<&str> = BRIDGED_CHANNELS.iter().map(|entry| entry.name).collect();
        let sorted = {
            let mut copy = names.clone();
            copy.sort();
            copy.dedup();
            copy
        };
        assert_eq!(sorted.len(), names.len(), "channel names must be unique");
        names.dedup();
        assert_eq!(names.len(), 5);

        let lifecycle = contract(system_channels::LIFECYCLE).expect("lifecycle channel");
        assert_eq!(lifecycle.codec, ChannelCodec::String);
        assert_eq!(lifecycle.kind, ChannelKind::BasicMessage);

        let system = contract(system_channels::SYSTEM).expect("system channel");
        assert_eq!(system.codec, ChannelCodec::Json);
        assert_eq!(system.kind, ChannelKind::BasicMessage);

        let platform = contract(system_channels::PLATFORM).expect("platform channel");
        assert_eq!(platform.codec, ChannelCodec::Json);
        assert_eq!(platform.kind, ChannelKind::Method { optional: true });
        assert!(platform.optional);

        for name in [
            system_channels::PLATFORM_VIEWS,
            system_channels::PLATFORM_VIEWS_2,
        ] {
            let entry = contract(name).expect("platform view channel");
            assert_eq!(entry.codec, ChannelCodec::Standard);
            assert_eq!(entry.kind, ChannelKind::Method { optional: false });
            assert!(!entry.optional);
        }
    }

    #[test]
    fn unknown_and_mismatched_channels_are_typed_rejections() {
        assert_eq!(
            encode_channel_message("flutter/unknown", &Value::Null),
            Err(CodecError::Unsupported("unknown framework channel"))
        );
        assert_eq!(
            encode_channel_method_call(
                system_channels::SYSTEM,
                &MethodCall {
                    method: "x".to_owned(),
                    arguments: Value::Null,
                }
            ),
            Err(CodecError::Unsupported("channel is not a method channel"))
        );
        assert_eq!(
            encode_channel_message(system_channels::PLATFORM, &Value::Null),
            Err(CodecError::Unsupported(
                "channel is not a basic message channel"
            ))
        );
    }

    #[test]
    fn lifecycle_channel_uses_the_string_codec() {
        let bytes = encode_channel_message(
            system_channels::LIFECYCLE,
            &Value::String("AppLifecycleState.resumed".to_owned()),
        )
        .expect("lifecycle message encodes")
        .expect("a string message has bytes");
        assert_eq!(bytes, b"AppLifecycleState.resumed".to_vec());
        assert_eq!(
            decode_channel_message(system_channels::LIFECYCLE, Some(&bytes)),
            Ok(Value::String("AppLifecycleState.resumed".to_owned()))
        );
        // A non-string payload cannot be carried by `StringCodec`.
        assert_eq!(
            encode_channel_message(system_channels::LIFECYCLE, &Value::I32(1)),
            Err(CodecError::Unsupported(
                "string codec payload is not a string"
            ))
        );
    }

    #[test]
    fn system_channel_uses_the_json_message_codec() {
        let value = Value::Map(std::collections::BTreeMap::from([(
            Value::String("type".to_owned()),
            Value::String("memoryPressure".to_owned()),
        )]));
        let bytes = encode_channel_message(system_channels::SYSTEM, &value)
            .expect("system message encodes")
            .expect("a map message has bytes");
        assert_eq!(bytes, br#"{"type":"memoryPressure"}"#.to_vec());
        assert_eq!(
            decode_channel_message(system_channels::SYSTEM, Some(&bytes)),
            Ok(value)
        );
    }

    #[test]
    fn method_channels_use_their_own_codecs() {
        let call = MethodCall {
            method: "Clipboard.setData".to_owned(),
            arguments: Value::Map(std::collections::BTreeMap::from([(
                Value::String("text".to_owned()),
                Value::String("hello".to_owned()),
            )])),
        };
        let json = encode_channel_method_call(system_channels::PLATFORM, &call)
            .expect("platform method call encodes");
        assert!(json.starts_with(b"{"));
        assert_eq!(
            decode_channel_method_call(system_channels::PLATFORM, &json),
            Ok(call.clone())
        );

        let binary = encode_channel_method_call(system_channels::PLATFORM_VIEWS, &call)
            .expect("platform view method call encodes");
        assert_eq!(binary[0], 7, "the first byte is the string tag");
        assert_eq!(
            decode_channel_method_call(system_channels::PLATFORM_VIEWS, &binary),
            Ok(call)
        );
    }

    #[test]
    fn envelopes_follow_the_channel_codec() {
        let success = Reply::Success(Value::I32(1));
        let json = encode_channel_envelope(system_channels::PLATFORM, &success)
            .expect("json envelope encodes")
            .expect("a success envelope has bytes");
        assert_eq!(json, b"[1]".to_vec());
        assert_eq!(
            decode_channel_envelope(system_channels::PLATFORM, Some(&json)),
            Ok(success.clone())
        );

        let binary = encode_channel_envelope(system_channels::PLATFORM_VIEWS, &success)
            .expect("binary envelope encodes")
            .expect("a success envelope has bytes");
        assert_eq!(binary[0], 0, "the first byte distinguishes success");
        assert_eq!(
            decode_channel_envelope(system_channels::PLATFORM_VIEWS, Some(&binary)),
            Ok(success)
        );
    }

    #[test]
    fn a_missing_plugin_is_a_null_reply_on_every_method_channel() {
        for name in [
            system_channels::PLATFORM,
            system_channels::PLATFORM_VIEWS,
            system_channels::PLATFORM_VIEWS_2,
        ] {
            assert_eq!(
                encode_channel_envelope(name, &Reply::MissingPlugin),
                Ok(None),
                "{name} must answer a null reply"
            );
            assert_eq!(
                decode_channel_envelope(name, None),
                Ok(Reply::MissingPlugin)
            );
        }
    }
}
