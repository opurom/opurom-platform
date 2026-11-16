//! Mapping tests for the window-event → input rules: motion classification,
//! wheel notch normalization and the wire frame stream, the layer every host
//! can exercise without a window server.

use opurom_platform::input_map::{
    dpi_from_millimeters, dpi_scale, pointer_move, wayland_wheel_delta, wheel_delta, wire_input,
    x11_scroll_notch,
};
use opurom_platform::wayland_wire::{decode_event, WireEvent};
use opurom_platform::InputEvent;

// ---------------------------------------------------------------- pointer

#[test]
fn a_move_is_a_drag_only_while_a_button_is_held() {
    assert_eq!(
        pointer_move(false, 3.0, 4.0),
        InputEvent::PointerMove { x: 3.0, y: 4.0 }
    );
    assert_eq!(
        pointer_move(true, 3.0, 4.0),
        InputEvent::PointerDrag { x: 3.0, y: 4.0 }
    );
}

// ---------------------------------------------------------------- wheel

#[test]
fn wayland_scroll_values_flip_into_notches() {
    assert_eq!(wayland_wheel_delta(-256), 1.0);
    assert_eq!(wayland_wheel_delta(256), -1.0);
    assert_eq!(wayland_wheel_delta(-128), 0.5);
}

#[test]
fn wheel_notches_are_normalized_from_raw_units() {
    assert_eq!(wheel_delta(120), 1.0);
    assert_eq!(wheel_delta(-120), -1.0);
    assert_eq!(wheel_delta(240), 2.0);
    assert_eq!(wheel_delta(60), 0.5);
    assert_eq!(wheel_delta(0), 0.0);
}

#[test]
fn x11_scroll_buttons_map_to_notches_and_nothing_else_does() {
    assert_eq!(x11_scroll_notch(4), Some(1.0));
    assert_eq!(x11_scroll_notch(5), Some(-1.0));
    // 1-3 are real buttons, 6/7 are the documented horizontal gap.
    for button in [1, 2, 3, 6, 7, 8] {
        assert_eq!(x11_scroll_notch(button), None, "button {button}");
    }
}

// ---------------------------------------------------------------- scale

#[test]
fn dpi_maps_against_the_96_baseline() {
    assert_eq!(dpi_scale(96), 1.0);
    assert_eq!(dpi_scale(144), 1.5);
    assert_eq!(dpi_scale(192), 2.0);
}

#[test]
fn x11_scale_comes_from_the_screen_millimeters() {
    // A 3840-wide screen reporting 609 mm is 160 DPI: 1.5x content.
    assert!((dpi_from_millimeters(3840, 609) - 1.666).abs() < 0.01);
    assert_eq!(dpi_from_millimeters(1920, 508), 1.0);
    // A screen reporting no physical size degrades to 1.0.
    assert_eq!(dpi_from_millimeters(1920, 0), 1.0);
}

// ---------------------------------------------------------------- wire

#[test]
fn a_wire_bracket_drives_the_whole_pointer_stream() {
    let mut pressed = false;

    let (input, after) = wire_input(WireEvent::Press { x: 1, y: 2 }, pressed);
    assert_eq!(
        input,
        Some(InputEvent::Tap {
            x: 1.0,
            y: 2.0,
            node_id: u32::MAX,
        })
    );
    pressed = after;
    assert!(pressed, "a press opens the bracket");

    let (input, after) = wire_input(WireEvent::Move { x: -5, y: 7 }, pressed);
    assert_eq!(input, Some(InputEvent::PointerDrag { x: -5.0, y: 7.0 }));
    pressed = after;

    let (input, after) = wire_input(WireEvent::Release { x: -5, y: 7 }, pressed);
    assert_eq!(input, Some(InputEvent::PointerUp { x: -5.0, y: 7.0 }));
    pressed = after;
    assert!(!pressed, "a release closes the bracket");

    let (input, _) = wire_input(WireEvent::Move { x: 0, y: 0 }, pressed);
    assert_eq!(input, Some(InputEvent::PointerMove { x: 0.0, y: 0.0 }));
}

#[test]
fn resize_maps_to_input_and_state_frames_stay_out() {
    let (input, pressed) = wire_input(
        WireEvent::Resize {
            width: 800,
            height: 600,
        },
        false,
    );
    assert_eq!(
        input,
        Some(InputEvent::Resize {
            size: opurom_platform::Size {
                width: 800,
                height: 600,
            }
        })
    );
    assert!(!pressed, "resize does not touch the press bracket");

    // Focus, geometry, layout and backend frames share one "state, not input"
    // arm; both ends of it are checked here.
    for frame in [
        WireEvent::Focus { focused: true },
        WireEvent::GraphicsBackend(
            opurom_platform::graphics_backend::GraphicsBackendCapabilities::software_default(),
        ),
    ] {
        let (input, pressed) = wire_input(frame, true);
        assert_eq!(input, None, "{frame:?} is state, not input");
        assert!(pressed, "a state frame keeps the bracket");
    }
}

/// End to end at the structure layer: encoded wire bytes decode and land in
/// the ABI stream exactly as sent.
#[test]
fn wire_bytes_decode_then_map_into_input() {
    let bytes = WireEvent::Move { x: -7, y: 9 }.encode().expect("encode");
    let (frame, consumed) = decode_event(&bytes).expect("decode").expect("one frame");
    assert_eq!(consumed, bytes.len());
    let (input, pressed) = wire_input(frame, false);
    assert_eq!(input, Some(InputEvent::PointerMove { x: -7.0, y: 9.0 }));
    assert!(!pressed);
}
