//! Mapping tests for the window-event → input rules: motion classification,
//! wheel notch normalization and the wire frame stream, the layer every host
//! can exercise without a window server.

use opurom_platform::input_map::{
    dpi_from_millimeters, dpi_scale, hover_events, logical_coord, logical_size, pointer_move,
    wayland_button, wayland_wheel_delta, wayland_wheel_delta_x, wheel_delta, win32_buttons,
    win32_modifiers, wire_input, x11_button, x11_modifiers, x11_motion_buttons, x11_scroll_notch,
    x11_scroll_notch_x, BUTTON_BACK, BUTTON_FORWARD, BUTTON_MIDDLE, BUTTON_PRIMARY,
    BUTTON_SECONDARY, MOD_ALT, MOD_CONTROL, MOD_META, MOD_SHIFT,
};
use opurom_platform::wayland_wire::{decode_event, WireEvent};
use opurom_platform::{InputEvent, PointerState, Size};

// ---------------------------------------------------------------- pointer

#[test]
fn a_move_is_a_drag_only_while_a_button_is_held() {
    assert_eq!(
        pointer_move(PointerState::default(), 3.0, 4.0),
        InputEvent::PointerMove {
            x: 3.0,
            y: 4.0,
            pointer: PointerState::default()
        }
    );
    assert_eq!(
        pointer_move(
            PointerState {
                buttons: opurom_platform::input_map::BUTTON_PRIMARY,
                ..PointerState::default()
            },
            3.0,
            4.0
        ),
        InputEvent::PointerDrag {
            x: 3.0,
            y: 4.0,
            pointer: PointerState {
                buttons: opurom_platform::input_map::BUTTON_PRIMARY,
                ..PointerState::default()
            }
        }
    );
}

// ---------------------------------------------------------------- identity

#[test]
fn win32_flags_map_to_stable_bits() {
    assert_eq!(win32_buttons(0x0001), BUTTON_PRIMARY);
    assert_eq!(win32_buttons(0x0002), BUTTON_SECONDARY);
    assert_eq!(win32_buttons(0x0010), BUTTON_MIDDLE);
    assert_eq!(win32_buttons(0x0020), BUTTON_BACK);
    assert_eq!(win32_buttons(0x0040), BUTTON_FORWARD);
    assert_eq!(win32_buttons(0x0003), BUTTON_PRIMARY | BUTTON_SECONDARY);
    assert_eq!(win32_buttons(0), 0);
    assert_eq!(win32_modifiers(0x0004), MOD_SHIFT);
    assert_eq!(win32_modifiers(0x0008), MOD_CONTROL);
    assert_eq!(win32_modifiers(0x000c), MOD_SHIFT | MOD_CONTROL);
    // 0x20 is MK_XBUTTON1, not Alt: Alt has no `wParam` bit.
    assert_eq!(win32_modifiers(0x0020), 0);
    assert_eq!(win32_modifiers(0x0040), 0);
}

#[test]
fn x11_buttons_and_modifiers_map_to_stable_bits() {
    assert_eq!(x11_button(1), Some(BUTTON_PRIMARY));
    assert_eq!(x11_button(2), Some(BUTTON_MIDDLE));
    assert_eq!(x11_button(3), Some(BUTTON_SECONDARY));
    assert_eq!(x11_button(8), Some(BUTTON_BACK));
    assert_eq!(x11_button(9), Some(BUTTON_FORWARD));
    assert_eq!(x11_button(4), None);
    assert_eq!(x11_button(5), None);
    assert_eq!(
        x11_modifiers((1 << 0) | (1 << 2) | (1 << 3) | (1 << 6)),
        MOD_SHIFT | MOD_CONTROL | MOD_ALT | MOD_META
    );
    assert_eq!(x11_modifiers(0), 0);
    assert_eq!(x11_motion_buttons(1 << 8), BUTTON_PRIMARY);
    assert_eq!(x11_motion_buttons(1 << 9), BUTTON_MIDDLE);
    assert_eq!(x11_motion_buttons(1 << 10), BUTTON_SECONDARY);
    assert_eq!(
        x11_motion_buttons((1 << 8) | (1 << 10)),
        BUTTON_PRIMARY | BUTTON_SECONDARY
    );
}

#[test]
fn wayland_button_codes_map_to_stable_bits() {
    assert_eq!(wayland_button(0x110), Some(BUTTON_PRIMARY));
    assert_eq!(wayland_button(0x111), Some(BUTTON_SECONDARY));
    assert_eq!(wayland_button(0x112), Some(BUTTON_MIDDLE));
    assert_eq!(wayland_button(0x113), Some(BUTTON_BACK));
    assert_eq!(wayland_button(0x114), Some(BUTTON_FORWARD));
    assert_eq!(wayland_button(0x115), Some(BUTTON_FORWARD));
    assert_eq!(wayland_button(0x116), Some(BUTTON_BACK));
    assert_eq!(wayland_button(0x100), None);
}

#[test]
fn drag_classification_uses_the_button_mask() {
    let idle = PointerState::default();
    assert_eq!(
        pointer_move(idle, 3.0, 4.0),
        InputEvent::PointerMove {
            x: 3.0,
            y: 4.0,
            pointer: idle
        }
    );
    let held = PointerState {
        buttons: BUTTON_SECONDARY,
        ..PointerState::default()
    };
    assert_eq!(
        pointer_move(held, 3.0, 4.0),
        InputEvent::PointerDrag {
            x: 3.0,
            y: 4.0,
            pointer: held
        }
    );
}

// ---------------------------------------------------------------- hover

#[test]
fn hover_edges_are_deduped_and_ordered() {
    let pointer = PointerState::default();
    assert_eq!(
        hover_events(false, true, 1.0, 2.0, pointer),
        vec![InputEvent::PointerEnter {
            x: 1.0,
            y: 2.0,
            pointer
        }]
    );
    assert_eq!(
        hover_events(true, false, 3.0, 4.0, pointer),
        vec![InputEvent::PointerExit {
            x: 3.0,
            y: 4.0,
            pointer
        }]
    );
    assert!(hover_events(false, false, 0.0, 0.0, pointer).is_empty());
    assert!(hover_events(true, true, 0.0, 0.0, pointer).is_empty());

    // A window-to-window move delivers exit before enter; the hover follows.
    let mut stream = Vec::new();
    stream.extend(hover_events(true, false, 1.0, 2.0, pointer));
    stream.extend(hover_events(false, true, 3.0, 4.0, pointer));
    stream.push(pointer_move(pointer, 3.0, 4.0));
    assert!(matches!(stream[0], InputEvent::PointerExit { .. }));
    assert!(matches!(stream[1], InputEvent::PointerEnter { .. }));
    assert!(matches!(stream[2], InputEvent::PointerMove { .. }));
}

// ---------------------------------------------------------------- coordinates

#[test]
fn physical_coordinates_convert_to_logical_pixels() {
    assert_eq!(logical_coord(100.0, 1.0), 100.0);
    assert_eq!(logical_coord(100.0, 2.0), 50.0);
    assert_eq!(logical_coord(100.0, 1.25), 80.0);
    let thirds = logical_coord(100.0, 1.5);
    assert!((thirds - 200.0 / 3.0).abs() < 0.001, "{thirds}");
    assert_eq!(logical_coord(0.0, 2.0), 0.0);
    assert_eq!(logical_coord(-100.0, 2.0), -50.0);
}

#[test]
fn invalid_scales_degrade_to_identity_and_stay_deterministic() {
    assert_eq!(logical_coord(42.0, 0.0), 42.0);
    assert_eq!(logical_coord(42.0, -1.0), 42.0);
    assert_eq!(logical_coord(42.0, f32::NAN), 42.0);
    assert_eq!(logical_coord(42.0, f32::INFINITY), 42.0);
    assert_eq!(
        logical_coord(123.4, 1.5).to_bits(),
        logical_coord(123.4, 1.5).to_bits()
    );
}

#[test]
fn logical_viewports_round_to_whole_pixels() {
    assert_eq!(
        logical_size(
            Size {
                width: 800,
                height: 600
            },
            2.0
        ),
        Size {
            width: 400,
            height: 300
        }
    );
    assert_eq!(
        logical_size(
            Size {
                width: 801,
                height: 601
            },
            1.5
        ),
        Size {
            width: 534,
            height: 401
        }
    );
    assert_eq!(
        logical_size(
            Size {
                width: 0,
                height: 0
            },
            2.0
        ),
        Size {
            width: 0,
            height: 0
        }
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
    assert_eq!(x11_scroll_notch_x(6), Some(1.0));
    assert_eq!(x11_scroll_notch_x(7), Some(-1.0));
    for button in [1, 2, 3, 8] {
        assert_eq!(x11_scroll_notch(button), None, "button {button}");
        assert_eq!(x11_scroll_notch_x(button), None, "button {button}");
    }
}

#[test]
fn wayland_scroll_values_map_to_signed_notches() {
    assert_eq!(wayland_wheel_delta(256), -1.0);
    assert_eq!(wayland_wheel_delta(-256), 1.0);
    assert_eq!(wayland_wheel_delta_x(256), 1.0);
    assert_eq!(wayland_wheel_delta_x(-256), -1.0);
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
            pointer: PointerState {
                buttons: BUTTON_PRIMARY,
                ..PointerState::default()
            },
        })
    );
    pressed = after;
    assert!(pressed, "a press opens the bracket");

    let (input, after) = wire_input(WireEvent::Move { x: -5, y: 7 }, pressed);
    assert_eq!(
        input,
        Some(InputEvent::PointerDrag {
            x: -5.0,
            y: 7.0,
            pointer: PointerState {
                buttons: BUTTON_PRIMARY,
                ..PointerState::default()
            }
        })
    );
    pressed = after;

    let (input, after) = wire_input(WireEvent::Release { x: -5, y: 7 }, pressed);
    assert_eq!(
        input,
        Some(InputEvent::PointerUp {
            x: -5.0,
            y: 7.0,
            pointer: PointerState::default()
        })
    );
    pressed = after;
    assert!(!pressed, "a release closes the bracket");

    let (input, _) = wire_input(WireEvent::Move { x: 0, y: 0 }, pressed);
    assert_eq!(
        input,
        Some(InputEvent::PointerMove {
            x: 0.0,
            y: 0.0,
            pointer: PointerState::default()
        })
    );
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
    assert_eq!(
        input,
        Some(InputEvent::PointerMove {
            x: -7.0,
            y: 9.0,
            pointer: PointerState::default()
        })
    );
    assert!(!pressed);
}
