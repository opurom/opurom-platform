//! Deterministic codec-to-dispatch fuzz matrix for system events.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/message_codecs.dart:88-114` —
//!   `JSONMessageCodec` decodes UTF-8 JSON with `json.decoder`.
//! - `packages/flutter/lib/src/services/system_channels.dart:395-405` —
//!   `flutter/system` is a JSON `BasicMessageChannel` with `Map` payloads.
//! - `packages/flutter/lib/src/services/binding.dart:174-182` — the
//!   default-less switch on `type`; only `memoryPressure` is handled.
//! - `packages/flutter/lib/src/services/binding.dart:448-457` — the five
//!   accepted lifecycle strings; `:313-381` generates intermediate states.
//!
//! This matrix feeds bounded malformed bytes through the exact production
//! decode/dispatch entry points. It never changes ABI/IR and adds no runtime
//! state beyond test-local subscriber logs.

use opurom_platform::channel_event_bridge::dispatch_system_message;
use opurom_platform::platform_channel::Value;
use opurom_platform::system_events::{Subscriber, SystemEvent, SystemEventDispatcher};

unsafe fn record(state: *mut u8, event: &SystemEvent) {
    // SAFETY: state points to a live Vec<SystemEvent> owned by each test.
    unsafe { &mut *(state as *mut Vec<SystemEvent>) }.push(event.clone());
}

fn subscriber<'a>(log: &'a mut Vec<SystemEvent>) -> Subscriber {
    Subscriber {
        function: record,
        state: log as *mut Vec<SystemEvent> as *mut u8,
    }
}

#[test]
fn bounded_bytes_never_reach_a_subscriber_as_fake_events() {
    let mut log = Vec::new();
    let mut dispatcher =
        SystemEventDispatcher::new().on(SystemEvent::MemoryPressure, subscriber(&mut log));

    let mut corpus: Vec<Vec<u8>> = vec![
        Vec::new(),
        b" ".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"\"memoryPressure\"".to_vec(),
        b"{\"type\":1}".to_vec(),
        b"{\"type\":\"unknown\"}".to_vec(),
        b"{\"type\":\"memoryPressure\"}".to_vec(),
        b"{\"type\":\"memoryPressure\"} trailing".to_vec(),
    ];
    // Repeated bytes and prefix/suffix truncations of the one valid payload.
    let valid = br#"{"type":"memoryPressure"}"#;
    for byte in [0u8, b'{', b'}', b'"', b':', b',', 0xff] {
        corpus.push(vec![byte; 64]);
    }
    for cut in 0..valid.len() {
        corpus.push(valid[..cut].to_vec());
        corpus.push(valid[cut..].to_vec());
    }
    for bytes in corpus {
        // SAFETY: `log` outlives every call and is the only subscriber state.
        let _ = unsafe { dispatch_system_message(&mut dispatcher, &bytes) };
    }
    assert_eq!(log, vec![SystemEvent::MemoryPressure]);
    assert_eq!(log.len(), 1, "duplicate valid payload must dedupe");
}

#[test]
fn interleaved_valid_and_invalid_payloads_keep_dispatch_order() {
    let mut log = Vec::new();
    let mut dispatcher =
        SystemEventDispatcher::new().on(SystemEvent::MemoryPressure, subscriber(&mut log));
    let inputs: [&[u8]; 9] = [
        b"",
        br#"{"type":"memoryPressure"}"#,
        b"{\"type\":null}",
        b"{\"type\":\"unknown\"}",
        br#"{"type":"memoryPressure"}"#,
        b"[]",
        br#"{"type":"memoryPressure"}"#,
        b"{\"type\":\"memoryPressure\",\"extra\": ",
        br#"{"type":"memoryPressure"}"#,
    ];
    for bytes in inputs {
        // SAFETY: `log` outlives every call and is the only subscriber state.
        let _ = unsafe { dispatch_system_message(&mut dispatcher, bytes) };
    }
    assert_eq!(log, vec![SystemEvent::MemoryPressure]);
}

#[test]
fn unknown_type_codes_never_mutate_dedupe_state() {
    let mut log = Vec::new();
    let mut dispatcher =
        SystemEventDispatcher::new().on(SystemEvent::MemoryPressure, subscriber(&mut log));
    // SAFETY: `log` outlives every call.
    assert!(
        unsafe { dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure"}"#) }
            .unwrap()
    );
    for text in [br#"{"type":"unknown"}"#.as_slice()] {
        // Unknown framework codes are rejected before the dispatcher, so a
        // malformed payload can never occupy a dedupe slot.
        assert!(unsafe { dispatch_system_message(&mut dispatcher, text) }.is_err());
    }
    // A host event code is accepted by the shared taxonomy; it has no
    // framework subscriber here, so dispatch reports no delivery without
    // consuming the memoryPressure dedupe slot.
    assert!(!unsafe {
        dispatch_system_message(&mut dispatcher, br#"{"type":"host.refreshRate","hz":120}"#)
    }
    .unwrap());
    // Extra keys are ignored by the binding's type switch
    // (`binding.dart:174-182`); it still maps to memoryPressure and dedupes.
    assert!(!unsafe {
        dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure","bad":0}"#)
    }
    .unwrap());
    // Repeating the exact valid payload remains deduped.
    assert!(
        !unsafe { dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure"}"#) }
            .unwrap()
    );
    // Reset restores delivery without fabricating an event payload.
    dispatcher.reset(SystemEvent::MemoryPressure.code());
    assert!(
        unsafe { dispatch_system_message(&mut dispatcher, br#"{"type":"memoryPressure"}"#) }
            .unwrap()
    );
    assert_eq!(log.len(), 2);

    // The system channel payloads remain closed under the codec contract.
    let wrapped = Value::String("memoryPressure".to_owned());
    assert_ne!(wrapped, SystemEvent::MemoryPressure.to_value());
}
