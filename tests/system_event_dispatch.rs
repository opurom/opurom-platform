//! Deterministic system-event dispatch conformance.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:375-405` —
//!   `flutter/lifecycle` uses `StringCodec`; `flutter/system` uses
//!   `JSONMessageCodec` and documents only `memoryPressure`.
//! - `packages/flutter/lib/src/services/binding.dart:174-182` —
//!   `handleSystemMessage` switches on the JSON `type` field; `:303-311`
//!   parses lifecycle strings; `:448-457` is the closed five-state set.
//! - `packages/flutter/lib/src/scheduler/binding.dart:416-430` —
//!   frame enablement is true only for resumed/inactive.
//! - `packages/flutter/lib/src/services/system_chrome.dart:43-70` —
//!   `DeviceOrientation` index order.
//! - `packages/flutter/lib/src/services/binding.dart:113-122` —
//!   root isolate token / background coordination boundary.
//!
//! The host-only signals (keyboard, orientation, brightness, power, refresh
//! rate) are not Flutter framework broadcasts; Opurom reports them through
//! stable `host.*` codes and tests their ordering/dedup here without changing
//! the ABI.

use opurom_platform::event_producers::{
    brightness, keyboard_visibility, orientation_from_index, power_mode, refresh_rate,
};
use opurom_platform::framework_channels::{encode_json_message, parse_lifecycle_message};
use opurom_platform::platform_channel::Value;
use opurom_platform::system_events::{Subscriber, SystemEvent, SystemEventDispatcher};
use opurom_platform::Lifecycle;

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
fn host_event_codes_and_wire_values_are_stable() {
    let cases = [
        (
            keyboard_visibility(true, Some(320)),
            "host.keyboardVisibility",
            "keyboard",
        ),
        (
            orientation_from_index(1).unwrap(),
            "host.orientation",
            "orientation",
        ),
        (brightness(true), "host.brightness", "brightness"),
        (power_mode(false), "host.powerMode", "power"),
        (refresh_rate(120), "host.refreshRate", "refresh"),
    ];
    for (event, code, name) in cases {
        assert_eq!(event.code(), code, "{name} code drifted");
        assert!(!event.is_flutter_framework_event(), "{name} is host-only");
        let value = event.to_value();
        assert_eq!(SystemEvent::from_value(&value), Some(event));
        let encoded = encode_json_message(&value);
        assert!(encoded.is_ok(), "{name} value is not JSON encodable");
    }
    let memory = SystemEvent::MemoryPressure;
    assert_eq!(memory.code(), "memoryPressure");
    assert!(memory.is_flutter_framework_event());
    assert_eq!(
        memory.to_value(),
        Value::Map(
            [(
                Value::String("type".to_owned()),
                Value::String("memoryPressure".to_owned())
            )]
            .into_iter()
            .collect()
        )
    );
}

#[test]
fn dispatcher_keeps_host_event_order_and_replaces_one_handler_per_kind() {
    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new();
    for event in [
        keyboard_visibility(false, None),
        orientation_from_index(2).unwrap(),
        brightness(false),
        power_mode(true),
        refresh_rate(60),
    ] {
        dispatcher = dispatcher.on(event, subscriber(&mut first));
    }
    // Replace only the refresh-rate handler; other slots stay attached.
    dispatcher = dispatcher.on(refresh_rate(60), subscriber(&mut second));
    assert_eq!(dispatcher.subscriber_count(), 5);

    let events = [
        keyboard_visibility(false, None),
        orientation_from_index(2).unwrap(),
        brightness(false),
        power_mode(true),
        refresh_rate(120),
        refresh_rate(144),
    ];
    for event in events.iter().cloned() {
        // SAFETY: both log vectors outlive the dispatcher calls.
        assert!(unsafe { dispatcher.emit_host_event(event) });
    }
    assert_eq!(second, events[4..]);
    assert_eq!(first, events[..4]);

    // Same kind + same value is deduped; a reset models re-subscription.
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
    dispatcher.reset("host.refreshRate");
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
    assert_eq!(second.len(), 3);
    assert_eq!(first.len(), 4);
}

#[test]
fn lifecycle_messages_drive_the_same_state_and_frame_gate() {
    let messages = [
        ("AppLifecycleState.resumed", Lifecycle::Resumed, true),
        ("AppLifecycleState.inactive", Lifecycle::Inactive, true),
        ("AppLifecycleState.hidden", Lifecycle::Hidden, false),
        ("AppLifecycleState.paused", Lifecycle::Paused, false),
        ("AppLifecycleState.detached", Lifecycle::Destroyed, false),
    ];
    for (text, state, frames) in messages {
        let decoded = parse_lifecycle_message(text.as_bytes());
        assert_eq!(decoded, Ok(state), "{text} parse drifted");
        assert_eq!(opurom_platform::frames_enabled(state), frames);
    }
    assert!(parse_lifecycle_message(b"AppLifecycleState.unknown").is_err());

    let transitions = opurom_platform::framework_channels::lifecycle_transitions(
        Some(Lifecycle::Resumed),
        Lifecycle::Paused,
    );
    assert_eq!(
        transitions,
        vec![Lifecycle::Inactive, Lifecycle::Hidden, Lifecycle::Paused]
    );
}
