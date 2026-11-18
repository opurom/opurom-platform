//! Translate framework channel payloads into Opurom event values.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Two framework channels feed the binding directly:
//! `flutter/system` is a JSON `BasicMessageChannel` whose only documented
//! payload is `{"type":"memoryPressure"}`
//! (`services/system_channels.dart:395-405`, `services/binding.dart:174-182`),
//! and `flutter/lifecycle` is a `StringCodec` `BasicMessageChannel` whose
//! five strings are parsed in `services/binding.dart:303-311,448-457`.
//! This module decodes those exact payloads into the closed event types;
//! the lifecycle path reuses the shared transition synthesizer
//! (`services/binding.dart:313-381`). No handler registry, isolate or
//! runtime object tree is introduced.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:44-62` —
//!   `StringCodec` is raw UTF-8.
//! - `packages/flutter/lib/src/services/message_codecs.dart:88-114` —
//!   `JSONMessageCodec` is UTF-8 JSON.
//! - `packages/flutter/lib/src/services/system_channels.dart:385-405` —
//!   channel names and codecs.
//! - `packages/flutter/lib/src/services/binding.dart:174-182,303-311,448-457` —
//!   payload handling and lifecycle strings.

use crate::framework_channels::{decode_json_message, parse_lifecycle_message};
use crate::platform_channel::CodecError;
use crate::system_events::{SystemEvent, SystemEventDispatcher};
use crate::Lifecycle;

/// Decode one `flutter/system` JSON payload into a typed system event.
/// Unknown `type` values are rejected, matching Flutter's default-less switch
/// (`binding.dart:178-182`).
pub fn decode_system_message(bytes: &[u8]) -> Result<SystemEvent, CodecError> {
    let value = decode_json_message(bytes)?;
    SystemEvent::from_value(&value).ok_or(CodecError::Corrupt("unknown system event payload"))
}

/// Decode one `flutter/lifecycle` `StringCodec` payload.
pub fn decode_lifecycle_message(bytes: &[u8]) -> Result<Lifecycle, CodecError> {
    parse_lifecycle_message(bytes)
}

/// Generate the intermediate lifecycle states Flutter emits for one message
/// (`binding.dart:313-381`). An unchanged state produces an empty vector.
pub fn lifecycle_transitions_from_message(
    previous: Option<Lifecycle>,
    bytes: &[u8],
) -> Result<Vec<Lifecycle>, CodecError> {
    let next = decode_lifecycle_message(bytes)?;
    Ok(crate::framework_channels::lifecycle_transitions(
        previous, next,
    ))
}

/// Decode one `flutter/system` payload and dispatch it through the shared
/// dispatcher. Returns `true` when a subscriber received the event.
///
/// # Safety
/// Every subscriber state pointer in `dispatcher` must remain valid for this
/// call, as required by [`SystemEventDispatcher::emit`].
pub unsafe fn dispatch_system_message(
    dispatcher: &mut SystemEventDispatcher,
    bytes: &[u8],
) -> Result<bool, CodecError> {
    let event = decode_system_message(bytes)?;
    // SAFETY: documented by the caller.
    Ok(unsafe { dispatcher.emit(event) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework_channels::encode_lifecycle_message;
    use crate::system_events::Subscriber;

    unsafe fn collect(state: *mut u8, event: &SystemEvent) {
        // SAFETY: state points to a live Vec<SystemEvent> owned by the test.
        unsafe { &mut *(state as *mut Vec<SystemEvent>) }.push(event.clone());
    }

    #[test]
    fn system_message_decodes_the_flutter_payload_exactly() {
        let bytes = br#"{"type":"memoryPressure"}"#;
        assert_eq!(
            decode_system_message(bytes),
            Ok(SystemEvent::MemoryPressure)
        );
        assert_eq!(
            decode_system_message(br#"{"type":"unknown"}"#),
            Err(CodecError::Corrupt("unknown system event payload"))
        );
        assert_eq!(
            decode_system_message(br#"[]"#),
            Err(CodecError::Corrupt("unknown system event payload"))
        );
    }

    #[test]
    fn lifecycle_message_uses_flutter_intermediate_states() {
        let bytes = encode_lifecycle_message(Lifecycle::Paused);
        assert_eq!(decode_lifecycle_message(&bytes), Ok(Lifecycle::Paused));
        assert_eq!(
            lifecycle_transitions_from_message(Some(Lifecycle::Resumed), &bytes),
            Ok(vec![
                Lifecycle::Inactive,
                Lifecycle::Hidden,
                Lifecycle::Paused,
            ])
        );
        assert_eq!(
            lifecycle_transitions_from_message(Some(Lifecycle::Paused), &bytes),
            Ok(Vec::new())
        );
    }

    #[test]
    fn decoded_system_messages_reach_the_dispatcher() {
        let mut delivered = Vec::new();
        let mut dispatcher = SystemEventDispatcher::new().on(
            SystemEvent::MemoryPressure,
            Subscriber {
                function: collect,
                state: &mut delivered as *mut Vec<SystemEvent> as *mut u8,
            },
        );
        // SAFETY: `delivered` outlives this call and is the subscriber state.
        assert_eq!(
            unsafe { dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure"}"#) },
            Ok(true)
        );
        assert_eq!(delivered, vec![SystemEvent::MemoryPressure]);
        // The dispatcher dedupes repeat host signals of the same kind.
        assert_eq!(
            unsafe { dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure"}"#) },
            Ok(false)
        );
        assert_eq!(delivered.len(), 1);
    }
}
