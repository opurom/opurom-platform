//! Pure window-event → input mapping rules. Each platform adapter extracts
//! raw fields and delegates the decision here, and the Wayland client maps its
//! wire frames through [`wire_input`], so every rule in the pointer stream is
//! testable on every host without a window server.

use crate::display_geometry::DPI_UNIT;
use crate::wayland_wire::WireEvent;
use crate::{InputEvent, PointerState, Size};

/// Stable button bits carried by every pointer event.
pub const BUTTON_PRIMARY: u32 = 1 << 0;
pub const BUTTON_SECONDARY: u32 = 1 << 1;
pub const BUTTON_MIDDLE: u32 = 1 << 2;
pub const BUTTON_BACK: u32 = 1 << 3;
pub const BUTTON_FORWARD: u32 = 1 << 4;

/// Stable modifier bits carried by every pointer event.
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CONTROL: u32 = 1 << 1;
pub const MOD_ALT: u32 = 1 << 2;
pub const MOD_META: u32 = 1 << 3;

/// A pointer moved: any button held means a drag, otherwise a free move.
pub fn pointer_move(pointer: PointerState, x: f32, y: f32) -> InputEvent {
    if pointer.buttons != 0 {
        InputEvent::PointerDrag { x, y, pointer }
    } else {
        InputEvent::PointerMove { x, y, pointer }
    }
}

/// Win32 `wParam` button flags (windows-sys `MK_LBUTTON`/`MK_RBUTTON`/
/// `MK_MBUTTON`) into the ABI's stable button bits.
pub fn win32_buttons(flags: u32) -> u32 {
    let mut bits = 0;
    if flags & 0x0001 != 0 {
        bits |= BUTTON_PRIMARY;
    }
    if flags & 0x0002 != 0 {
        bits |= BUTTON_SECONDARY;
    }
    if flags & 0x0010 != 0 {
        bits |= BUTTON_MIDDLE;
    }
    bits
}

/// Win32 `wParam` modifier flags (`MK_SHIFT`/`MK_CONTROL`/`MK_ALT`).
pub fn win32_modifiers(flags: u32) -> u32 {
    let mut bits = 0;
    if flags & 0x0004 != 0 {
        bits |= MOD_SHIFT;
    }
    if flags & 0x0008 != 0 {
        bits |= MOD_CONTROL;
    }
    if flags & 0x0020 != 0 {
        bits |= MOD_ALT;
    }
    bits
}

/// X11 button numbers (X protocol Button1..Button9) into stable button bits.
pub fn x11_button(button: u32) -> Option<u32> {
    match button {
        1 => Some(BUTTON_PRIMARY),
        2 => Some(BUTTON_MIDDLE),
        3 => Some(BUTTON_SECONDARY),
        8 => Some(BUTTON_BACK),
        9 => Some(BUTTON_FORWARD),
        _ => None,
    }
}

/// X11 motion `state` button masks (`Button1Mask` 1<<8 ... `Button3Mask`
/// 1<<10) into the stable button bits.
pub fn x11_motion_buttons(state: u32) -> u32 {
    let mut bits = 0;
    if state & (1 << 8) != 0 {
        bits |= BUTTON_PRIMARY;
    }
    if state & (1 << 9) != 0 {
        bits |= BUTTON_MIDDLE;
    }
    if state & (1 << 10) != 0 {
        bits |= BUTTON_SECONDARY;
    }
    bits
}

/// X11 `state` modifier mask (`X.h`: ShiftMask=1, ControlMask=4,
/// Mod1Mask=8, Mod4Mask=64) into stable modifier bits.
pub fn x11_modifiers(state: u32) -> u32 {
    let mut bits = 0;
    if state & (1 << 0) != 0 {
        bits |= MOD_SHIFT;
    }
    if state & (1 << 2) != 0 {
        bits |= MOD_CONTROL;
    }
    if state & (1 << 3) != 0 {
        bits |= MOD_ALT;
    }
    if state & (1 << 6) != 0 {
        bits |= MOD_META;
    }
    bits
}

/// Wayland `wl_pointer.button` carries Linux `input-event-codes.h` values
/// (`BTN_LEFT` 0x110 ... `BTN_BACK` 0x116).
pub fn wayland_button(button: u32) -> Option<u32> {
    match button {
        0x110 => Some(BUTTON_PRIMARY),
        0x111 => Some(BUTTON_SECONDARY),
        0x112 => Some(BUTTON_MIDDLE),
        0x113 | 0x116 => Some(BUTTON_BACK),
        0x114 | 0x115 => Some(BUTTON_FORWARD),
        _ => None,
    }
}

/// `WM_MOUSEWHEEL` carries its delta in twelfths of a notch step (120 per
/// notch); normalize to notches, negative pointing toward the user (scrolled
/// down).
pub fn wheel_delta(raw: i16) -> f32 {
    raw as f32 / 120.0
}

fn fixed_scale(scale: f32) -> i128 {
    let fixed = if scale.is_finite() && scale > 0.0 {
        (scale * DPI_UNIT as f32).round()
    } else {
        DPI_UNIT as f32
    };
    (fixed as i128).max(1)
}

/// Window-level hover transition. Returns `PointerEnter` on the outside→inside
/// edge, `PointerExit` on inside→outside, and nothing when the state is
/// unchanged; node-level hover is the ordinary move that follows the enter.
pub fn hover_events(
    was_inside: bool,
    inside: bool,
    x: f32,
    y: f32,
    pointer: PointerState,
) -> Vec<InputEvent> {
    match (was_inside, inside) {
        (false, true) => vec![InputEvent::PointerEnter { x, y, pointer }],
        (true, false) => vec![InputEvent::PointerExit { x, y, pointer }],
        _ => Vec::new(),
    }
}

/// Convert one physical (backing) pointer coordinate to the ABI's logical
/// pixels. The window scale is snapped to the fixed-point `DPI_UNIT`
/// (`1024 == 1.0`) and the division is integer, so every host produces
/// bit-identical coordinates for the same input; the final cast is the only
/// floating-point step.
pub fn logical_coord(physical: f32, scale: f32) -> f32 {
    let fixed = fixed_scale(scale);
    let physical_sub = (physical as f64 * DPI_UNIT as f64).round() as i128;
    let numerator = physical_sub * DPI_UNIT as i128;
    let logical_sub = if numerator >= 0 {
        (numerator + fixed / 2) / fixed
    } else {
        -((-numerator + fixed / 2) / fixed)
    };
    (logical_sub as f64 / DPI_UNIT as f64) as f32
}

/// The logical viewport of a physical backing size, rounded to whole pixels.
pub fn logical_size(physical: Size, scale: f32) -> Size {
    Size {
        width: logical_coord(physical.width as f32, scale).round() as u32,
        height: logical_coord(physical.height as f32, scale).round() as u32,
    }
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
                pointer: PointerState {
                    buttons: BUTTON_PRIMARY,
                    ..PointerState::default()
                },
            }),
            true,
        ),
        WireEvent::Release { x, y } => (
            Some(InputEvent::PointerUp {
                x: x as f32,
                y: y as f32,
                pointer: PointerState::default(),
            }),
            false,
        ),
        WireEvent::Move { x, y } => {
            let pointer = PointerState {
                buttons: if pressed { BUTTON_PRIMARY } else { 0 },
                ..PointerState::default()
            };
            (Some(pointer_move(pointer, x as f32, y as f32)), pressed)
        }
        WireEvent::Focus { .. }
        | WireEvent::Geometry(_)
        | WireEvent::LayoutPatch(_)
        | WireEvent::GraphicsBackend(_) => (None, pressed),
    }
}

/// Wayland's vertical scroll value is signed 24.8 and positive scrolls down;
/// flip the sign into the ABI's "positive scrolls up" notch convention.
pub fn wayland_wheel_delta(raw_fixed: i32) -> f32 {
    -(raw_fixed as f32 / 256.0)
}

/// A device DPI value against the ABI's 96 DPI baseline: 96 reports 1.0,
/// 144 reports 1.5.
pub fn dpi_scale(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// X11 knows its screen only in pixels and millimeters; the physical DPI
/// (25.4 mm to the inch) stands in for the scale. A screen that reports no
/// size degrades to 1.0.
///
/// shortcut: fractional RandR scaling and its change events need libXrandr
/// plus buffer-scaled presentation; 0.1.0 emits this one value per window.
pub fn dpi_from_millimeters(pixels: u32, millimeters: u32) -> f32 {
    if millimeters == 0 {
        return 1.0;
    }
    (f64::from(pixels) * 25.4 / f64::from(millimeters) / 96.0) as f32
}
