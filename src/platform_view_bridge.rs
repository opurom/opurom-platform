//! Platform-view method-channel bridge over the deterministic view registry.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `packages/flutter/lib/src/services/system_channels.dart:423,430` defines
//! `flutter/platform_views` and `flutter/platform_views_2`. The operations on
//! those channels are enumerated in `services/platform_views.dart`:
//! `create` (`:286,325,1359-1390`), `touch` (`:936,1255`),
//! `setDirection` (`:989`), `clearFocus` (`:1037`),
//! `acceptGesture`/`rejectGesture` (`:1405-1410,1567-1571,1615,1627`),
//! `resize` (`:1442`), `offset` (`:1472`), `dispose` (`:1481,1517,1560,1637`)
//! and the `_2`-channel capability query `isSurfaceControlEnabled`
//! (`:1529-1532`). The host-to-Dart `viewFocused` callback is the only
//! inbound method (`:78-85`).
//!
//! Platform-specific argument shapes are compile-time typed keys: Darwin
//! `create` carries `gestureBlockingPolicy` (`:257-306`, enum values
//! `:1672-1705`), Android `create` carries `direction`/`hybrid`/
//! `hybridFallback`/left/top (`:1359-1390`), and Android gesture rejection
//! carries an optional `gestureId` (`:1405-1410,1567-1571`). This module only
//! validates and normalizes those wire values; it never guesses a missing
//! platform capability.
//!
//! Opurom keeps the same wire operations but routes them to the existing
//! [`crate::platform_view::PlatformViewRegistry`]: a compile-time handle
//! table with strict lifecycle and deterministic compositing order. No
//! runtime view object tree or interpreter is introduced.

use crate::framework_channels::unimplemented;
use crate::platform_channel::{error_codes, MethodCall, Reply, Value};
use crate::platform_view::{PlatformViewError, PlatformViewRegistry, ViewBounds, ViewHandle};
use crate::wayland_wire::WireEvent;

/// Method names on `flutter/platform_views` and `flutter/platform_views_2`
/// (`services/platform_views.dart:286-1637`).
pub mod view_methods {
    pub const CREATE: &str = "create";
    pub const TOUCH: &str = "touch";
    pub const SET_DIRECTION: &str = "setDirection";
    pub const CLEAR_FOCUS: &str = "clearFocus";
    pub const ACCEPT_GESTURE: &str = "acceptGesture";
    pub const REJECT_GESTURE: &str = "rejectGesture";
    pub const RESIZE: &str = "resize";
    pub const OFFSET: &str = "offset";
    pub const DISPOSE: &str = "dispose";
    pub const IS_SURFACE_CONTROL_ENABLED: &str = "isSurfaceControlEnabled";
    pub const VIEW_FOCUSED: &str = "viewFocused";

    /// Argument keys (`platform_views.dart:286-1571`).
    pub const ARG_ID: &str = "id";
    pub const ARG_VIEW_TYPE: &str = "viewType";
    pub const ARG_WIDTH: &str = "width";
    pub const ARG_HEIGHT: &str = "height";
    pub const ARG_DIRECTION: &str = "direction";
    pub const ARG_X: &str = "x";
    pub const ARG_Y: &str = "y";
    /// Darwin `initUiKitView` / `initAppKitView` (`platform_views.dart:268-280`).
    pub const ARG_GESTURE_BLOCKING_POLICY: &str = "gestureBlockingPolicy";
    /// Android create/gesture messages (`:1365-1379,1406-1410,1567-1571`).
    pub const ARG_HYBRID: &str = "hybrid";
    pub const ARG_HYBRID_FALLBACK: &str = "hybridFallback";
    pub const ARG_LEFT: &str = "left";
    pub const ARG_TOP: &str = "top";
    pub const ARG_GESTURE_ID: &str = "gestureId";
    pub const ARG_PARAMS: &str = "params";

    /// `UiKitViewGestureBlockingPolicy` wire strings
    /// (`platform_views.dart:268-273,1672-1705`).
    pub mod gesture_blocking_policy {
        pub const EAGER: &str = "eager";
        pub const WAIT_UNTIL_TOUCHES_ENDED: &str = "waitUntilTouchesEnded";
        pub const FALLBACK_TO_PLUGIN_DEFAULT: &str = "fallbackToPluginDefault";
        pub const DO_NOT_BLOCK_GESTURE: &str = "doNotBlockGesture";

        pub fn all() -> &'static [&'static str] {
            &[
                EAGER,
                WAIT_UNTIL_TOUCHES_ENDED,
                FALLBACK_TO_PLUGIN_DEFAULT,
                DO_NOT_BLOCK_GESTURE,
            ]
        }
    }

    /// Android layout direction constants (`platform_views.dart:789-792,827-832`).
    pub mod direction {
        pub const LTR: i32 = 0;
        pub const RTL: i32 = 1;

        pub fn all() -> &'static [i32] {
            &[LTR, RTL]
        }
    }

    pub fn all() -> &'static [&'static str] {
        &[
            CREATE,
            TOUCH,
            SET_DIRECTION,
            CLEAR_FOCUS,
            ACCEPT_GESTURE,
            REJECT_GESTURE,
            RESIZE,
            OFFSET,
            DISPOSE,
            IS_SURFACE_CONTROL_ENABLED,
            VIEW_FOCUSED,
        ]
    }
}

fn invalid_argument(message: impl Into<String>) -> Reply {
    Reply::PlatformError {
        code: error_codes::INVALID_ARGUMENT.to_owned(),
        message: Some(message.into()),
        details: Value::Null,
    }
}

fn map_error(error: PlatformViewError) -> Reply {
    Reply::PlatformError {
        code: error_codes::ERROR.to_owned(),
        message: Some(error.to_string()),
        details: Value::Null,
    }
}

fn arg_number(arguments: &Value, name: &str) -> Option<i32> {
    let Value::Map(map) = arguments else {
        return None;
    };
    match Value::map_get_str(map, name) {
        Some(Value::I32(value)) => Some(*value),
        Some(Value::I64(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn arg_string(arguments: &Value, name: &str) -> Option<String> {
    let Value::Map(map) = arguments else {
        return None;
    };
    match Value::map_get_str(map, name) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn arg_value(arguments: &Value, name: &str) -> Option<Value> {
    let Value::Map(map) = arguments else {
        return None;
    };
    Value::map_get_str(map, name).cloned()
}

fn handle(arguments: &Value) -> Result<ViewHandle, Reply> {
    match arg_number(arguments, view_methods::ARG_ID) {
        Some(id) if id >= 0 => Ok(ViewHandle(id as u32)),
        _ => Err(invalid_argument("platform view requires a non-negative id")),
    }
}

/// A method-call facade over one view registry. The registry is the only
/// mutable state; method calls are pure functions of that state and their
/// arguments, so the same sequence always produces the same replies.
pub struct PlatformViewBridge<'a> {
    registry: &'a mut PlatformViewRegistry,
    /// Reply to `isSurfaceControlEnabled` from the injected host capability.
    /// Flutter's HCPP path is opt-in on Impeller Android and has no framework
    /// default (`platform_views.dart:1524-1532`); a host without the feature
    /// must report `false`, not claim support by default.
    surface_control_enabled: bool,
}

impl<'a> PlatformViewBridge<'a> {
    pub fn new(registry: &'a mut PlatformViewRegistry) -> Self {
        Self {
            registry,
            surface_control_enabled: false,
        }
    }

    /// Inject the host's HCPP capability (`platform_views.dart:1529-1532`).
    pub fn with_surface_control(mut self, enabled: bool) -> Self {
        self.surface_control_enabled = enabled;
        self
    }

    /// Call one method from [`view_methods::all`]. `isSurfaceControlEnabled`
    /// is a capability query on the `_2` channel and needs no view id.
    pub fn call(&mut self, call: &MethodCall) -> Reply {
        if !view_methods::all().contains(&call.method.as_str()) {
            return unimplemented(&call.method);
        }
        if call.method == view_methods::IS_SURFACE_CONTROL_ENABLED {
            return Reply::Success(Value::Bool(self.surface_control_enabled));
        }
        let target = match handle(&call.arguments) {
            Ok(target) => target,
            Err(reply) => return reply,
        };
        match call.method.as_str() {
            view_methods::CREATE => {
                let view_type = match &call.arguments {
                    Value::Map(map) => {
                        Value::map_get_str(map, view_methods::ARG_VIEW_TYPE).cloned()
                    }
                    _ => None,
                };
                if !matches!(view_type, Some(Value::String(_))) {
                    return invalid_argument("create requires a viewType");
                }
                // Darwin `initUiKitView`/`initAppKitView` send a closed
                // gesture-blocking policy string; Android `sendCreateMessage`
                // sends a 0/1 layout direction. Validate whichever shape is
                // present and reject values outside the Flutter vocabularies.
                if let Some(policy) =
                    arg_string(&call.arguments, view_methods::ARG_GESTURE_BLOCKING_POLICY)
                {
                    if !view_methods::gesture_blocking_policy::all().contains(&policy.as_str()) {
                        return invalid_argument("gestureBlockingPolicy is not a Flutter policy");
                    }
                }
                if let Some(direction) = arg_number(&call.arguments, view_methods::ARG_DIRECTION) {
                    if !view_methods::direction::all().contains(&direction) {
                        return invalid_argument("direction must be 0 or 1");
                    }
                }
                for key in [view_methods::ARG_HYBRID, view_methods::ARG_HYBRID_FALLBACK] {
                    if let Some(value) = arg_value(&call.arguments, key) {
                        if !matches!(value, Value::Bool(_)) {
                            return invalid_argument("hybrid flags must be booleans");
                        }
                    }
                }
                if let Some(params) = arg_value(&call.arguments, view_methods::ARG_PARAMS) {
                    if !matches!(params, Value::Uint8List(_)) {
                        return invalid_argument("params must be a uint8 list");
                    }
                }
                let width = arg_number(&call.arguments, view_methods::ARG_WIDTH)
                    .filter(|value| *value >= 0);
                let height = arg_number(&call.arguments, view_methods::ARG_HEIGHT)
                    .filter(|value| *value >= 0);
                let (Some(width), Some(height)) = (width, height) else {
                    return invalid_argument("create requires non-negative width and height");
                };
                match ViewBounds::new(0, 0, width, height) {
                    Ok(bounds) => {
                        if let Err(error) = self.registry.register(target, bounds, 0) {
                            return map_error(error);
                        }
                        match self.registry.attach(target) {
                            Ok(()) => Reply::Success(Value::Null),
                            Err(error) => map_error(error),
                        }
                    }
                    Err(error) => map_error(error),
                }
            }
            view_methods::RESIZE => {
                let width = arg_number(&call.arguments, view_methods::ARG_WIDTH)
                    .filter(|value| *value >= 0);
                let height = arg_number(&call.arguments, view_methods::ARG_HEIGHT)
                    .filter(|value| *value >= 0);
                let (Some(width), Some(height)) = (width, height) else {
                    return invalid_argument("resize requires non-negative width and height");
                };
                match ViewBounds::new(0, 0, width, height) {
                    Ok(bounds) => {
                        match self.registry.view(target).map(|state| state.lifecycle) {
                            Ok(crate::platform_view::ViewLifecycle::Created) => {
                                if let Err(error) = self.registry.attach(target) {
                                    return map_error(error);
                                }
                            }
                            Ok(crate::platform_view::ViewLifecycle::InputForwarded) => {
                                // Input already advanced the cursor; the registry
                                // keeps a strict one-way lifecycle, so a later
                                // resize is an idempotent size ack. The host can
                                // dispose and recreate a view to restart it.
                                return Reply::Success(Value::Null);
                            }
                            Ok(crate::platform_view::ViewLifecycle::Disposed) => {
                                return map_error(PlatformViewError::Disposed(target));
                            }
                            Ok(_) | Err(_) => {}
                        }
                        match self.registry.resize(target, bounds) {
                            Ok(()) => Reply::Success(Value::Null),
                            Err(error) => map_error(error),
                        }
                    }
                    Err(error) => map_error(error),
                }
            }
            view_methods::TOUCH => {
                let x = match arg_number(&call.arguments, view_methods::ARG_X) {
                    Some(value) => value,
                    None => return invalid_argument("touch requires x"),
                };
                let y = match arg_number(&call.arguments, view_methods::ARG_Y) {
                    Some(value) => value,
                    None => return invalid_argument("touch requires y"),
                };
                let (lifecycle, bounds) = match self.registry.view(target) {
                    Ok(state) => (state.lifecycle, state.bounds),
                    Err(error) => return map_error(error),
                };
                if lifecycle == crate::platform_view::ViewLifecycle::Attached {
                    // The registered bounds came from `ViewBounds::new`, so
                    // the typed value can be reused without revalidation.
                    if let Err(error) = self.registry.resize(target, bounds) {
                        return map_error(error);
                    }
                }
                let event = WireEvent::Press { x, y };
                match self.registry.forward_input(target, event) {
                    Ok(_) => Reply::Success(Value::Null),
                    Err(error) => map_error(error),
                }
            }
            view_methods::CLEAR_FOCUS => {
                let event = WireEvent::Focus { focused: false };
                match self.registry.forward_input(target, event) {
                    Ok(_) => Reply::Success(Value::Null),
                    Err(error) => map_error(error),
                }
            }
            view_methods::SET_DIRECTION => {
                let direction = arg_number(&call.arguments, view_methods::ARG_DIRECTION);
                if !matches!(direction, Some(direction) if view_methods::direction::all().contains(&direction))
                {
                    return invalid_argument("direction must be 0 or 1");
                }
                Reply::Success(Value::Null)
            }
            view_methods::ACCEPT_GESTURE => Reply::Success(Value::Null),
            view_methods::REJECT_GESTURE => {
                if let Some(gesture_id) = arg_value(&call.arguments, view_methods::ARG_GESTURE_ID) {
                    if !matches!(gesture_id, Value::I32(_) | Value::I64(_)) {
                        return invalid_argument("gestureId must be an integer");
                    }
                }
                Reply::Success(Value::Null)
            }
            view_methods::OFFSET => {
                // Android sends `left`/`top` doubles; Darwin has no offset
                // operation. Validate the shape without mutating bounds: the
                // view registry keeps integer layout, not native view offsets.
                for key in [view_methods::ARG_LEFT, view_methods::ARG_TOP] {
                    if let Some(value) = arg_value(&call.arguments, key) {
                        if !matches!(value, Value::F64(_)) {
                            return invalid_argument("offset requires double left/top");
                        }
                    }
                }
                Reply::Success(Value::Null)
            }
            view_methods::DISPOSE => match self.registry.dispose(target) {
                Ok(()) => Reply::Success(Value::Null),
                Err(error) => map_error(error),
            },
            view_methods::VIEW_FOCUSED => {
                // Inbound host callback (`:78-85`): `arguments` is the view id
                // and the callback is only dispatched when the id has a
                // registered focus handler (`:82-84`). The bridge validates
                // the id; the outer binding owns the handler table.
                match self.registry.view(target) {
                    Ok(state)
                        if state.lifecycle != crate::platform_view::ViewLifecycle::Disposed =>
                    {
                        Reply::Success(Value::Null)
                    }
                    Ok(_) => map_error(PlatformViewError::Disposed(target)),
                    Err(error) => map_error(error),
                }
            }
            _ => unimplemented(&call.method),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn call_method(bridge: &mut PlatformViewBridge<'_>, method: &str, args: Value) -> Reply {
        bridge.call(&MethodCall {
            method: method.to_owned(),
            arguments: args,
        })
    }

    fn args(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(k, v)| (Value::String((*k).to_owned()), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    #[test]
    fn create_touch_resize_and_dispose_follow_the_view_lifecycle() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry);
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::CREATE,
                args(&[
                    (view_methods::ARG_ID, Value::I32(7)),
                    (view_methods::ARG_VIEW_TYPE, Value::String("maps".into())),
                    (view_methods::ARG_WIDTH, Value::I32(320)),
                    (view_methods::ARG_HEIGHT, Value::I32(200)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::TOUCH,
                args(&[
                    (view_methods::ARG_ID, Value::I32(7)),
                    (view_methods::ARG_X, Value::I32(12)),
                    (view_methods::ARG_Y, Value::I32(34)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::RESIZE,
                args(&[
                    (view_methods::ARG_ID, Value::I32(7)),
                    (view_methods::ARG_WIDTH, Value::I32(640)),
                    (view_methods::ARG_HEIGHT, Value::I32(400)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::DISPOSE,
                args(&[(view_methods::ARG_ID, Value::I32(7))]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            registry.lifecycle(ViewHandle(7)),
            Ok(crate::platform_view::ViewLifecycle::Disposed)
        );
    }

    #[test]
    fn darwin_create_validates_gesture_blocking_policy_and_params() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry);
        let base = [
            (view_methods::ARG_ID, Value::I32(11)),
            (view_methods::ARG_VIEW_TYPE, Value::String("ui-view".into())),
            (view_methods::ARG_WIDTH, Value::I32(240)),
            (view_methods::ARG_HEIGHT, Value::I32(180)),
            (
                view_methods::ARG_GESTURE_BLOCKING_POLICY,
                Value::String(view_methods::gesture_blocking_policy::EAGER.into()),
            ),
            (view_methods::ARG_PARAMS, Value::Uint8List(vec![1, 2, 3])),
        ];
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, args(&base)),
            Reply::Success(Value::Null)
        );
        let mut bad = base.to_vec();
        bad[0].1 = Value::I32(12);
        bad[4].1 = Value::String("unknown".into());
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, args(&bad)),
            invalid_argument("gestureBlockingPolicy is not a Flutter policy")
        );
        assert_eq!(
            registry.lifecycle(ViewHandle(12)),
            Err(PlatformViewError::UnregisteredHandle(ViewHandle(12)))
        );
    }

    #[test]
    fn android_create_and_direction_validate_closed_cross_platform_vocabulary() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry);
        let create = args(&[
            (view_methods::ARG_ID, Value::I32(21)),
            (view_methods::ARG_VIEW_TYPE, Value::String("maps".into())),
            (view_methods::ARG_WIDTH, Value::I32(320)),
            (view_methods::ARG_HEIGHT, Value::I32(200)),
            (
                view_methods::ARG_DIRECTION,
                Value::I32(view_methods::direction::RTL),
            ),
            (view_methods::ARG_HYBRID, Value::Bool(true)),
            (view_methods::ARG_HYBRID_FALLBACK, Value::Bool(false)),
            (view_methods::ARG_LEFT, Value::F64(10.5)),
            (view_methods::ARG_TOP, Value::F64(20.0)),
        ]);
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, create.clone()),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::SET_DIRECTION,
                args(&[
                    (view_methods::ARG_ID, Value::I32(21)),
                    (view_methods::ARG_DIRECTION, Value::I32(1)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::SET_DIRECTION,
                args(&[
                    (view_methods::ARG_ID, Value::I32(21)),
                    (view_methods::ARG_DIRECTION, Value::I32(3)),
                ]),
            ),
            invalid_argument("direction must be 0 or 1")
        );
        let mut bad_create = create;
        if let Value::Map(map) = &mut bad_create {
            map.insert(
                Value::String(view_methods::ARG_HYBRID.into()),
                Value::String("true".into()),
            );
        }
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, bad_create),
            invalid_argument("hybrid flags must be booleans")
        );
    }

    #[test]
    fn surface_control_query_and_gesture_arguments_follow_flutter_shapes() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry).with_surface_control(true);
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::IS_SURFACE_CONTROL_ENABLED,
                Value::Map(BTreeMap::new()),
            ),
            Reply::Success(Value::Bool(true))
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::REJECT_GESTURE,
                args(&[
                    (view_methods::ARG_ID, Value::I32(31)),
                    (view_methods::ARG_GESTURE_ID, Value::I32(9)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::REJECT_GESTURE,
                args(&[
                    (view_methods::ARG_ID, Value::I32(31)),
                    (view_methods::ARG_GESTURE_ID, Value::String("9".into())),
                ]),
            ),
            invalid_argument("gestureId must be an integer")
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::ACCEPT_GESTURE,
                args(&[(view_methods::ARG_ID, Value::I32(31))]),
            ),
            Reply::Success(Value::Null)
        );
    }

    #[test]
    fn offset_validates_android_left_top_and_focus_callback_rejects_disposed_views() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry);
        let create = args(&[
            (view_methods::ARG_ID, Value::I32(41)),
            (view_methods::ARG_VIEW_TYPE, Value::String("maps".into())),
            (view_methods::ARG_WIDTH, Value::I32(100)),
            (view_methods::ARG_HEIGHT, Value::I32(100)),
        ]);
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, create),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::OFFSET,
                args(&[
                    (view_methods::ARG_ID, Value::I32(41)),
                    (view_methods::ARG_LEFT, Value::F64(1.0)),
                    (view_methods::ARG_TOP, Value::F64(2.0)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::OFFSET,
                args(&[
                    (view_methods::ARG_ID, Value::I32(41)),
                    (view_methods::ARG_LEFT, Value::I32(1)),
                ]),
            ),
            invalid_argument("offset requires double left/top")
        );
        call_method(
            &mut bridge,
            view_methods::VIEW_FOCUSED,
            args(&[(view_methods::ARG_ID, Value::I32(41))]),
        );
        // `viewFocused` is a host callback, not a lifecycle input event: it
        // observes focus without advancing the registry cursor.
        assert_eq!(
            bridge.registry.lifecycle(ViewHandle(41)),
            Ok(crate::platform_view::ViewLifecycle::Attached)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::TOUCH,
                args(&[
                    (view_methods::ARG_ID, Value::I32(41)),
                    (view_methods::ARG_X, Value::I32(5)),
                    (view_methods::ARG_Y, Value::I32(6)),
                ]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::DISPOSE,
                args(&[(view_methods::ARG_ID, Value::I32(41))]),
            ),
            Reply::Success(Value::Null)
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::VIEW_FOCUSED,
                args(&[(view_methods::ARG_ID, Value::I32(41))]),
            ),
            map_error(PlatformViewError::Disposed(ViewHandle(41)))
        );
    }

    #[test]
    fn malformed_calls_are_rejected_without_mutating_state() {
        let mut registry = PlatformViewRegistry::new();
        let mut bridge = PlatformViewBridge::new(&mut registry);
        assert_eq!(
            call_method(&mut bridge, view_methods::CREATE, Value::Null),
            invalid_argument("platform view requires a non-negative id")
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::CREATE,
                args(&[(view_methods::ARG_ID, Value::I32(-1))]),
            ),
            invalid_argument("platform view requires a non-negative id")
        );
        assert_eq!(
            call_method(
                &mut bridge,
                view_methods::CREATE,
                args(&[
                    (view_methods::ARG_ID, Value::I32(1)),
                    (view_methods::ARG_VIEW_TYPE, Value::String("maps".into())),
                ]),
            ),
            invalid_argument("create requires non-negative width and height")
        );
        assert_eq!(
            call_method(&mut bridge, "unknown", args(&[])),
            unimplemented("unknown")
        );
        assert!(registry.is_empty());
    }
}
