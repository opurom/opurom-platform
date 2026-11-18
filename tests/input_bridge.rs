//! Byte-level contract tests for the host-side input bridge: every portable
//! field of one renderer pointer call, the single logical clock, cancel de-dup
//! across lifecycle transitions, and the deadline-to-wait conversion.

use opurom_platform::input_bridge::{
    wait_from_deadline, Clock, ClockError, Feed, Gesture, HostPointer, InputRouter, InputSink,
    PointerKind,
};
use opurom_platform::{InputEvent, Lifecycle, PointerState, Size};

#[derive(Default)]
struct Recorder {
    pointers: Vec<HostPointer>,
    ticks: Vec<i64>,
    deadline: i64,
}

impl InputSink for Recorder {
    fn pointer(&mut self, event: HostPointer) -> Result<Gesture, i32> {
        self.pointers.push(event);
        Ok(Gesture {
            code: 0,
            node_id: event.node_id,
        })
    }

    fn next_deadline_ms(&self) -> i64 {
        self.deadline
    }

    fn tick(&mut self, t_ms: i64) -> Result<Gesture, i32> {
        self.ticks.push(t_ms);
        Ok(Gesture {
            code: 0,
            node_id: 0,
        })
    }
}

fn tap(x: f32, y: f32, node_id: u32) -> InputEvent {
    InputEvent::Tap {
        x,
        y,
        node_id,
        pointer: PointerState::default(),
    }
}

#[test]
fn tap_maps_every_bridge_field() {
    let mut sink = Recorder::default();
    sink.deadline = 600;
    let mut router = InputRouter::new();
    assert_eq!(
        router.feed(&tap(120.9, 7.8, 42), 100, &mut sink),
        Ok(Feed::Pointer)
    );
    assert_eq!(
        sink.pointers,
        vec![HostPointer {
            kind: PointerKind::Down,
            pointer_id: 0,
            node_id: 42,
            time_ms: 100,
            x: 120,
            y: 7,
        }]
    );
    assert_eq!(sink.next_deadline_ms(), 600);
    assert_eq!(sink.tick(600).unwrap().code, 0);
    assert_eq!(sink.ticks, vec![600]);
}

#[test]
fn move_drag_up_share_one_clock_and_pointer_id() {
    let mut sink = Recorder::default();
    let mut router = InputRouter::new();
    router.feed(&tap(10.0, 20.0, 1), 100, &mut sink).unwrap();
    router
        .feed(
            &InputEvent::PointerMove {
                x: 11.0,
                y: 21.0,
                pointer: PointerState::default(),
            },
            110,
            &mut sink,
        )
        .unwrap();
    router
        .feed(
            &InputEvent::PointerDrag {
                x: 12.0,
                y: 22.0,
                pointer: PointerState::default(),
            },
            120,
            &mut sink,
        )
        .unwrap();
    router
        .feed(
            &InputEvent::PointerUp {
                x: 13.0,
                y: 23.0,
                pointer: PointerState::default(),
            },
            130,
            &mut sink,
        )
        .unwrap();
    let kinds: Vec<PointerKind> = sink.pointers.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        vec![
            PointerKind::Down,
            PointerKind::Move,
            PointerKind::Move,
            PointerKind::Up
        ]
    );
    assert!(sink
        .pointers
        .iter()
        .all(|p| p.pointer_id == 0 && p.node_id != 1 || p.kind == PointerKind::Down));
    let times: Vec<i64> = sink.pointers.iter().map(|p| p.time_ms).collect();
    assert_eq!(times, [100, 110, 120, 130]);
    assert!(router
        .feed(&tap(14.0, 24.0, 2), 129, &mut sink)
        .is_err_and(|error| error
            == ClockError::NonMonotonic {
                previous: Some(130),
                now: 129
            }));
    assert_eq!(
        sink.pointers.len(),
        4,
        "a rejected time never reaches the sink"
    );
}

#[test]
fn focus_loss_emits_one_cancel_and_refocus_does_not() {
    let mut sink = Recorder::default();
    let mut router = InputRouter::new();
    router
        .feed(&InputEvent::Lifecycle(Lifecycle::Resumed), 10, &mut sink)
        .unwrap();
    router.feed(&tap(48.0, 49.0, 7), 100, &mut sink).unwrap();
    assert_eq!(
        router.feed(&InputEvent::Lifecycle(Lifecycle::Inactive), 200, &mut sink),
        Ok(Feed::Cancelled)
    );
    assert_eq!(
        sink.pointers.last().unwrap(),
        &HostPointer {
            kind: PointerKind::Cancel,
            pointer_id: 0,
            node_id: 0,
            time_ms: 200,
            x: 48,
            y: 49,
        }
    );
    assert_eq!(
        router.feed(&InputEvent::Lifecycle(Lifecycle::Inactive), 210, &mut sink),
        Ok(Feed::Ignored),
        "duplicate focus loss must not cancel twice"
    );
    router
        .feed(&InputEvent::Lifecycle(Lifecycle::Resumed), 220, &mut sink)
        .unwrap();
    assert_eq!(
        router.feed(&InputEvent::Lifecycle(Lifecycle::Inactive), 230, &mut sink),
        Ok(Feed::Ignored),
        "refocus without a new down has nothing to cancel"
    );
    assert_eq!(sink.pointers.len(), 2);
}

#[test]
fn portable_cancel_variant_uses_its_own_coordinates() {
    let mut sink = Recorder::default();
    let mut router = InputRouter::new();
    router.feed(&tap(10.0, 10.0, 3), 100, &mut sink).unwrap();
    assert_eq!(
        router.feed(
            &InputEvent::PointerCancel { x: 9.0, y: 8.0 },
            110,
            &mut sink
        ),
        Ok(Feed::Cancelled)
    );
    let cancel = sink.pointers.last().unwrap();
    assert_eq!(
        (cancel.kind, cancel.x, cancel.y, cancel.time_ms),
        (PointerKind::Cancel, 9, 8, 110)
    );
}

#[test]
fn clock_rejects_negative_and_backward_until_renderer_reset() {
    let mut clock = Clock::default();
    assert_eq!(
        clock.accept(-1),
        Err(ClockError::NonMonotonic {
            previous: None,
            now: -1
        })
    );
    assert_eq!(clock.accept(50), Ok(50));
    assert_eq!(
        clock.accept(49),
        Err(ClockError::NonMonotonic {
            previous: Some(50),
            now: 49
        })
    );
    assert_eq!(clock.accept(50), Ok(50), "rejection leaves the clock armed");
    clock.reset();
    assert_eq!(clock.accept(0), Ok(0));
}

#[test]
fn renderer_reset_clears_the_cancel_cursor() {
    let mut sink = Recorder::default();
    let mut router = InputRouter::new();
    router
        .feed(&InputEvent::Lifecycle(Lifecycle::Resumed), 10, &mut sink)
        .unwrap();
    router.feed(&tap(1.0, 2.0, 1), 20, &mut sink).unwrap();
    router.reset_renderer();
    assert_eq!(
        router.feed(&InputEvent::Lifecycle(Lifecycle::Inactive), 15, &mut sink),
        Ok(Feed::Ignored)
    );
    assert_eq!(sink.pointers.len(), 1);
}

#[test]
fn deadline_to_wait_keeps_sentinel_and_saturation_distinct() {
    assert_eq!(wait_from_deadline(-1, 100, 40), None);
    assert_eq!(wait_from_deadline(i64::MAX, 100, 40), Some(40));
    assert_eq!(wait_from_deadline(600, 100, 40), Some(40));
    assert_eq!(wait_from_deadline(130, 100, 40), Some(30));
    assert_eq!(wait_from_deadline(90, 100, 40), Some(0));
}

#[test]
fn unrelated_events_are_ignored() {
    let mut sink = Recorder::default();
    let mut router = InputRouter::new();
    for event in [
        InputEvent::Wheel {
            x: 1.0,
            y: 2.0,
            delta: 1.0,
            delta_x: 0.0,
            pointer: PointerState::default(),
        },
        InputEvent::Resize {
            size: Size {
                width: 10,
                height: 10,
            },
        },
        InputEvent::Key {
            code: 1,
            pressed: true,
        },
    ] {
        assert_eq!(router.feed(&event, 1, &mut sink), Ok(Feed::Ignored));
    }
    assert!(sink.pointers.is_empty());
}
