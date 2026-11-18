//! Host-system-event producer and dedupe edge matrix.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:375-405` —
//!   `flutter/lifecycle` (`StringCodec`) and `flutter/system`
//!   (`JSONMessageCodec`, only `memoryPressure`).
//! - `packages/flutter/lib/src/services/binding.dart:174-182` —
//!   a default-less switch on the JSON `type` field.
//! - `packages/flutter/lib/src/services/binding.dart:303-311,448-457` —
//!   the five accepted lifecycle strings and rejection of everything else.
//! - `packages/flutter/lib/src/services/binding.dart:313-381` —
//!   generated intermediate transitions; illegal adjacency is guarded at
//!   `:360-381`.
//! - `packages/flutter/lib/src/services/system_chrome.dart:43-70` —
//!   the four-value `DeviceOrientation` index domain.
//!
//! The matrix is bounded and deterministic: no random values, no wall clock,
//! no ABI/IR change. Host-only events stay in the `host.*` namespace.

use std::collections::BTreeMap;

use opurom_platform::channel_event_bridge::decode_system_message;
use opurom_platform::event_producers::{keyboard_visibility, orientation_from_index, refresh_rate};
use opurom_platform::framework_channels::{lifecycle_transitions, parse_lifecycle_message};
use opurom_platform::platform_channel::{CodecError, Value};
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

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(k, v)| (Value::String((*k).to_owned()), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[test]
fn empty_unknown_and_malformed_payloads_are_rejected() {
    let payloads = [
        "",
        " ",
        "null",
        "[]",
        "{}",
        "{\"type\":1}",
        "{\"type\":\"\"}",
        "{\"type\":\"memoryPressure\",}",
        "{\"type\":\"unknown\"}",
        "{\"type\":\"Host.RefreshRate\",\"hz\":120}",
        "{\"type\":\"memoryPressure\"} trailing",
    ];
    for text in payloads {
        assert!(
            decode_system_message(text.as_bytes()).is_err(),
            "malformed payload accepted: {text}"
        );
    }
    let unknown = map(&[("type", Value::String("somethingElse".into()))]);
    assert_eq!(SystemEvent::from_value(&unknown), None);
    // A valid code with an invalid field type never fabricates an event.
    assert_eq!(
        SystemEvent::from_value(&map(&[
            ("type", Value::String("host.refreshRate".into())),
            ("hz", Value::String("120".into())),
        ])),
        None
    );
    assert_eq!(
        SystemEvent::from_value(&map(&[
            ("type", Value::String("host.orientation".into())),
            ("orientation", Value::I32(4)),
        ])),
        None
    );
}

#[test]
fn duplicate_out_of_order_and_long_inputs_keep_closed_domains() {
    let repeats = [
        map(&[
            ("type", Value::String("host.refreshRate".into())),
            ("hz", Value::I32(120)),
        ]),
        map(&[
            ("type", Value::String("host.refreshRate".into())),
            ("hz", Value::I32(120)),
        ]),
        map(&[
            ("type", Value::String("host.refreshRate".into())),
            ("hz", Value::I32(144)),
        ]),
        map(&[("type", Value::String("memoryPressure".into()))]),
        map(&[("type", Value::String("memoryPressure".into()))]),
    ];
    for (index, value) in repeats.iter().enumerate() {
        let event = SystemEvent::from_value(value).expect("code in closed set");
        let expected = if index < 3 {
            "host.refreshRate"
        } else {
            "memoryPressure"
        };
        assert_eq!(event.code(), expected);
    }
    for index in [3u8, 0, 255, 1, 4] {
        assert_eq!(
            orientation_from_index(index).map(|e| e.code().to_owned()),
            if index <= 3 {
                Some("host.orientation".to_owned())
            } else {
                None
            }
        );
    }
    // Oversize UTF-8 strings are values, not parser state; they round-trip
    // through the codec and still respect the closed type domain.
    let long = "x".repeat(4096);
    let value = map(&[("type", Value::String(long.clone()))]);
    assert_eq!(SystemEvent::from_value(&value), None);
    // Heights above i32::MAX are outside the codec's I32 field domain and
    // degrade to `None`, never to a truncated or negative inset.
    let long_height = keyboard_visibility(true, Some(u32::MAX));
    assert_eq!(
        SystemEvent::from_value(&long_height.to_value()),
        Some(keyboard_visibility(true, None))
    );
    assert_eq!(refresh_rate(u32::MAX).code(), "host.refreshRate");
}

#[test]
fn dedupe_and_reset_are_exact_and_slot_local() {
    let mut keyboard_log = Vec::new();
    let mut refresh_log = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new()
        .on(
            keyboard_visibility(true, None),
            subscriber(&mut keyboard_log),
        )
        .on(refresh_rate(60), subscriber(&mut refresh_log));

    // SAFETY: both logs outlive every dispatcher call below.
    assert!(unsafe { dispatcher.emit_host_event(keyboard_visibility(true, Some(240))) });
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert_eq!(keyboard_log, vec![keyboard_visibility(true, Some(240))]);
    assert_eq!(refresh_log, vec![refresh_rate(120)]);

    // Exact duplicates are dropped; a changed payload of the same kind is
    // delivered to the one slot registered for that kind.
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
    assert_eq!(refresh_log.len(), 2);
    dispatcher.reset("host.refreshRate");
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
    assert_eq!(refresh_log.len(), 3);
    // An unknown code reset must not disturb any live slot.
    dispatcher.reset("host.unknown");
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
    assert_eq!(dispatcher.subscriber_count(), 2);
}

#[test]
fn lifecycle_illegal_and_extreme_inputs_never_fabricate_states() {
    for text in [
        "",
        "AppLifecycleState",
        "AppLifecycleState.",
        "AppLifecycleState.Resumed",
        "AppLifecycleState.resumed ",
        "appLifecycleState.resumed",
        "AppLifecycleState.unknown",
    ] {
        assert_eq!(
            parse_lifecycle_message(text.as_bytes()),
            Err(CodecError::Corrupt("unknown lifecycle message")),
            "accepted invalid lifecycle string: {text}"
        );
    }
    assert_eq!(
        parse_lifecycle_message(&[0xff]),
        Err(CodecError::Corrupt("lifecycle message is not UTF-8"))
    );
    // An unchanged state yields no synthetic transitions; the five-state
    // domain is closed for every legal input.
    for state in [
        opurom_platform::Lifecycle::Resumed,
        opurom_platform::Lifecycle::Inactive,
        opurom_platform::Lifecycle::Hidden,
        opurom_platform::Lifecycle::Paused,
        opurom_platform::Lifecycle::Destroyed,
    ] {
        assert!(lifecycle_transitions(Some(state), state).is_empty());
    }
    // A long string never becomes a valid lifecycle message by truncation.
    let long = format!("AppLifecycleState.{}", "r".repeat(8192));
    assert!(parse_lifecycle_message(long.as_bytes()).is_err());
}
