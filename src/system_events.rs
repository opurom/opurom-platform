//! Unified system event taxonomy and deterministic dispatcher.
//!
//! Flutter baseline: 3.50-candidate, engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! Source pins:
//! - `packages/flutter/lib/src/services/system_channels.dart:395-405` — the
//!   only framework-level generic system channel is
//!   `BasicMessageChannel<Object?>('flutter/system', JSONMessageCodec())`,
//!   whose sole documented type is `memoryPressure`.
//! - `packages/flutter/lib/src/services/binding.dart:170-186` —
//!   `handleSystemMessage` casts the payload to `Map<String, dynamic>` and
//!   switches on the string `type`; `memoryPressure` invokes
//!   `handleMemoryPressure`, which clears `rootBundle` (`:160-166`).
//! - `packages/flutter/lib/src/services/system_chrome.dart:547-552,609-618`
//!   — orientation and system UI mode are host commands, not framework
//!   broadcast events.
//! - `packages/flutter/lib/src/services/system_channels.dart:370-371` —
//!   keyboard events use a separate `flutter/keyevent` basic message channel.
//!
//! Opurom has no runtime observer registry (IR is the only truth source). This
//! module defines a closed event enum, a deduplicating dispatcher with
//! compile-time subscriber slots, and the exact `memoryPressure` payload
//! shape. Host-specific signals that Flutter has no public unified event for
//! are marked `HOST_ADAPTER` and carry a stable string code; they are not
//! claimed to be Flutter framework API.

use crate::platform_channel::{system_channels, Value};

/// The closed system event set. Variant order is stable and part of the ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// `flutter/system` `{"type":"memoryPressure"}`
    /// (`services/binding.dart:178-181`).
    MemoryPressure,
    /// Keyboard visibility change. Flutter has no public framework event;
    /// hosts report it through `flutter/keyboard`
    /// (`system_channels.dart:599`). Marked HOST_ADAPTER.
    KeyboardVisibility {
        visible: bool,
        /// Insets in logical pixels when known; `None` when the host only
        /// reports the boolean edge.
        height: Option<u32>,
    },
    /// Screen orientation change. Flutter exposes the preferred orientation
    /// as a command (`system_chrome.dart:547-552`), not a broadcast event.
    /// Marked HOST_ADAPTER.
    Orientation {
        /// 0=portrait, 1=landscapeLeft, 2=portraitUp, 3=landscapeRight,
        /// matching Flutter `DeviceOrientation` index order
        /// (`system_chrome.dart:43-70`).
        orientation: u8,
    },
    /// Dark-mode / brightness change. Flutter exposes it through
    /// `MediaQuery.of(context).platformBrightness`, rebuilt from the platform
    /// view configuration, not a system channel. Marked HOST_ADAPTER.
    Brightness {
        /// `true` = dark, `false` = light.
        dark: bool,
    },
    /// Low-power / battery-saver mode change. No Flutter framework event;
    /// HOST_ADAPTER.
    PowerMode { low_power: bool },
    /// Display refresh-rate change. Flutter's frame scheduler reads the
    /// engine vsync target; no public event. HOST_ADAPTER.
    RefreshRate { hz: u32 },
}

impl SystemEvent {
    /// Stable wire code. `memoryPressure` is the exact Flutter JSON `type`
    /// string (`services/binding.dart:178`). Others are Opurom host-adapter
    /// codes, explicitly namespaced.
    pub fn code(&self) -> &'static str {
        match self {
            SystemEvent::MemoryPressure => "memoryPressure",
            SystemEvent::KeyboardVisibility { .. } => "host.keyboardVisibility",
            SystemEvent::Orientation { .. } => "host.orientation",
            SystemEvent::Brightness { .. } => "host.brightness",
            SystemEvent::PowerMode { .. } => "host.powerMode",
            SystemEvent::RefreshRate { .. } => "host.refreshRate",
        }
    }

    /// Whether Flutter's framework defines this event type in
    /// `services/binding.dart:178-182`. Only memory pressure is `true`.
    pub fn is_flutter_framework_event(&self) -> bool {
        matches!(self, SystemEvent::MemoryPressure)
    }

    /// Encode as the exact JSON-shaped value Flutter's `JSONMessageCodec`
    /// delivers for `flutter/system` (`system_channels.dart:401-404`,
    /// `binding.dart:177-179`). For host-adapter events, the same map shape
    /// carries the Opurom-namespaced code and typed fields.
    pub fn to_value(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            Value::String("type".to_owned()),
            Value::String(self.code().to_owned()),
        );
        match self {
            SystemEvent::MemoryPressure => {}
            SystemEvent::KeyboardVisibility { visible, height } => {
                map.insert(Value::String("visible".to_owned()), Value::Bool(*visible));
                map.insert(
                    Value::String("height".to_owned()),
                    height.map(|h| Value::I32(h as i32)).unwrap_or(Value::Null),
                );
            }
            SystemEvent::Orientation { orientation } => {
                map.insert(
                    Value::String("orientation".to_owned()),
                    Value::I32(*orientation as i32),
                );
            }
            SystemEvent::Brightness { dark } => {
                map.insert(Value::String("dark".to_owned()), Value::Bool(*dark));
            }
            SystemEvent::PowerMode { low_power } => {
                map.insert(
                    Value::String("lowPower".to_owned()),
                    Value::Bool(*low_power),
                );
            }
            SystemEvent::RefreshRate { hz } => {
                map.insert(Value::String("hz".to_owned()), Value::I32(*hz as i32));
            }
        }
        Value::Map(map)
    }

    /// Decode a map produced by [`SystemEvent::to_value`]. Unknown types and
    /// malformed payloads return `None`, never a fabricated default event.
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Map(map) = value else {
            return None;
        };
        let code = match map.get(&Value::String("type".to_owned())) {
            Some(Value::String(code)) => code.as_str(),
            _ => return None,
        };
        let field = |name: &str| map.get(&Value::String(name.to_owned()));
        match code {
            "memoryPressure" => Some(SystemEvent::MemoryPressure),
            "host.keyboardVisibility" => Some(SystemEvent::KeyboardVisibility {
                visible: matches!(field("visible"), Some(Value::Bool(true))),
                height: match field("height") {
                    Some(Value::I32(h)) if *h >= 0 => Some(*h as u32),
                    _ => None,
                },
            }),
            "host.orientation" => match field("orientation") {
                Some(Value::I32(o)) if (0..=3).contains(o) => Some(SystemEvent::Orientation {
                    orientation: *o as u8,
                }),
                _ => None,
            },
            "host.brightness" => Some(SystemEvent::Brightness {
                dark: matches!(field("dark"), Some(Value::Bool(true))),
            }),
            "host.powerMode" => Some(SystemEvent::PowerMode {
                low_power: matches!(field("lowPower"), Some(Value::Bool(true))),
            }),
            "host.refreshRate" => match field("hz") {
                Some(Value::I32(hz)) if *hz > 0 => {
                    Some(SystemEvent::RefreshRate { hz: *hz as u32 })
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// The exact `flutter/system` channel name (`system_channels.dart:403`).
pub const SYSTEM_CHANNEL: &str = system_channels::SYSTEM;

/// A deterministic subscriber slot. No `Arc`, no dynamic dispatch: a slot is
/// one compile-time function pointer plus static state, like
/// `platform_channel::Handler`.
#[derive(Clone, Copy)]
pub struct Subscriber {
    pub function: unsafe fn(*mut u8, &SystemEvent),
    pub state: *mut u8,
}

impl std::fmt::Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscriber")
            .field("state", &self.state)
            .finish()
    }
}

/// Bounded event dispatcher implementing 0→1 activate / 1→0 deactivate
/// semantics of Flutter's broadcast streams
/// (`platform_channel.dart:695-737`), but with a compile-time subscriber list
/// instead of a runtime `StreamController`.
#[derive(Default)]
pub struct SystemEventDispatcher {
    subscribers: Vec<(SystemEvent, Subscriber)>,
    /// Last event per code, used to deduplicate identical host signals the
    /// same way `LifecycleTracker` dedupes lifecycle states.
    last: std::collections::BTreeMap<&'static str, SystemEvent>,
}

impl SystemEventDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a subscriber for exactly one event kind. Adding a second
    /// subscriber for an already-present event kind replaces it, which is
    /// the compile-time equivalent of Flutter's single message handler per
    /// channel (`binary_messenger.dart:66-80`).
    pub fn on(mut self, event: SystemEvent, subscriber: Subscriber) -> Self {
        if let Some(entry) = self
            .subscribers
            .iter_mut()
            .find(|(kind, _)| Self::same_kind(kind, &event))
        {
            entry.1 = subscriber;
        } else {
            self.subscribers.push((event, subscriber));
        }
        self
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    fn same_kind(a: &SystemEvent, b: &SystemEvent) -> bool {
        a.code() == b.code()
    }

    /// Dispatch one event. Returns `true` if a subscriber received it,
    /// `false` if no subscriber was registered. Identical consecutive events
    /// of the same kind are deduplicated (host signals often repeat).
    ///
    /// # Safety
    /// Each subscriber's state pointer must remain valid for this call.
    pub unsafe fn emit(&mut self, event: SystemEvent) -> bool {
        if self.last.get(event.code()) == Some(&event) {
            return false;
        }
        self.last.insert(event.code(), event.clone());
        let Some((_, subscriber)) = self
            .subscribers
            .iter()
            .find(|(kind, _)| Self::same_kind(kind, &event))
        else {
            return false;
        };
        unsafe { (subscriber.function)(subscriber.state, &event) };
        true
    }

    /// Forget the last value of an event kind, so the next identical signal
    /// is delivered again. Mirrors a stream being cancelled and re-listened.
    pub fn reset(&mut self, code: &str) {
        self.last.remove(code);
    }

    /// Feed one already-decoded host signal through this dispatcher. The
    /// producers in `event_producers` are pure; this is a deterministic
    /// composition of production and dispatch.
    ///
    /// # Safety
    /// Every subscriber state pointer in `self` must remain valid for this call.
    pub unsafe fn emit_host_event(&mut self, event: SystemEvent) -> bool {
        // SAFETY: documented by the caller.
        unsafe { self.emit(event) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_producers_feed_the_dispatcher_with_dedupe() {
        use crate::event_producers::{
            brightness, keyboard_visibility, memory_pressure, orientation_from_index, power_mode,
            refresh_rate,
        };

        unsafe fn collect(state: *mut u8, event: &SystemEvent) {
            // SAFETY: state points to a live Vec<SystemEvent> owned by the test.
            unsafe { &mut *(state as *mut Vec<SystemEvent>) }.push(event.clone());
        }

        let mut delivered = Vec::new();
        let state = &mut delivered as *mut Vec<SystemEvent> as *mut u8;
        let mut dispatcher = SystemEventDispatcher::new();
        for event in [
            memory_pressure(),
            keyboard_visibility(true, Some(300)),
            orientation_from_index(1).unwrap(),
            brightness(true),
            power_mode(true),
            refresh_rate(120),
        ] {
            dispatcher = dispatcher.on(
                event.clone(),
                Subscriber {
                    function: collect,
                    state,
                },
            );
        }
        let produced = [
            memory_pressure(),
            keyboard_visibility(true, Some(300)),
            orientation_from_index(1).unwrap(),
            brightness(true),
            power_mode(true),
            refresh_rate(120),
        ];
        for event in produced.iter().cloned() {
            // SAFETY: `delivered` outlives every call and owns the state.
            assert!(unsafe { dispatcher.emit_host_event(event) });
        }
        assert_eq!(delivered, produced);
        // Repeating an identical host signal is deduped by kind and value.
        assert!(!unsafe { dispatcher.emit_host_event(refresh_rate(120)) });
        assert_eq!(delivered.len(), 6);
        // A changed value of the same kind is delivered to the same slot.
        assert!(unsafe { dispatcher.emit_host_event(refresh_rate(144)) });
        assert_eq!(delivered.len(), 7);
        assert_eq!(dispatcher.subscriber_count(), 6);
    }

    #[test]
    fn memory_pressure_payload_matches_flutter() {
        let event = SystemEvent::MemoryPressure;
        assert_eq!(event.code(), "memoryPressure");
        assert!(event.is_flutter_framework_event());
        assert_eq!(
            event.to_value(),
            Value::Map(std::collections::BTreeMap::from([(
                Value::String("type".to_owned()),
                Value::String("memoryPressure".to_owned()),
            )]))
        );
        assert_eq!(SystemEvent::from_value(&event.to_value()), Some(event));
        assert_eq!(SYSTEM_CHANNEL, "flutter/system");
    }

    #[test]
    fn host_events_are_not_mislabeled_as_flutter_framework_api() {
        let events = [
            SystemEvent::KeyboardVisibility {
                visible: true,
                height: Some(300),
            },
            SystemEvent::Orientation { orientation: 1 },
            SystemEvent::Brightness { dark: true },
            SystemEvent::PowerMode { low_power: true },
            SystemEvent::RefreshRate { hz: 120 },
        ];
        for event in events {
            assert!(!event.is_flutter_framework_event());
            assert!(event.code().starts_with("host."));
            assert_eq!(SystemEvent::from_value(&event.to_value()), Some(event));
        }
    }

    #[test]
    fn malformed_payload_never_fabricates_an_event() {
        assert_eq!(SystemEvent::from_value(&Value::Null), None);
        assert_eq!(
            SystemEvent::from_value(&Value::Map(std::collections::BTreeMap::from([(
                Value::String("type".to_owned()),
                Value::String("unknown".to_owned()),
            )]))),
            None
        );
        assert_eq!(
            SystemEvent::from_value(&Value::Map(std::collections::BTreeMap::from([(
                Value::String("type".to_owned()),
                Value::String("host.orientation".to_owned()),
            )]))),
            None
        );
    }
}
