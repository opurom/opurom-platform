//! Runtime contract for host-injected time, deadline and state values.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/gestures/events.dart:175-176` and
//!   `packages/flutter/lib/src/gestures/binding.dart:100-104` — pointer event
//!   timestamps are supplied by the host and inspected for ordering; the
//!   framework never invents a wall clock.
//! - `packages/flutter/lib/src/scheduler/binding.dart:416-430,878` — frame
//!   enablement is a pure function of the injected lifecycle state.
//! - `packages/flutter/lib/src/services/binding.dart:174-182,296-311,313-381,448-457`
//!   — system/lifecycle payloads are value-driven and reject unknown strings.
//! - `packages/flutter/lib/src/services/binding.dart:113-122` and
//!   `services/_background_isolate_binary_messenger_io.dart:45-63,76-96` —
//!   background messaging uses a root token and a monotonic message id.
//!
//! No ABI/IR definition changes here: the tests exercise existing pure
//! contracts with injected values only.

use opurom_platform::background_tasks::{BackgroundMessenger, RootToken};
use opurom_platform::event_producers::refresh_rate;
use opurom_platform::input_bridge::{Clock, ClockError};
use opurom_platform::platform_channel::CodecError;
use opurom_platform::system_events::{Subscriber, SystemEvent, SystemEventDispatcher};

unsafe fn record(state: *mut u8, event: &SystemEvent) {
    // SAFETY: state points to a live Vec<SystemEvent> owned by each test.
    unsafe { &mut *(state as *mut Vec<SystemEvent>) }.push(event.clone());
}

#[test]
fn injected_timestamps_wire_round_trip_and_non_monotonic_values_are_rejected() {
    let mut clock = Clock::default();
    for stamp in [0i64, 1, 16, 1000, i64::MAX] {
        assert_eq!(clock.accept(stamp), Ok(stamp));
    }
    // A duplicate is accepted (monotonic, not strictly increasing), but a
    // rollback or a negative host timestamp is rejected without state change.
    assert_eq!(clock.accept(i64::MAX), Ok(i64::MAX));
    assert_eq!(
        clock.accept(999),
        Err(ClockError::NonMonotonic {
            previous: Some(i64::MAX),
            now: 999,
        })
    );
    assert_eq!(
        clock.accept(-1),
        Err(ClockError::NonMonotonic {
            previous: Some(i64::MAX),
            now: -1,
        })
    );
    // Reset is explicit and only valid with a rebuilt renderer.
    clock.reset();
    assert_eq!(clock.accept(0), Ok(0));
}

#[test]
fn background_deadline_and_state_wire_correlate_by_injected_sequence() {
    let mut messenger = BackgroundMessenger::new();
    assert!(messenger.send("before", vec![1]).is_err());
    let token = RootToken::new(42).unwrap();
    assert!(messenger.ensure_initialized(token));
    assert!(!messenger.ensure_initialized(RootToken(99)));
    assert_eq!(messenger.token(), Some(token));

    let first = messenger.send("a.channel", vec![1, 2]).unwrap();
    let second = messenger.send("b.channel", vec![3, 4]).unwrap();
    assert_eq!((first, second), (1, 2));
    assert_eq!(messenger.pending()[&first].channel, "a.channel");
    assert_eq!(messenger.pending()[&second].message, vec![3, 4]);

    // Out-of-order replies converge to the original payload correlation.
    assert_eq!(messenger.reply(second, vec![40]), Ok(()));
    assert_eq!(messenger.reply(first, vec![10]), Ok(()));
    assert_eq!(messenger.take_reply(first), Some(vec![10]));
    assert_eq!(messenger.take_reply(second), Some(vec![40]));
    assert!(messenger.pending().is_empty());

    assert_eq!(
        messenger.reply(999, vec![]),
        Err(CodecError::Corrupt("unknown background message identifier"))
    );
    assert_eq!(messenger.take_reply(first), None);
}

#[test]
fn injected_state_changes_drive_dedupe_and_frame_gate_in_order() {
    let mut log = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new().on(
        refresh_rate(60),
        Subscriber {
            function: record,
            state: &mut log as *mut Vec<SystemEvent> as *mut u8,
        },
    );

    // SAFETY: `log` outlives every dispatcher call below.
    let sequence = [30u32, 120, 60, 120, 144, 60];
    for hz in sequence {
        assert!(unsafe { dispatcher.emit_host_event(refresh_rate(hz)) });
    }
    assert_eq!(
        log,
        sequence
            .iter()
            .copied()
            .map(refresh_rate)
            .collect::<Vec<_>>()
    );

    // Reset models a fresh injected state source after cancellation.
    dispatcher.reset(refresh_rate(0).code());
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(60)) });
    assert_eq!(log.len(), sequence.len() + 1);

    let states = [
        (opurom_platform::Lifecycle::Resumed, true),
        (opurom_platform::Lifecycle::Inactive, true),
        (opurom_platform::Lifecycle::Hidden, false),
        (opurom_platform::Lifecycle::Paused, false),
        (opurom_platform::Lifecycle::Destroyed, false),
    ];
    for (state, frames_enabled) in states {
        assert_eq!(opurom_platform::frames_enabled(state), frames_enabled);
    }
}
