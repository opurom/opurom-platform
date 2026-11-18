//! Deterministic subscription-slot semantics for the event dispatcher.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `services/platform_channel.dart:679-738` defines `EventChannel`:
//! `receiveBroadcastStream` installs one message handler in `onListen`
//! (`:696-708`), sends `listen` on 0→1 listeners (`:709-720`), removes the
//! handler in `onCancel` (`:722-724`) and sends `cancel` on 1→0 (`:725-735`).
//! `services/binary_messenger.dart:80,87-95` makes `setMessageHandler` a
//! replace-or-clear operation on one named slot. Opurom's compile-time
//! [`SystemEventDispatcher`] has the same slot semantics: one subscriber per
//! event code, replace on re-subscribe, reset on cancellation. These tests
//! pin the order/state convergence without adding an ABI or IR change.

use opurom_platform::event_producers::{keyboard_visibility, refresh_rate};
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
fn repeated_listen_replaces_one_slot_and_keeps_dispatch_order() {
    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new()
        .on(refresh_rate(60), subscriber(&mut first))
        .on(refresh_rate(60), subscriber(&mut second));
    assert_eq!(dispatcher.subscriber_count(), 1);

    let events = [refresh_rate(90), refresh_rate(120), refresh_rate(144)];
    for event in events.iter().cloned() {
        // SAFETY: `second` outlives every call and owns the active slot.
        assert!(unsafe { dispatcher.emit_host_event(event) });
    }
    assert_eq!(second, events);
    assert!(first.is_empty());
}

#[test]
fn reset_models_cancel_then_relisten_without_ghost_delivery() {
    let mut log = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new().on(refresh_rate(60), subscriber(&mut log));

    // SAFETY: `log` outlives every call.
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(120)) });

    // Cancel 1→0: the slot remains registered, but the last value is cleared
    // so a later 0→1 listen accepts an identical signal again.
    dispatcher.reset(refresh_rate(0).code());
    assert_eq!(dispatcher.subscriber_count(), 1);
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert_eq!(log.len(), 2);

    // An unknown reset is a no-op and must not create or disturb slots.
    dispatcher.reset("host.unknown");
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert_eq!(dispatcher.subscriber_count(), 1);
}

#[test]
fn interleaved_replace_and_reset_converge_to_last_slot() {
    let mut a = Vec::new();
    let mut b = Vec::new();
    let mut c = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new()
        .on(keyboard_visibility(true, None), subscriber(&mut a))
        .on(refresh_rate(60), subscriber(&mut b));

    // SAFETY: all log vectors outlive dispatcher calls.
    assert!(unsafe { dispatcher.emit_host_event(keyboard_visibility(true, Some(100))) });
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(90)) });

    // Re-subscribe refreshRate while keyboard stays on its original slot.
    dispatcher = dispatcher.on(refresh_rate(90), subscriber(&mut c));
    assert_eq!(dispatcher.subscriber_count(), 2);
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert_eq!(b, vec![refresh_rate(90)]);
    assert_eq!(c, vec![refresh_rate(120)]);
    assert_eq!(a, vec![keyboard_visibility(true, Some(100))]);

    dispatcher.reset(refresh_rate(0).code());
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
    assert_eq!(c.len(), 2);
    assert_eq!(a.len(), 1);
}

#[test]
fn clear_and_reinstall_a_slot_without_cross_kind_leakage() {
    let mut keyboard_log = Vec::new();
    let mut refresh_log = Vec::new();
    let mut dispatcher = SystemEventDispatcher::new()
        .on(
            keyboard_visibility(false, None),
            subscriber(&mut keyboard_log),
        )
        .on(refresh_rate(60), subscriber(&mut refresh_log));

    // Replace the refresh slot by re-adding a different code; the old code
    // must become unowned while all other slots remain intact.
    dispatcher = dispatcher.on(refresh_rate(60), subscriber(&mut refresh_log));
    assert_eq!(dispatcher.subscriber_count(), 2);

    // SAFETY: logs outlive the dispatcher.
    assert!(unsafe { dispatcher.emit_host_event(keyboard_visibility(true, Some(44))) });
    assert!(unsafe { dispatcher.emit_host_event(refresh_rate(75)) });
    assert_eq!(keyboard_log, vec![keyboard_visibility(true, Some(44))]);
    assert_eq!(refresh_log, vec![refresh_rate(75)]);
    assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(75)) });
}
