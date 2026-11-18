//! Deterministic system-event producers bridging host OS signals into the
//! unified taxonomy.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Flutter's framework has exactly one public system event, `memoryPressure`
//! (`services/binding.dart:174-182`). The other host signals (keyboard
//! insets, orientation, dark mode, power mode, refresh rate) have no public
//! framework broadcast; the framework reads them through platform view
//! configuration (`MediaQuery`) or one-shot commands (`system_chrome.dart`).
//! Opurom's hosts therefore report them as typed `host.*` adapter events in
//! [`crate::system_events::SystemEvent`], and this module provides the pure
//! translation from raw OS facts to those events.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/binding.dart:174-182` — the only
//!   framework-level event and its payload.
//! - `packages/flutter/lib/src/services/system_chrome.dart:43-70` — the
//!   four `DeviceOrientation` values in index order.
//! - `packages/flutter/lib/src/widgets/media_query.dart:916-935` —
//!   `platformBrightnessOf` derives dark mode from the platform view
//!   configuration; hosts report the change.
//! - `packages/flutter/lib/src/services/system_channels.dart:599` — the
//!   keyboard channel that carries visibility.
//!
//! Determinism: every producer is a pure function from raw host facts to a
//! closed event value. No wall clock, no random source, no hidden state, so
//! the same facts always produce the same event sequence on every target.

use crate::display_geometry::{Posture, DPI_UNIT};
use crate::system_events::SystemEvent;

/// The four Flutter device orientations (`system_chrome.dart:43-70`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceOrientation {
    Portrait,
    LandscapeLeft,
    PortraitUp,
    LandscapeRight,
}

impl DeviceOrientation {
    /// Flutter's enum index, which is the wire value of the host event.
    pub fn index(self) -> u8 {
        match self {
            Self::Portrait => 0,
            Self::LandscapeLeft => 1,
            Self::PortraitUp => 2,
            Self::LandscapeRight => 3,
        }
    }

    /// Decode an orientation index. Out-of-range values are `None`, never a
    /// guessed default.
    pub fn from_index(index: u8) -> Option<Self> {
        Some(match index {
            0 => Self::Portrait,
            1 => Self::LandscapeLeft,
            2 => Self::PortraitUp,
            3 => Self::LandscapeRight,
            _ => return None,
        })
    }

    /// The orientation implied by a physical size: width greater than height
    /// is `landscapeLeft`, otherwise `portrait`. Hosts without a native
    /// orientation signal use this projection (`display_geometry` decides
    /// compact/expanded from the same comparison).
    pub fn from_size(width: u32, height: u32) -> Self {
        if width > height {
            Self::LandscapeLeft
        } else {
            Self::Portrait
        }
    }
}

/// Translate a raw keyboard visibility fact into the host event. `height` is
/// the inset in logical pixels; `None` means the host only knows the boolean.
pub fn keyboard_visibility(visible: bool, height: Option<u32>) -> SystemEvent {
    SystemEvent::KeyboardVisibility { visible, height }
}

/// Translate an orientation index into the host event. An unknown index is
/// `None` (the caller decides whether to drop or log it); a known index maps
/// to the exact Flutter order (`system_chrome.dart:43-70`).
pub fn orientation_from_index(index: u8) -> Option<SystemEvent> {
    DeviceOrientation::from_index(index).map(|orientation| SystemEvent::Orientation {
        orientation: orientation.index(),
    })
}

/// Translate a physical size into the host orientation event.
pub fn orientation_from_size(width: u32, height: u32) -> SystemEvent {
    SystemEvent::Orientation {
        orientation: DeviceOrientation::from_size(width, height).index(),
    }
}

/// Translate a dark-mode fact into the host event. `true` is dark.
pub fn brightness(dark: bool) -> SystemEvent {
    SystemEvent::Brightness { dark }
}

/// Translate a power-mode fact into the host event. `true` is low power.
pub fn power_mode(low_power: bool) -> SystemEvent {
    SystemEvent::PowerMode { low_power }
}

/// Translate a display refresh rate into the host event.
pub fn refresh_rate(hz: u32) -> SystemEvent {
    SystemEvent::RefreshRate { hz }
}

/// The sole Flutter framework event: memory pressure. The host calls this
/// when the OS delivers its low-memory signal (`binding.dart:174-182`).
pub fn memory_pressure() -> SystemEvent {
    SystemEvent::MemoryPressure
}

/// The posture events a foldable host reports. The geometry event carries
/// the full [`crate::display_geometry::DisplayGeometry`]; the system event
/// stream carries a compact `host.orientation` projection so subscribers
/// that only care about rotation can subscribe to one kind. The mapping is
/// stable: `Flat`/`HalfOpened`/`Book` keep the size-derived orientation, and
/// a `Hinge` angle of at least 180 degrees is treated as `Flat`.
pub fn orientation_from_posture(posture: Posture, width: u32, height: u32) -> SystemEvent {
    match posture {
        Posture::Flat | Posture::HalfOpened | Posture::Book | Posture::Hinge { .. } => {
            orientation_from_size(width, height)
        }
    }
}

/// The DPI scale in fixed-point units, exposed so a host can convert its raw
/// scale to the shared [`DPI_UNIT`] grid before building a geometry event.
pub fn dpi_scale_fixed_point(scale: f32) -> u32 {
    (scale * DPI_UNIT as f32).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_channel::Value;
    use crate::system_events::SystemEvent;

    #[test]
    fn orientation_index_order_matches_flutter() {
        assert_eq!(DeviceOrientation::Portrait.index(), 0);
        assert_eq!(DeviceOrientation::LandscapeLeft.index(), 1);
        assert_eq!(DeviceOrientation::PortraitUp.index(), 2);
        assert_eq!(DeviceOrientation::LandscapeRight.index(), 3);
        for index in 0..4 {
            assert_eq!(
                DeviceOrientation::from_index(index).map(|o| o.index()),
                Some(index)
            );
        }
        assert_eq!(DeviceOrientation::from_index(4), None);
        assert_eq!(DeviceOrientation::from_index(255), None);
    }

    #[test]
    fn host_events_round_trip_through_the_wire_value() {
        let events = [
            keyboard_visibility(true, Some(300)),
            keyboard_visibility(false, None),
            orientation_from_index(1).unwrap(),
            orientation_from_size(800, 400),
            brightness(true),
            brightness(false),
            power_mode(true),
            refresh_rate(120),
            memory_pressure(),
        ];
        for event in events {
            let value = event.to_value();
            assert_eq!(SystemEvent::from_value(&value), Some(event));
        }
    }

    #[test]
    fn size_projection_uses_width_over_height() {
        assert_eq!(
            orientation_from_size(400, 800),
            SystemEvent::Orientation { orientation: 0 }
        );
        assert_eq!(
            orientation_from_size(800, 400),
            SystemEvent::Orientation { orientation: 1 }
        );
        assert_eq!(
            orientation_from_size(600, 600),
            SystemEvent::Orientation { orientation: 0 }
        );
    }

    #[test]
    fn posture_projection_keeps_the_size_rule() {
        assert_eq!(
            orientation_from_posture(Posture::HalfOpened, 400, 800),
            SystemEvent::Orientation { orientation: 0 }
        );
        assert_eq!(
            orientation_from_posture(Posture::Hinge { angle: 180 }, 800, 400),
            SystemEvent::Orientation { orientation: 1 }
        );
    }

    #[test]
    fn memory_pressure_payload_is_exact() {
        let event = memory_pressure();
        assert_eq!(event.code(), "memoryPressure");
        assert!(event.is_flutter_framework_event());
        assert_eq!(
            event.to_value(),
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
    fn dpi_scale_converts_to_the_shared_grid() {
        assert_eq!(dpi_scale_fixed_point(1.0), DPI_UNIT);
        assert_eq!(dpi_scale_fixed_point(2.0), DPI_UNIT * 2);
        assert_eq!(dpi_scale_fixed_point(1.5), DPI_UNIT * 3 / 2);
    }
}
