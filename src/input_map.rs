//! Pure window-event → input mapping rules. Each platform adapter extracts
//! raw fields and delegates the decision here, and the Wayland client maps its
//! wire frames through [`wire_input`], so every rule in the pointer stream is
//! testable on every host without a window server.

use crate::wayland_wire::WireEvent;
use crate::{InputEvent, Size};

/// A pointer moved: a button held means a drag, otherwise a free move.
pub fn pointer_move(pressed: bool, x: f32, y: f32) -> InputEvent {
    if pressed {
        InputEvent::PointerDrag { x, y }
    } else {
        InputEvent::PointerMove { x, y }
    }
}

/// `WM_MOUSEWHEEL` carries its delta in twelfths of a notch step (120 per
/// notch); normalize to notches, negative pointing toward the user (scrolled
/// down).
pub fn wheel_delta(raw: i16) -> f32 {
    raw as f32 / 120.0
}

/// X11 scroll buttons: 4 scrolls away from the user (up), 5 toward it. Any
/// other button number is not a wheel.
///
/// shortcut: horizontal wheel buttons 6/7 stay unmapped until the ABI grows
/// a horizontal axis.
pub fn x11_scroll_notch(button: u32) -> Option<f32> {
    match button {
        4 => Some(1.0),
        5 => Some(-1.0),
        _ => None,
    }
}

/// One decoded wire frame into the ABI stream, carrying the pointer stream's
/// press bracket: `pressed` starts false, turns true at `Press` and clears at
/// `Release`, so a `Move` inside the bracket is a drag. Returns the input
/// event (focus, geometry, layout and backend frames are state, not input)
/// and the bracket state after the frame.
pub fn wire_input(event: WireEvent, pressed: bool) -> (Option<InputEvent>, bool) {
    match event {
        WireEvent::Resize { width, height } => (
            Some(InputEvent::Resize {
                size: Size { width, height },
            }),
            pressed,
        ),
        WireEvent::Press { x, y } => (
            Some(InputEvent::Tap {
                x: x as f32,
                y: y as f32,
                node_id: u32::MAX,
            }),
            true,
        ),
        WireEvent::Release { x, y } => (
            Some(InputEvent::PointerUp {
                x: x as f32,
                y: y as f32,
            }),
            false,
        ),
        WireEvent::Move { x, y } => (Some(pointer_move(pressed, x as f32, y as f32)), pressed),
        WireEvent::Focus { .. }
        | WireEvent::Geometry(_)
        | WireEvent::LayoutPatch(_)
        | WireEvent::GraphicsBackend(_) => (None, pressed),
    }
}
