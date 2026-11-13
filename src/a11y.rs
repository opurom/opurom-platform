//! Accessibility tree mapping and macOS screen-reader posting.
//!
//! Parsing and tree-to-AX mapping are pure data operations and need no OS
//! permission. `post_a11y_tree` checks Accessibility trust before calling
//! AppKit/AX APIs; a denial is a concrete `SKIP`, never a fabricated `PASS`.

use crate::AdapterError;

const INVALID_TREE: &str = "invalid accessibility tree JSON";
pub const SKIP_A11Y_ENV: &str = "OPR_SKIP_A11Y";
pub const DEFAULT_A11Y_TREE: &str = include_str!("../tests/fixtures/a11y-tree.json");

fn invalid<T>() -> Result<T, AdapterError> {
    Err(AdapterError::Backend(INVALID_TREE))
}

/// Roles from COMPONENTS_AND_A11Y.md section 2, mapped to AppKit accessibility roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxRole {
    Button,
    StaticText,
    Image,
    CheckBox,
    Slider,
    Group,
    Heading,
}

impl AxRole {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "button" => Self::Button,
            "text" => Self::StaticText,
            "image" => Self::Image,
            "checkbox" => Self::CheckBox,
            "slider" => Self::Slider,
            "heading" => Self::Heading,
            "none" => Self::Group,
            _ => return None,
        })
    }

    /// The AX role name used by AppKit's NSAccessibilityRole constants.
    pub fn ax_name(self) -> &'static str {
        match self {
            Self::Button => "AXButton",
            Self::StaticText => "AXStaticText",
            Self::Image => "AXImage",
            Self::CheckBox => "AXCheckBox",
            Self::Slider => "AXSlider",
            Self::Group => "AXGroup",
            Self::Heading => "AXHeading",
        }
    }
}

/// One node of the renderer's deterministic pre-order a11y tree. Bounds are
/// `[x, y, w, h]` in logical pixels with a top-left origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A11yNode {
    pub id: u32,
    pub role: AxRole,
    pub label: String,
    pub bounds: [i64; 4],
    pub hidden: bool,
    /// A disabled control is still posted to the AX tree but never takes focus.
    pub disabled: bool,
}

/// A visible node mapped to AX coordinates (`frame` has a bottom-left origin).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxElement {
    pub id: u32,
    pub role: AxRole,
    pub label: String,
    pub frame: [i64; 4],
}

/// Honest outcome of a screen-reader posting attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum A11yPost {
    Posted { nodes: usize },
    Skipped { reason: &'static str },
}

impl A11yPost {
    pub fn verdict(self) -> &'static str {
        match self {
            Self::Posted { .. } => "PASS",
            Self::Skipped { .. } => "SKIP",
        }
    }

    pub fn report(self) -> String {
        match self {
            Self::Posted { nodes } => format!("PASS a11y_nodes={nodes}"),
            Self::Skipped { reason } => format!("SKIP {reason}"),
        }
    }
}

fn after_key<'a>(line: &'a str, key: &str) -> Result<&'a str, AdapterError> {
    line.find(key)
        .map(|at| line[at + key.len()..].trim_start())
        .ok_or(AdapterError::Backend(INVALID_TREE))
}

fn parse_u32(rest: &str) -> Result<u32, AdapterError> {
    let len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return invalid();
    }
    rest[..len]
        .parse::<u32>()
        .map_err(|_| AdapterError::Backend(INVALID_TREE))
}

fn hex4(chars: &mut std::str::Chars<'_>) -> Result<u16, AdapterError> {
    let mut value = 0u16;
    for _ in 0..4 {
        let digit = chars
            .next()
            .and_then(|ch| ch.to_digit(16))
            .ok_or(AdapterError::Backend(INVALID_TREE))?;
        value = (value << 4) | digit as u16;
    }
    Ok(value)
}

fn parse_json_string(rest: &str) -> Result<String, AdapterError> {
    if !rest.starts_with('"') {
        return invalid();
    }
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Ok(out),
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let high = hex4(&mut chars)?;
                    let codepoint = if (0xd800..=0xdbff).contains(&high) {
                        if chars.next() != Some('\\') || chars.next() != Some('u') {
                            return invalid();
                        }
                        let low = hex4(&mut chars)?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            return invalid();
                        }
                        0x10000 + (((high as u32 - 0xd800) << 10) | (low as u32 - 0xdc00))
                    } else {
                        high as u32
                    };
                    out.push(char::from_u32(codepoint).ok_or(AdapterError::Backend(INVALID_TREE))?);
                }
                _ => return invalid(),
            },
            _ => out.push(ch),
        }
    }
    invalid()
}

fn parse_string_field(line: &str, key: &str) -> Result<String, AdapterError> {
    parse_json_string(after_key(line, key)?)
}

fn parse_bounds(line: &str) -> Result<[i64; 4], AdapterError> {
    let rest = after_key(line, "\"bounds\":")?;
    let rest = rest
        .strip_prefix('[')
        .ok_or(AdapterError::Backend(INVALID_TREE))?;
    let end = rest.find(']').ok_or(AdapterError::Backend(INVALID_TREE))?;
    let parts: Vec<_> = rest[..end].split(',').collect();
    if parts.len() != 4 {
        return invalid();
    }
    let mut bounds = [0i64; 4];
    for (index, part) in parts.iter().enumerate() {
        bounds[index] = part
            .trim()
            .parse::<i64>()
            .map_err(|_| AdapterError::Backend(INVALID_TREE))?;
    }
    Ok(bounds)
}

fn parse_bool(rest: &str) -> Result<bool, AdapterError> {
    if rest.starts_with("true") {
        Ok(true)
    } else if rest.starts_with("false") {
        Ok(false)
    } else {
        invalid()
    }
}

/// Parse an optional boolean field the renderer may omit; absent means `false`.
fn parse_optional_bool_field(line: &str, key: &str) -> Result<bool, AdapterError> {
    match line.find(key) {
        Some(at) => parse_bool(line[at + key.len()..].trim_start()),
        None => Ok(false),
    }
}

/// Read the renderer's deterministic one-object-per-line accessibility tree JSON.
/// Unknown roles and malformed fields are errors, not silent guesses.
pub fn parse_a11y_json(text: &str) -> Result<Vec<A11yNode>, AdapterError> {
    let mut nodes = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line == "[" || line == "]" {
            continue;
        }
        let line = line.strip_suffix(',').unwrap_or(line);
        if !line.starts_with('{') || !line.ends_with('}') {
            return invalid();
        }
        let id = parse_u32(after_key(line, "\"id\":")?)?;
        let role = AxRole::from_name(&parse_string_field(line, "\"role\":")?)
            .ok_or(AdapterError::Backend(INVALID_TREE))?;
        let label = parse_string_field(line, "\"label\":")?;
        let bounds = parse_bounds(line)?;
        let hidden = parse_bool(after_key(line, "\"hidden\":")?)?;
        let disabled = parse_optional_bool_field(line, "\"disabled\":")?;
        nodes.push(A11yNode {
            id,
            role,
            label,
            bounds,
            hidden,
            disabled,
        });
    }
    Ok(nodes)
}

/// Map the flat pre-order renderer tree to visible AX elements. The tree JSON has no
/// parent ids, so the poster builds a flat group; a nested hierarchy is not supported.
pub fn map_a11y_tree(nodes: &[A11yNode]) -> Vec<AxElement> {
    let root_height = nodes.first().map_or(0, |node| node.bounds[3]);
    nodes
        .iter()
        .filter(|node| !node.hidden)
        .map(|node| {
            let [x, y, width, height] = node.bounds;
            AxElement {
                id: node.id,
                role: node.role,
                label: node.label.clone(),
                frame: [
                    x,
                    root_height.saturating_sub(y).saturating_sub(height),
                    width,
                    height,
                ],
            }
        })
        .collect()
}

/// Honest outcome of the focus-order verification: a value, never a fabricated
/// PASS, in the same shape as `Verification` and `A11yPost`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusOrder {
    /// The focusable nodes in document order.
    Passed { order: Vec<u32> },
    /// The tree cannot support deterministic focus traversal; `reason` says why.
    Failed { reason: String },
}

impl FocusOrder {
    /// The reported verdict string: `PASS` or `FAIL`.
    pub fn verdict(&self) -> &'static str {
        match self {
            Self::Passed { .. } => "PASS",
            Self::Failed { .. } => "FAIL",
        }
    }

    /// A one-line human readable report.
    pub fn report(&self) -> String {
        match self {
            Self::Passed { order } => format!("PASS focus_nodes={}", order.len()),
            Self::Failed { reason } => format!("FAIL {reason}"),
        }
    }
}

/// Roles that participate in keyboard focus: the interactive controls.
fn focusable(node: &A11yNode) -> bool {
    !node.hidden
        && !node.disabled
        && matches!(
            node.role,
            AxRole::Button | AxRole::CheckBox | AxRole::Slider
        )
}

fn describe(node: &A11yNode) -> String {
    if node.label.is_empty() {
        node.role.ax_name().to_string()
    } else {
        format!("{} \"{}\"", node.role.ax_name(), node.label)
    }
}

/// Verify deterministic keyboard traversal and return the focus order: the
/// visible, enabled interactive nodes in document order.
///
/// Three assertions, each failing with a precise message: focusable nodes are
/// listed in document order (the renderer numbers nodes in pre-order), a
/// disabled node is skipped, and every focusable node has non-empty bounds.
pub fn focus_order(nodes: &[A11yNode]) -> FocusOrder {
    let mut order = Vec::new();
    let mut previous: Option<u32> = None;
    for node in nodes {
        if !focusable(node) {
            continue;
        }
        if let Some(previous) = previous {
            if node.id <= previous {
                return FocusOrder::Failed {
                    reason: format!(
                        "focusable node {} does not follow node {previous}: the a11y tree must list focusable nodes in document order",
                        node.id
                    ),
                };
            }
        }
        let [x, y, width, height] = node.bounds;
        if width <= 0 || height <= 0 {
            return FocusOrder::Failed {
                reason: format!(
                    "focusable node {} ({}) has empty bounds [{x},{y},{width},{height}]",
                    node.id,
                    describe(node)
                ),
            };
        }
        previous = Some(node.id);
        order.push(node.id);
    }
    FocusOrder::Passed { order }
}

/// Check OS Accessibility trust and post the mapped tree on macOS.
/// `OPR_SKIP_A11Y=1` is a deterministic harness hook: it makes no OS call.
pub fn post_a11y_tree(nodes: &[A11yNode]) -> Result<A11yPost, AdapterError> {
    if std::env::var_os(SKIP_A11Y_ENV).is_some() {
        return Ok(A11yPost::Skipped {
            reason: "OPR_SKIP_A11Y is set: no Accessibility API was called",
        });
    }
    #[cfg(target_os = "macos")]
    {
        macos::post(nodes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = nodes;
        Ok(A11yPost::Skipped {
            reason: "macOS NSAccessibility and AXUIElement APIs are unavailable on this host",
        })
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{map_a11y_tree, A11yNode, A11yPost, AxElement, AxRole};
    use crate::AdapterError;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAccessibility, NSAccessibilityButtonRole, NSAccessibilityCheckBoxRole,
        NSAccessibilityCreatedNotification, NSAccessibilityElement, NSAccessibilityGroupRole,
        NSAccessibilityHeadingRole, NSAccessibilityImageRole,
        NSAccessibilityLayoutChangedNotification, NSAccessibilityNotificationName,
        NSAccessibilityPostNotification, NSAccessibilityRole, NSAccessibilitySliderRole,
        NSAccessibilityStaticTextRole, NSApplication,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::ffi::{c_uchar, c_void};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        // C `Boolean` is an unsigned char, not Rust's bool.
        fn AXIsProcessTrusted() -> c_uchar;
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: *const c_void);
    }

    fn native_role(role: AxRole) -> &'static NSAccessibilityRole {
        // SAFETY: these are immutable AppKit role constants.
        unsafe {
            match role {
                AxRole::Button => NSAccessibilityButtonRole,
                AxRole::StaticText => NSAccessibilityStaticTextRole,
                AxRole::Image => NSAccessibilityImageRole,
                AxRole::CheckBox => NSAccessibilityCheckBoxRole,
                AxRole::Slider => NSAccessibilitySliderRole,
                AxRole::Group => NSAccessibilityGroupRole,
                AxRole::Heading => NSAccessibilityHeadingRole,
            }
        }
    }

    unsafe fn object_ref<T: objc2::Message>(retained: &Retained<T>) -> &AnyObject {
        &*(Retained::as_ptr(retained).cast::<AnyObject>())
    }

    fn rect(frame: [i64; 4]) -> NSRect {
        NSRect::new(
            NSPoint::new(frame[0] as f64, frame[1] as f64),
            NSSize::new(frame[2] as f64, frame[3] as f64),
        )
    }

    fn element_for(node: &AxElement, root: &AnyObject) -> Retained<NSAccessibilityElement> {
        let element = NSAccessibilityElement::new();
        element.setAccessibilityRole(Some(native_role(node.role)));
        let label = NSString::from_str(&node.label);
        element.setAccessibilityLabel(Some(&label));
        let identifier = NSString::from_str(&format!("opurom-{}", node.id));
        element.setAccessibilityIdentifier(Some(&identifier));
        element.setAccessibilityFrame(rect(node.frame));
        element.setAccessibilityElement(true);
        // SAFETY: `root` is retained by the caller until all postings finish.
        unsafe { element.setAccessibilityParent(Some(root)) };
        element
    }

    pub(super) fn post(nodes: &[A11yNode]) -> Result<A11yPost, AdapterError> {
        let elements = map_a11y_tree(nodes);
        if elements.is_empty() {
            return Ok(A11yPost::Skipped {
                reason: "the a11y tree has no visible nodes",
            });
        }
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Ok(A11yPost::Skipped {
                reason: "AXIsProcessTrusted returned false: grant Accessibility permission to this binary in System Settings",
            });
        }
        let mtm = MainThreadMarker::new().ok_or(AdapterError::Backend(
            "NSAccessibility posting requires the process main thread",
        ))?;
        let application = NSApplication::sharedApplication(mtm);
        application.finishLaunching();
        let pid = i32::try_from(std::process::id())
            .map_err(|_| AdapterError::Backend("the process id exceeds pid_t"))?;
        let ax_app = unsafe { AXUIElementCreateApplication(pid) };
        if ax_app.is_null() {
            return Err(AdapterError::Backend(
                "AXUIElementCreateApplication failed: no AX application element",
            ));
        }
        unsafe { CFRelease(ax_app as *const c_void) };

        let root = NSAccessibilityElement::new();
        root.setAccessibilityRole(Some(native_role(AxRole::Group)));
        let root_label = NSString::from_str("OPUROM");
        root.setAccessibilityLabel(Some(&root_label));
        let root_frame = map_a11y_tree(&nodes[..1])
            .first()
            .map_or([0, 0, 0, 0], |node| node.frame);
        root.setAccessibilityFrame(rect(root_frame));
        root.setAccessibilityElement(true);
        let root_object = unsafe { object_ref(&root) };
        let mut children = Vec::with_capacity(elements.len());
        for node in &elements {
            let child = element_for(node, root_object);
            root.accessibilityAddChildElement(&child);
            children.push(child);
        }

        // SAFETY: AppKit notification names are immutable system constants,
        // and every element below stays retained for the duration of the call.
        unsafe {
            let created: &'static NSAccessibilityNotificationName =
                NSAccessibilityCreatedNotification;
            for child in &children {
                NSAccessibilityPostNotification(object_ref(child), created);
            }
            let layout: &'static NSAccessibilityNotificationName =
                NSAccessibilityLayoutChangedNotification;
            NSAccessibilityPostNotification(root_object, layout);
        }
        Ok(A11yPost::Posted {
            nodes: children.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        focus_order, map_a11y_tree, parse_a11y_json, AxRole, FocusOrder, DEFAULT_A11Y_TREE,
    };

    #[test]
    fn renderer_tree_maps_to_ax_without_os_permission() {
        let nodes = parse_a11y_json(DEFAULT_A11Y_TREE).unwrap();
        assert_eq!(nodes.len(), 11);
        let elements = map_a11y_tree(&nodes);
        assert_eq!(elements.len(), 9);
        assert_eq!(
            elements.iter().map(|e| e.id).collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert_eq!(elements[0].role, AxRole::Group);
        assert_eq!(elements[0].role.ax_name(), "AXGroup");
        assert_eq!(elements[1].role, AxRole::StaticText);
        assert_eq!(elements[1].label, "Hello");
        assert_eq!(elements[1].frame, [4, 220, 60, 16]);
        assert_eq!(elements[2].role, AxRole::Heading);
        assert_eq!(elements[2].role.ax_name(), "AXHeading");
        assert_eq!(elements[2].frame, [4, 187, 72, 31]);
        assert_eq!(elements[3].role.ax_name(), "AXButton");
        assert_eq!(elements[4].role.ax_name(), "AXImage");
        assert_eq!(elements[5].role.ax_name(), "AXCheckBox");
        assert_eq!(elements[6].role.ax_name(), "AXSlider");
        assert!(!elements.iter().any(|e| e.label == "Hidden"));
    }

    #[test]
    fn json_escapes_are_decoded() {
        let text = "[\n  {\"id\":1,\"role\":\"button\",\"label\":\"say \\\"hi\\\"\\n\\\\\\u0041\",\"bounds\":[-1,0,2,3],\"hidden\":false}\n]";
        let nodes = parse_a11y_json(text).unwrap();
        assert_eq!(nodes[0].label, "say \"hi\"\n\\A");
        assert_eq!(nodes[0].bounds, [-1, 0, 2, 3]);
    }

    #[test]
    fn malformed_trees_and_unknown_roles_are_rejected() {
        assert!(parse_a11y_json("[ {\"id\":1} ]").is_err());
        assert!(parse_a11y_json(
            "[\n  {\"id\":1,\"role\":\"toast\",\"label\":\"\",\"bounds\":[0,0,1,1],\"hidden\":false}\n]"
        )
        .is_err());
        assert!(parse_a11y_json(
            "[\n  {\"id\":1,\"role\":\"button\",\"label\":\"\",\"bounds\":[0,0,1],\"hidden\":false}\n]"
        )
        .is_err());
    }

    #[test]
    fn a_disabled_node_is_skipped_but_still_posted() {
        let text = concat!(
            "[\n",
            "  {\"id\":1,\"role\":\"button\",\"label\":\"On\",\"bounds\":[0,0,10,10],\"hidden\":false},\n",
            "  {\"id\":2,\"role\":\"button\",\"label\":\"Off\",\"bounds\":[0,12,10,10],\"hidden\":false,\"disabled\":true},\n",
            "  {\"id\":3,\"role\":\"checkbox\",\"label\":\"\",\"bounds\":[0,24,10,10],\"hidden\":false}\n",
            "]"
        );
        let nodes = parse_a11y_json(text).unwrap();
        assert!(!nodes[0].disabled, "an absent field defaults to false");
        assert!(nodes[1].disabled);
        let outcome = focus_order(&nodes);
        assert_eq!(outcome.verdict(), "PASS");
        assert_eq!(outcome.report(), "PASS focus_nodes=2");
        assert_eq!(
            outcome,
            FocusOrder::Passed { order: vec![1, 3] },
            "the disabled button is skipped, the rest keep document order"
        );
        assert_eq!(
            map_a11y_tree(&nodes).len(),
            3,
            "a disabled node is still posted to the AX tree"
        );
    }

    #[test]
    fn empty_bounds_fail_with_a_precise_message() {
        let nodes = parse_a11y_json(concat!(
            "[\n",
            "  {\"id\":3,\"role\":\"button\",\"label\":\"Go\",\"bounds\":[4,55,0,0],\"hidden\":false}\n",
            "]"
        ))
        .unwrap();
        let outcome = focus_order(&nodes);
        assert_eq!(outcome.verdict(), "FAIL");
        assert_eq!(
            outcome,
            FocusOrder::Failed {
                reason: "focusable node 3 (AXButton \"Go\") has empty bounds [4,55,0,0]"
                    .to_string(),
            }
        );

        let nodes = parse_a11y_json(concat!(
            "[\n",
            "  {\"id\":7,\"role\":\"slider\",\"label\":\"\",\"bounds\":[4,55,-1,8],\"hidden\":false}\n",
            "]"
        ))
        .unwrap();
        assert_eq!(
            focus_order(&nodes),
            FocusOrder::Failed {
                reason: "focusable node 7 (AXSlider) has empty bounds [4,55,-1,8]".to_string(),
            }
        );
    }

    #[test]
    fn out_of_document_order_fails_with_a_precise_message() {
        let text = concat!(
            "[\n",
            "  {\"id\":4,\"role\":\"button\",\"label\":\"Late\",\"bounds\":[0,0,10,10],\"hidden\":false},\n",
            "  {\"id\":2,\"role\":\"button\",\"label\":\"Early\",\"bounds\":[0,12,10,10],\"hidden\":false}\n",
            "]"
        );
        let nodes = parse_a11y_json(text).unwrap();
        assert_eq!(
            focus_order(&nodes),
            FocusOrder::Failed {
                reason: "focusable node 2 does not follow node 4: the a11y tree must list focusable nodes in document order"
                    .to_string(),
            }
        );

        // A duplicated node breaks pre-order the same way.
        let text = concat!(
            "[\n",
            "  {\"id\":5,\"role\":\"button\",\"label\":\"A\",\"bounds\":[0,0,10,10],\"hidden\":false},\n",
            "  {\"id\":5,\"role\":\"button\",\"label\":\"B\",\"bounds\":[0,12,10,10],\"hidden\":false}\n",
            "]"
        );
        let nodes = parse_a11y_json(text).unwrap();
        assert_eq!(
            focus_order(&nodes),
            FocusOrder::Failed {
                reason: "focusable node 5 does not follow node 5: the a11y tree must list focusable nodes in document order"
                    .to_string(),
            }
        );
    }
}
