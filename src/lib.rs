//! OPUROM platform adapter ABI v0.1 (normative: `opurom-spec/ADAPTER_CONTRACT.md`).
//!
//! Zero dependencies. Only `Headless` is implemented and Verified in v0.1; every
//! OS-native / mini-program / web adapter, native widget embedding and GPU
//! presentation are later rounds (R2+).

use std::fmt;

mod headless;

pub use headless::Headless;

/// Logical viewport size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Borrowed, platform-neutral RGBA8 pixel view. Defined here so the renderer never
/// becomes a compile-time dependency of adapters.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

/// Unified input, normalized from touch/mouse/keyboard by each adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Tap { x: f32, y: f32, node_id: u32 },
    Input { node_id: u32, text: String },
    Key { code: u32, pressed: bool },
    Resize { size: Size },
    Lifecycle(Lifecycle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Resumed,
    Paused,
    Destroyed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capabilities {
    pub gpu: bool,
    pub native_embed: bool,
    pub multi_window: bool,
    pub lifecycle: bool,
    pub dpi_scale: f32,
}

/// Adapter errors are values, never panics across the boundary. Three consecutive
/// `Err`s from an adapter mean "degraded": the kernel swaps in `Headless`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterError {
    /// `FrameView::pixels` is not exactly `width * height * 4` bytes.
    InvalidFrame,
    /// A recoverable host/backend failure (e.g. a lost surface).
    Backend(&'static str),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::InvalidFrame => write!(f, "invalid RGBA8 frame buffer length"),
            AdapterError::Backend(why) => write!(f, "backend error: {why}"),
        }
    }
}

impl std::error::Error for AdapterError {}

/// The one platform-specific ABI. The kernel, compiler and renderer do not change
/// when a new platform is added.
pub trait PlatformAdapter {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn viewport(&self) -> Size;
    fn poll_event(&mut self) -> Option<InputEvent>;
    fn present(&mut self, frame: FrameView<'_>) -> Result<(), AdapterError>;
}

/// Verified status of a platform. Never claim `Verified` for a scaffold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Verified,
    Scaffold,
    Planned,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Verified => "Verified",
            Status::Scaffold => "Scaffold",
            Status::Planned => "Planned",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatformSpec {
    pub name: &'static str,
    /// Target emitter from `opurom-spec/IR_SCHEMA.md`; OS hosts share `native`.
    pub emitter: &'static str,
    pub capabilities: Capabilities,
    pub status: Status,
}

/// The headless reference adapter: the only Verified platform in v0.1.
/// It is a host/adapter, not a target emitter, so it is not one of the 14 target rows.
pub const HEADLESS_SPEC: PlatformSpec = PlatformSpec {
    name: "headless",
    emitter: "headless",
    capabilities: Capabilities {
        gpu: false,
        native_embed: false,
        multi_window: false,
        lifecycle: false,
        dpi_scale: 1.0,
    },
    status: Status::Verified,
};

const fn caps(multi_window: bool, lifecycle: bool, dpi_scale: f32) -> Capabilities {
    // GPU presentation and native widget embedding are R2+ work, false everywhere in v0.1.
    Capabilities {
        gpu: false,
        native_embed: false,
        multi_window,
        lifecycle,
        dpi_scale,
    }
}

// `Scaffold` = a known emitter with a registry row, no runtime adapter yet.
// `Planned` = no dedicated target emitter yet. Neither is implemented.
const TARGET_SPECS: [PlatformSpec; 14] = [
    PlatformSpec {
        name: "android",
        emitter: "native",
        capabilities: caps(false, true, 2.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "ios",
        emitter: "native",
        capabilities: caps(false, true, 2.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "harmony-mobile",
        emitter: "native",
        capabilities: caps(false, true, 2.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "harmony-pc",
        emitter: "native",
        capabilities: caps(true, false, 1.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "windows",
        emitter: "native",
        capabilities: caps(true, false, 1.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "macos",
        emitter: "native",
        capabilities: caps(true, false, 1.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "linux",
        emitter: "native",
        capabilities: caps(true, false, 1.0),
        status: Status::Planned,
    },
    PlatformSpec {
        name: "wechat",
        emitter: "wechat",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "alipay",
        emitter: "alipay",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "qq",
        emitter: "qq",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "douyin",
        emitter: "douyin",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "baidu",
        emitter: "baidu",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "kuaishou",
        emitter: "kuaishou",
        capabilities: caps(false, true, 2.0),
        status: Status::Scaffold,
    },
    PlatformSpec {
        name: "web",
        emitter: "web",
        capabilities: caps(false, true, 1.0),
        status: Status::Scaffold,
    },
];

/// The 14 target platforms named by ADAPTER_CONTRACT.md, one unique row each.
/// None is `Verified` in v0.1.
pub fn registry() -> Vec<PlatformSpec> {
    TARGET_SPECS.to_vec()
}

/// Every platform v0.1 knows about: the headless oracle plus the 14 targets.
pub fn all_platforms() -> Vec<PlatformSpec> {
    let mut all = Vec::with_capacity(TARGET_SPECS.len() + 1);
    all.push(HEADLESS_SPEC.clone());
    all.extend(TARGET_SPECS.iter().cloned());
    all
}

/// Query the registry by name, including the headless reference adapter.
pub fn spec_by_name(name: &str) -> Option<PlatformSpec> {
    all_platforms().into_iter().find(|spec| spec.name == name)
}
