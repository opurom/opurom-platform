//! OPUROM platform adapter ABI v0.1 (normative: `opurom-spec/ADAPTER_CONTRACT.md`).
//!
//! Zero dependencies. `Headless` is the v0.1 reference adapter and conformance
//! oracle; R3 adds `Terminal` as a second fully implemented, Verified host adapter.
//! Both are thin host adapters demonstrating the platform boundary, not GPU
//! backends. OS-native / mini-program / web adapters, native widget embedding and
//! Metal/Vulkan/ANGLE GPU presentation remain later rounds (R4+).

use std::fmt;

mod headless;
mod terminal;

pub use headless::Headless;
pub use terminal::Terminal;

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

/// The headless reference adapter: a Verified host (v0.1). It is a host/adapter, not
/// a target emitter, so it is not one of the 14 target rows. R3 adds `TERMINAL_SPEC`
/// as a second Verified host.
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

/// R3 Verified host adapter: terminal truecolor ANSI presentation (R3_ADDENDUM.md
/// section 6). A thin host adapter demonstrating the platform boundary, not a GPU
/// backend. Like `headless` it is a host, not one of the 14 target emitter rows, so
/// it is exposed via `host_platforms()` / `spec_by_name()` / the CLI `--hosts` flag.
pub const TERMINAL_SPEC: PlatformSpec = PlatformSpec {
    name: "terminal",
    emitter: "terminal",
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
    // GPU presentation and native widget embedding are later rounds (R4+), false
    // everywhere; the headless and terminal hosts never claim them either.
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

/// The frozen v0.1 listing: the headless oracle plus the 14 targets (15, one
/// `Verified`). Kept in its R2 shape as a stable regression oracle; R3 Verified
/// hosts are listed by `host_platforms()` and covered by `spec_by_name()`.
pub fn all_platforms() -> Vec<PlatformSpec> {
    let mut all = Vec::with_capacity(TARGET_SPECS.len() + 1);
    all.push(HEADLESS_SPEC.clone());
    all.extend(TARGET_SPECS.iter().cloned());
    all
}

/// Fully implemented, `Verified` host adapters (run on any OS, not target emitters).
/// R3 adds `terminal` alongside the v0.1 `headless` oracle; listed by `--hosts`.
pub fn host_platforms() -> Vec<PlatformSpec> {
    vec![HEADLESS_SPEC.clone(), TERMINAL_SPEC.clone()]
}

/// Query any known platform by name: the 14 targets, the headless oracle and the
/// R3 terminal host.
pub fn spec_by_name(name: &str) -> Option<PlatformSpec> {
    all_platforms()
        .into_iter()
        .chain(std::iter::once(TERMINAL_SPEC.clone()))
        .find(|spec| spec.name == name)
}
