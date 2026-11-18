//! Host-side input bridge: maps platform pointer and lifecycle events onto a
//! renderer-facing sink without taking a dependency on the renderer crate.
//! The sink owns the C ABI layout (`opurom.h`); this module owns only the
//! portable decisions, so the mapping is testable on every host.
//!
//! The logical clock is caller-owned: the host feeds `now_ms` from its own
//! monotonic source, and the same value drives every pointer event and tick.
//! It advances monotonically for the renderer's whole lifetime; a state patch
//! or route change must never reset it, or the ABI rejects later input.

use crate::{app_lifecycle::transitions, InputEvent, Lifecycle};
use std::fmt;

/// Pointer kinds from `opurom.h:43-45`. Kept as a plain enum so no C header is
/// duplicated here; the sink maps it onto the ABI's `OpuPointerKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Down = 1,
    Move = 2,
    Up = 3,
    Cancel = 4,
}

/// One host pointer event in the portable bridge shape. `pointer_id` is 0 for
/// the single host pointer until the ABI grows stable multi-pointer ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostPointer {
    pub kind: PointerKind,
    pub pointer_id: u32,
    pub node_id: u32,
    pub time_ms: i64,
    pub x: i64,
    pub y: i64,
}

/// The renderer's gesture result in portable form; `code` is `OpuGestureCode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gesture {
    pub code: u32,
    pub node_id: u32,
}

/// The renderer-facing surface the host implements. No C types cross it.
pub trait InputSink {
    fn pointer(&mut self, event: HostPointer) -> Result<Gesture, i32>;
    fn next_deadline_ms(&self) -> i64;
    fn tick(&mut self, t_ms: i64) -> Result<Gesture, i32>;
}

/// A rejected logical time: negative, or older than the last accepted value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    NonMonotonic { previous: Option<i64>, now: i64 },
}

impl fmt::Display for ClockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClockError::NonMonotonic { previous, now } => {
                write!(f, "non-monotonic clock: previous={previous:?}, now={now}")
            }
        }
    }
}

impl std::error::Error for ClockError {}

/// The accepted clock position, tracked for one renderer only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    last: i64,
    armed: bool,
}

impl Clock {
    /// Accept a monotonic logical time or reject it without changing state.
    pub fn accept(&mut self, now: i64) -> Result<i64, ClockError> {
        if now < 0 || (self.armed && now < self.last) {
            return Err(ClockError::NonMonotonic {
                previous: self.armed.then_some(self.last),
                now,
            });
        }
        self.last = now;
        self.armed = true;
        Ok(now)
    }

    /// Called only when the renderer and its arena are rebuilt together.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// What [`InputRouter::feed`] did with one platform event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Ignored,
    Pointer,
    Cancelled,
}

/// Routes one window's input stream into the sink. Lifecycle transitions from
/// `Resumed` to an inactive state produce one cancel while a pointer is down;
/// repeated focus loss and refocus cannot emit a second one.
#[derive(Clone, Debug, Default)]
pub struct InputRouter {
    clock: Clock,
    last_x: f32,
    last_y: f32,
    down: bool,
    lifecycle: Option<Lifecycle>,
}

impl InputRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The renderer was rebuilt: its clock and arena start together.
    pub fn reset_renderer(&mut self) {
        self.clock.reset();
        self.down = false;
        self.lifecycle = None;
    }

    /// Feed one platform event at logical time `now_ms`.
    pub fn feed<S: InputSink>(
        &mut self,
        event: &InputEvent,
        now_ms: i64,
        sink: &mut S,
    ) -> Result<Feed, ClockError> {
        match event {
            InputEvent::Tap { x, y, node_id, .. } => {
                self.pointer(sink, PointerKind::Down, *node_id, *x, *y, now_ms)?;
                self.down = true;
                Ok(Feed::Pointer)
            }
            InputEvent::PointerMove { x, y, .. } | InputEvent::PointerDrag { x, y, .. } => {
                self.pointer(sink, PointerKind::Move, 0, *x, *y, now_ms)?;
                Ok(Feed::Pointer)
            }
            InputEvent::PointerUp { x, y, .. } => {
                self.pointer(sink, PointerKind::Up, 0, *x, *y, now_ms)?;
                self.down = false;
                Ok(Feed::Pointer)
            }
            InputEvent::PointerCancel { x, y } => {
                if !self.down {
                    return Ok(Feed::Ignored);
                }
                self.pointer(sink, PointerKind::Cancel, 0, *x, *y, now_ms)?;
                self.down = false;
                Ok(Feed::Cancelled)
            }
            InputEvent::Lifecycle(state) => {
                let changes = transitions(self.lifecycle, *state);
                if let Some(last) = changes.last() {
                    self.lifecycle = Some(*last);
                }
                let leaving = changes.iter().any(|next| {
                    matches!(
                        next,
                        Lifecycle::Inactive
                            | Lifecycle::Hidden
                            | Lifecycle::Paused
                            | Lifecycle::Destroyed
                    )
                });
                if !leaving || !self.down {
                    return Ok(Feed::Ignored);
                }
                let (x, y) = (self.last_x, self.last_y);
                self.pointer(sink, PointerKind::Cancel, 0, x, y, now_ms)?;
                self.down = false;
                Ok(Feed::Cancelled)
            }
            _ => Ok(Feed::Ignored),
        }
    }

    fn pointer<S: InputSink>(
        &mut self,
        sink: &mut S,
        kind: PointerKind,
        node_id: u32,
        x: f32,
        y: f32,
        now_ms: i64,
    ) -> Result<(), ClockError> {
        let time_ms = self.clock.accept(now_ms)?;
        self.last_x = x;
        self.last_y = y;
        let _ = sink.pointer(HostPointer {
            kind,
            pointer_id: 0,
            node_id,
            time_ms,
            x: self.last_x as i64,
            y: self.last_y as i64,
        });
        Ok(())
    }
}

/// Convert a signed logical deadline into a bounded relative wait. `-1` means
/// no deadline at all (keep the host's blocking pump); `i64::MAX` is a legal
/// saturated deadline and clamps to `cap_ms`, never collapsing into `-1`.
pub fn wait_from_deadline(deadline_ms: i64, now_ms: i64, cap_ms: u32) -> Option<u32> {
    if deadline_ms < 0 {
        return None;
    }
    let remaining = deadline_ms.saturating_sub(now_ms);
    Some(remaining.clamp(0, i64::from(cap_ms)) as u32)
}
