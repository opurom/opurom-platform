//! Deterministic `PlatformView` bridge contract (Flutter `PlatformView` model).
//!
//! The registry owns native-view handles, integer logical-pixel bounds and a
//! stable compositing order relative to self-drawn nodes. Lifecycle state is
//! strictly sequential: `created` -> `attached` -> `resized` ->
//! `input-forwarded` -> `disposed`. Pointer and keyboard-focus events are
//! typed as the existing `wayland_wire::WireEvent` encoder, so the same bytes
//! travel to every native host. Handle encoding is a fixed 8-byte frame:
//! the `OPV1` magic followed by a little-endian 32-bit handle.

use std::collections::BTreeMap;
use std::fmt;

use crate::wayland_wire::WireEvent;

/// Handle frame magic, shared byte-for-byte with the mobile JNI/iOS contract.
pub const HANDLE_MAGIC: [u8; 4] = *b"OPV1";
/// A platform-view handle frame is four magic bytes plus a 32-bit handle.
pub const HANDLE_LEN: usize = 8;
/// Logical-pixel Z-order of self-drawn nodes; negative values compose behind
/// and positive values compose in front of that layer.
pub const SELF_DRAWN_Z_ORDER: i32 = 0;

/// Opaque native-view handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ViewHandle(pub u32);

/// Integer logical-pixel bounds. Every component must be non-negative.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The strict lifecycle state of one registered platform view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViewLifecycle {
    Created,
    Attached,
    Resized,
    InputForwarded,
    Disposed,
}

impl ViewLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewLifecycle::Created => "created",
            ViewLifecycle::Attached => "attached",
            ViewLifecycle::Resized => "resized",
            ViewLifecycle::InputForwarded => "input-forwarded",
            ViewLifecycle::Disposed => "disposed",
        }
    }

    /// Only the documented one-step advance is legal.
    pub fn can_advance_to(self, next: ViewLifecycle) -> bool {
        match self {
            ViewLifecycle::Created => next == ViewLifecycle::Attached,
            ViewLifecycle::Attached => next == ViewLifecycle::Resized,
            ViewLifecycle::Resized => next == ViewLifecycle::InputForwarded,
            ViewLifecycle::InputForwarded => next == ViewLifecycle::Disposed,
            ViewLifecycle::Disposed => false,
        }
    }
}

/// A registered view's deterministic state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewState {
    pub handle: ViewHandle,
    pub bounds: ViewBounds,
    pub z_order: i32,
    pub lifecycle: ViewLifecycle,
    order: u32,
}

/// Registry and lifecycle errors are values, never panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformViewError {
    DuplicateHandle(ViewHandle),
    UnregisteredHandle(ViewHandle),
    NegativeBounds,
    InvalidHandle,
    InvalidInput(ViewHandle),
    OutOfOrderLifecycle {
        handle: ViewHandle,
        from: ViewLifecycle,
        to: ViewLifecycle,
    },
    Disposed(ViewHandle),
}

impl fmt::Display for PlatformViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformViewError::DuplicateHandle(handle) => {
                write!(f, "platform view handle {} is already registered", handle.0)
            }
            PlatformViewError::UnregisteredHandle(handle) => {
                write!(f, "platform view handle {} is not registered", handle.0)
            }
            PlatformViewError::NegativeBounds => {
                write!(
                    f,
                    "platform view bounds must not contain a negative logical pixel"
                )
            }
            PlatformViewError::InvalidHandle => {
                write!(f, "platform view handle frame is not an 8-byte OPV1 frame")
            }
            PlatformViewError::InvalidInput(handle) => {
                write!(
                    f,
                    "platform view handle {} does not accept this input event",
                    handle.0
                )
            }
            PlatformViewError::OutOfOrderLifecycle { handle, from, to } => write!(
                f,
                "platform view handle {} cannot move from {} to {}",
                handle.0,
                from.as_str(),
                to.as_str()
            ),
            PlatformViewError::Disposed(handle) => {
                write!(
                    f,
                    "platform view handle {} is disposed and no longer accepts input",
                    handle.0
                )
            }
        }
    }
}

impl std::error::Error for PlatformViewError {}

impl ViewHandle {
    /// Encode the handle as the shared `OPV1` little-endian frame.
    pub fn encode(self) -> [u8; HANDLE_LEN] {
        let mut bytes = [0u8; HANDLE_LEN];
        bytes[..4].copy_from_slice(&HANDLE_MAGIC);
        bytes[4..].copy_from_slice(&self.0.to_le_bytes());
        bytes
    }

    /// Decode the shared `OPV1` little-endian frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, PlatformViewError> {
        if bytes.len() != HANDLE_LEN || bytes[..4] != HANDLE_MAGIC {
            return Err(PlatformViewError::InvalidHandle);
        }
        let raw = [bytes[4], bytes[5], bytes[6], bytes[7]];
        Ok(ViewHandle(u32::from_le_bytes(raw)))
    }
}

impl ViewBounds {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Result<Self, PlatformViewError> {
        if x < 0 || y < 0 || width < 0 || height < 0 {
            return Err(PlatformViewError::NegativeBounds);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
}

/// Headless-testable handle registry with a strict lifecycle and stable
/// compositing order.
#[derive(Debug)]
pub struct PlatformViewRegistry {
    views: BTreeMap<ViewHandle, ViewState>,
    next_order: u32,
}

impl PlatformViewRegistry {
    pub fn new() -> Self {
        Self {
            views: BTreeMap::new(),
            next_order: 0,
        }
    }

    /// Register a new view in the `created` state. Duplicate handles and
    /// negative logical-pixel bounds are rejected.
    pub fn register(
        &mut self,
        handle: ViewHandle,
        bounds: ViewBounds,
        z_order: i32,
    ) -> Result<(), PlatformViewError> {
        if self.views.contains_key(&handle) {
            return Err(PlatformViewError::DuplicateHandle(handle));
        }
        let state = ViewState {
            handle,
            bounds,
            z_order,
            lifecycle: ViewLifecycle::Created,
            order: self.next_order,
        };
        self.next_order = self.next_order.saturating_add(1);
        self.views.insert(handle, state);
        Ok(())
    }

    /// Remove a view and return its final state. Unregistered handles are rejected.
    pub fn unregister(&mut self, handle: ViewHandle) -> Result<ViewState, PlatformViewError> {
        self.views
            .remove(&handle)
            .ok_or(PlatformViewError::UnregisteredHandle(handle))
    }

    pub fn view(&self, handle: ViewHandle) -> Result<&ViewState, PlatformViewError> {
        self.views
            .get(&handle)
            .ok_or(PlatformViewError::UnregisteredHandle(handle))
    }

    pub fn lifecycle(&self, handle: ViewHandle) -> Result<ViewLifecycle, PlatformViewError> {
        Ok(self.view(handle)?.lifecycle)
    }

    pub fn z_order(&self, handle: ViewHandle) -> Result<i32, PlatformViewError> {
        Ok(self.view(handle)?.z_order)
    }

    /// Move a live view to a new Z-order. Disposed views are terminal.
    pub fn set_z_order(
        &mut self,
        handle: ViewHandle,
        z_order: i32,
    ) -> Result<(), PlatformViewError> {
        let state = self.view_mut(handle)?;
        if state.lifecycle == ViewLifecycle::Disposed {
            return Err(PlatformViewError::Disposed(handle));
        }
        state.z_order = z_order;
        Ok(())
    }

    pub fn attach(&mut self, handle: ViewHandle) -> Result<(), PlatformViewError> {
        self.advance(handle, ViewLifecycle::Attached)
    }

    pub fn resize(
        &mut self,
        handle: ViewHandle,
        bounds: ViewBounds,
    ) -> Result<(), PlatformViewError> {
        let state = self.view_mut(handle)?;
        if !state.lifecycle.can_advance_to(ViewLifecycle::Resized) {
            return Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: state.lifecycle,
                to: ViewLifecycle::Resized,
            });
        }
        state.bounds = bounds;
        state.lifecycle = ViewLifecycle::Resized;
        Ok(())
    }

    /// Forward a pointer or keyboard-focus event and advance the lifecycle on
    /// the first input. Resize and geometry events are window state, not input.
    /// A disposed view stops receiving input.
    pub fn forward_input(
        &mut self,
        handle: ViewHandle,
        event: WireEvent,
    ) -> Result<WireEvent, PlatformViewError> {
        let state = self.view_mut(handle)?;
        if state.lifecycle == ViewLifecycle::Disposed {
            return Err(PlatformViewError::Disposed(handle));
        }
        if matches!(
            event,
            WireEvent::Resize {
                width: _,
                height: _
            } | WireEvent::Geometry(_)
        ) {
            return Err(PlatformViewError::InvalidInput(handle));
        }
        if !state
            .lifecycle
            .can_advance_to(ViewLifecycle::InputForwarded)
            && state.lifecycle != ViewLifecycle::InputForwarded
        {
            return Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: state.lifecycle,
                to: ViewLifecycle::InputForwarded,
            });
        }
        state.lifecycle = ViewLifecycle::InputForwarded;
        Ok(event)
    }

    pub fn dispose(&mut self, handle: ViewHandle) -> Result<(), PlatformViewError> {
        self.advance(handle, ViewLifecycle::Disposed)
    }

    /// Compositing order of live views, ascending by Z-order and insertion
    /// order. Disposed views are excluded; repeated calls are stable.
    pub fn composite_order(&self) -> Vec<ViewHandle> {
        let mut order: Vec<&ViewState> = self
            .views
            .values()
            .filter(|state| state.lifecycle != ViewLifecycle::Disposed)
            .collect();
        order.sort_by(|left, right| {
            left.z_order
                .cmp(&right.z_order)
                .then_with(|| left.order.cmp(&right.order))
        });
        order.into_iter().map(|state| state.handle).collect()
    }

    pub fn len(&self) -> usize {
        self.views.len()
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    fn view_mut(&mut self, handle: ViewHandle) -> Result<&mut ViewState, PlatformViewError> {
        self.views
            .get_mut(&handle)
            .ok_or(PlatformViewError::UnregisteredHandle(handle))
    }

    fn advance(
        &mut self,
        handle: ViewHandle,
        next: ViewLifecycle,
    ) -> Result<(), PlatformViewError> {
        let state = self.view_mut(handle)?;
        if !state.lifecycle.can_advance_to(next) {
            return Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: state.lifecycle,
                to: next,
            });
        }
        state.lifecycle = next;
        Ok(())
    }
}

impl Default for PlatformViewRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wayland_wire::WireEvent;
    use crate::{DisplayGeometry, Posture, WindowRole, DPI_UNIT};

    fn bounds(width: i32, height: i32) -> ViewBounds {
        ViewBounds::new(0, 0, width, height).unwrap()
    }

    fn ready(registry: &mut PlatformViewRegistry, handle: u32, z_order: i32) {
        registry
            .register(ViewHandle(handle), bounds(320, 240), z_order)
            .unwrap();
        registry.attach(ViewHandle(handle)).unwrap();
        registry
            .resize(ViewHandle(handle), bounds(640, 480))
            .unwrap();
        registry
            .forward_input(ViewHandle(handle), WireEvent::Move { x: 0, y: 0 })
            .unwrap();
    }

    #[test]
    fn handle_frame_is_the_shared_fixed_byte_encoding() {
        assert_eq!(HANDLE_MAGIC, *b"OPV1");
        assert_eq!(HANDLE_LEN, 8);
        assert_eq!(
            ViewHandle(0x1234_5678).encode(),
            [0x4f, 0x50, 0x56, 0x31, 0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            ViewHandle(1).encode(),
            [0x4f, 0x50, 0x56, 0x31, 0x01, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            ViewHandle(u32::MAX).encode(),
            [0x4f, 0x50, 0x56, 0x31, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            ViewHandle::decode(&ViewHandle(0x1234_5678).encode()).unwrap(),
            ViewHandle(0x1234_5678)
        );
        assert_eq!(
            ViewHandle::decode(&[0x4f, 0x50, 0x56, 0x32, 0, 0, 0, 0]),
            Err(PlatformViewError::InvalidHandle)
        );
        assert_eq!(
            ViewHandle::decode(&ViewHandle(1).encode()[..7]),
            Err(PlatformViewError::InvalidHandle)
        );
    }

    #[test]
    fn bounds_reject_every_negative_logical_pixel() {
        for (x, y, width, height) in [
            (-1, 0, 10, 10),
            (0, -1, 10, 10),
            (0, 0, -1, 10),
            (0, 0, 10, -1),
        ] {
            assert_eq!(
                ViewBounds::new(x, y, width, height),
                Err(PlatformViewError::NegativeBounds)
            );
        }
        assert_eq!(bounds(0, 0).width, 0);
        assert_eq!(
            ViewBounds::new(1, 2, 3, 4).unwrap(),
            ViewBounds {
                x: 1,
                y: 2,
                width: 3,
                height: 4
            }
        );
    }

    #[test]
    fn register_sets_created_state_and_rejects_duplicates() {
        let mut registry = PlatformViewRegistry::new();
        let handle = ViewHandle(7);
        registry.register(handle, bounds(320, 240), 2).unwrap();
        assert_eq!(
            registry.view(handle).unwrap().lifecycle,
            ViewLifecycle::Created
        );
        assert_eq!(registry.view(handle).unwrap().z_order, 2);
        assert_eq!(
            registry.register(handle, bounds(10, 10), 0),
            Err(PlatformViewError::DuplicateHandle(handle))
        );
        assert_eq!(registry.len(), 1);
        assert!(registry
            .register(handle, bounds(320, 240), 0)
            .unwrap_err()
            .to_string()
            .contains("is already registered"));
    }

    #[test]
    fn unregister_returns_state_and_unregistered_handles_are_rejected() {
        let mut registry = PlatformViewRegistry::new();
        ready(&mut registry, 1, 0);
        let removed = registry.unregister(ViewHandle(1)).unwrap();
        assert_eq!(removed.lifecycle, ViewLifecycle::InputForwarded);
        assert_eq!(registry.len(), 0);
        assert_eq!(
            registry.unregister(ViewHandle(1)),
            Err(PlatformViewError::UnregisteredHandle(ViewHandle(1)))
        );
        assert_eq!(
            registry.view(ViewHandle(1)),
            Err(PlatformViewError::UnregisteredHandle(ViewHandle(1)))
        );
        assert_eq!(
            registry.lifecycle(ViewHandle(1)),
            Err(PlatformViewError::UnregisteredHandle(ViewHandle(1)))
        );
        assert_eq!(
            registry.z_order(ViewHandle(1)),
            Err(PlatformViewError::UnregisteredHandle(ViewHandle(1)))
        );
    }

    #[test]
    fn lifecycle_advances_only_in_the_documented_order() {
        let mut registry = PlatformViewRegistry::new();
        let handle = ViewHandle(3);
        registry.register(handle, bounds(10, 10), 0).unwrap();
        assert_eq!(registry.lifecycle(handle).unwrap(), ViewLifecycle::Created);

        // resize before attach is out of order.
        assert_eq!(
            registry.resize(handle, bounds(20, 20)),
            Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: ViewLifecycle::Created,
                to: ViewLifecycle::Resized
            })
        );
        // forwarding input before resize is out of order.
        assert_eq!(
            registry.forward_input(handle, WireEvent::Press { x: 0, y: 0 }),
            Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: ViewLifecycle::Created,
                to: ViewLifecycle::InputForwarded
            })
        );
        // dispose before input is out of order.
        assert_eq!(
            registry.dispose(handle),
            Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: ViewLifecycle::Created,
                to: ViewLifecycle::Disposed
            })
        );

        registry.attach(handle).unwrap();
        assert_eq!(registry.lifecycle(handle).unwrap(), ViewLifecycle::Attached);
        registry.resize(handle, bounds(20, 20)).unwrap();
        assert_eq!(registry.lifecycle(handle).unwrap(), ViewLifecycle::Resized);
        registry
            .forward_input(handle, WireEvent::Move { x: 1, y: 2 })
            .unwrap();
        assert_eq!(
            registry.lifecycle(handle).unwrap(),
            ViewLifecycle::InputForwarded
        );
        registry.dispose(handle).unwrap();
        assert_eq!(registry.lifecycle(handle).unwrap(), ViewLifecycle::Disposed);
        assert_eq!(
            registry.dispose(handle),
            Err(PlatformViewError::OutOfOrderLifecycle {
                handle,
                from: ViewLifecycle::Disposed,
                to: ViewLifecycle::Disposed
            })
        );
    }

    #[test]
    fn disposed_views_stop_receiving_input_and_compositing() {
        let mut registry = PlatformViewRegistry::new();
        ready(&mut registry, 4, 1);
        registry
            .forward_input(ViewHandle(4), WireEvent::Focus { focused: true })
            .unwrap();
        registry.dispose(ViewHandle(4)).unwrap();

        assert_eq!(
            registry.forward_input(ViewHandle(4), WireEvent::Focus { focused: false }),
            Err(PlatformViewError::Disposed(ViewHandle(4)))
        );
        assert_eq!(
            registry.forward_input(ViewHandle(4), WireEvent::Press { x: 1, y: 1 }),
            Err(PlatformViewError::Disposed(ViewHandle(4)))
        );
        assert!(registry.composite_order().is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn resize_events_are_not_platform_view_input() {
        let mut registry = PlatformViewRegistry::new();
        let handle = ViewHandle(5);
        registry.register(handle, bounds(10, 10), 0).unwrap();
        registry.attach(handle).unwrap();
        registry.resize(handle, bounds(20, 20)).unwrap();
        assert_eq!(
            registry.forward_input(
                handle,
                WireEvent::Resize {
                    width: 10,
                    height: 10
                }
            ),
            Err(PlatformViewError::InvalidInput(handle))
        );
        let geometry =
            DisplayGeometry::new(320, 240, DPI_UNIT, Posture::Flat, WindowRole::Split, None)
                .unwrap();
        assert_eq!(
            registry.forward_input(handle, WireEvent::Geometry(geometry)),
            Err(PlatformViewError::InvalidInput(handle))
        );
        assert_eq!(registry.lifecycle(handle).unwrap(), ViewLifecycle::Resized);
    }

    #[test]
    fn z_order_is_stable_and_relative_to_self_drawn_nodes() {
        let mut registry = PlatformViewRegistry::new();
        registry
            .register(ViewHandle(1), bounds(10, 10), -1)
            .unwrap();
        registry.register(ViewHandle(2), bounds(10, 10), 1).unwrap();
        registry
            .register(ViewHandle(3), bounds(10, 10), SELF_DRAWN_Z_ORDER)
            .unwrap();
        registry
            .register(ViewHandle(4), bounds(10, 10), SELF_DRAWN_Z_ORDER)
            .unwrap();

        assert_eq!(
            registry.composite_order(),
            vec![ViewHandle(1), ViewHandle(3), ViewHandle(4), ViewHandle(2)]
        );
        // Equal Z-order preserves registration order; repeated calls are stable.
        assert_eq!(registry.composite_order(), registry.composite_order());

        registry.set_z_order(ViewHandle(1), 2).unwrap();
        assert_eq!(
            registry.composite_order(),
            vec![ViewHandle(3), ViewHandle(4), ViewHandle(2), ViewHandle(1)]
        );

        // A disposed view leaves the composite and never returns.
        ready(&mut registry, 5, 0);
        registry.dispose(ViewHandle(5)).unwrap();
        assert!(registry
            .composite_order()
            .iter()
            .all(|handle| *handle != ViewHandle(5)));
        assert_eq!(
            registry.set_z_order(ViewHandle(5), 9),
            Err(PlatformViewError::Disposed(ViewHandle(5)))
        );
    }

    #[test]
    fn input_events_are_typed_by_the_wire_encoder() {
        let mut registry = PlatformViewRegistry::new();
        ready(&mut registry, 6, 0);
        let pressed = registry
            .forward_input(ViewHandle(6), WireEvent::Press { x: 4, y: 5 })
            .unwrap();
        assert_eq!(pressed, WireEvent::Press { x: 4, y: 5 });
        let focused = registry
            .forward_input(ViewHandle(6), WireEvent::Focus { focused: true })
            .unwrap();
        assert_eq!(focused, WireEvent::Focus { focused: true });

        let press_bytes = pressed.encode().unwrap();
        assert_eq!(
            crate::wayland_wire::decode_event(&press_bytes)
                .unwrap()
                .unwrap()
                .0,
            pressed
        );
        let focus_bytes = focused.encode().unwrap();
        assert_eq!(
            crate::wayland_wire::decode_event(&focus_bytes)
                .unwrap()
                .unwrap()
                .0,
            focused
        );
    }
}
