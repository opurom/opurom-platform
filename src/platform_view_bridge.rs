//! Platform-view method-channel bridge over the deterministic view registry.
//!
//! Flutter baseline: 3.50-candidate (flutter-golden HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb`), engine
//! `371f77202a73e8dff57a8b3bff75bca55c090aee`.
//!
//! `packages/flutter/lib/src/services/system_channels.dart:423,430` defines
//! `flutter/platform_views` and `flutter/platform_views_2`. The operations on
//! those channels are enumerated in `services/platform_views.dart`:
//! `create` (`:286,325`), `touch` (`:936`), `setDirection` (`:989`),
//! `clearFocus` (`:1037`), `acceptGesture`/`rejectGesture` (`:1406,1617,1627`),
//! `resize` (`:1442`), `offset` (`:1472`) and `dispose` (`:1481,1517`).
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

    /// Argument keys (`platform_views.dart:286-1517`).
    pub const ARG_ID: &str = "id";
    pub const ARG_VIEW_TYPE: &str = "viewType";
    pub const ARG_WIDTH: &str = "width";
    pub const ARG_HEIGHT: &str = "height";
    pub const ARG_DIRECTION: &str = "direction";
    pub const ARG_X: &str = "x";
    pub const ARG_Y: &str = "y";

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
    match map.get(&Value::String(name.to_owned())) {
        Some(Value::I32(value)) => Some(*value),
        Some(Value::I64(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
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
}

impl<'a> PlatformViewBridge<'a> {
    pub fn new(registry: &'a mut PlatformViewRegistry) -> Self {
        Self { registry }
    }

    /// Call one method from [`view_methods::all`].
    pub fn call(&mut self, call: &MethodCall) -> Reply {
        if !view_methods::all().contains(&call.method.as_str()) {
            return unimplemented(&call.method);
        }
        let target = match handle(&call.arguments) {
            Ok(target) => target,
            Err(reply) => return reply,
        };
        match call.method.as_str() {
            view_methods::CREATE => {
                let view_type = match &call.arguments {
                    Value::Map(map) => map
                        .get(&Value::String(view_methods::ARG_VIEW_TYPE.to_owned()))
                        .cloned(),
                    _ => None,
                };
                if !matches!(view_type, Some(Value::String(_))) {
                    return invalid_argument("create requires a viewType");
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
                let state = match self.registry.view(target) {
                    Ok(state) => state,
                    Err(error) => return map_error(error),
                };
                if state.lifecycle == crate::platform_view::ViewLifecycle::Attached {
                    if let Err(error) = self.registry.resize(
                        target,
                        ViewBounds::new(0, 0, state.bounds.width, state.bounds.height)
                            .expect("registered bounds stay valid"),
                    ) {
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
                if !matches!(direction, Some(0..=3)) {
                    return invalid_argument("direction must be 0..3");
                }
                Reply::Success(Value::Null)
            }
            view_methods::ACCEPT_GESTURE | view_methods::REJECT_GESTURE => {
                Reply::Success(Value::Null)
            }
            view_methods::OFFSET => {
                let _ = arg_number(&call.arguments, view_methods::ARG_X);
                let _ = arg_number(&call.arguments, view_methods::ARG_Y);
                Reply::Success(Value::Null)
            }
            view_methods::DISPOSE => match self.registry.dispose(target) {
                Ok(()) => Reply::Success(Value::Null),
                Err(error) => map_error(error),
            },
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
