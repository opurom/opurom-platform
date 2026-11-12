//! OPUROM platform adapter ABI v0.1 (normative: `opurom-spec/ADAPTER_CONTRACT.md`).
//!
//! `Headless` is the v0.1 reference adapter and conformance oracle; R3 adds
//! `Terminal`. R5 adds the macOS-only `MacHost` with a real AppKit/CAMetalLayer
//! host descriptor, a headlessly verifiable PNG path and `WindowSet`. The
//! AppKit/CoreGraphics dependencies are target-scoped, so Linux CI builds the
//! ABI and the cross-platform hosts without AppKit. OS-native adapters for
//! Windows/Linux/Harmony, native widget trees and GPU presentation remain R6+.

use std::fmt;

mod headless;
mod pixel;
mod png;
mod ppm;
mod terminal;
mod verify;
mod window_set;

#[cfg(target_os = "macos")]
mod appkit;
#[cfg(target_os = "macos")]
mod core_graphics_bridge;

pub use headless::Headless;
pub use pixel::{compare_rgba_to_ppm, PixelComparison, CHANNEL_TOLERANCE, MISMATCH_BUDGET_PERCENT};
pub use png::frame_to_png;
pub use ppm::{read_ppm, PpmImage};
pub use terminal::Terminal;
pub use verify::{compare_capture, render_reference_ppm, software_renderer_path, Verification};
pub use window_set::WindowSet;

#[cfg(target_os = "macos")]
pub use appkit::{
    capture_window, draw_and_capture, window_availability, write_png, CapturedWindow, MacHost,
    MacHostDescriptor, MacWindow, WindowUnavailable,
};

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

/// Non-macOS fallback for the macOS-only PNG path. It is an error value, never
/// a fabricated CoreGraphics result or a fake success.
pub const APPKIT_UNAVAILABLE: &str = "AppKitUnavailable: frame_to_png requires macOS";

/// Environment override that forces the on-screen verification to SKIP. CI
/// sets `OPR_SKIP_WINDOW=1` so a headless runner never claims a PASS.
pub const SKIP_WINDOW_ENV: &str = "OPR_SKIP_WINDOW";

/// Environment override that points the verification at a pre-rendered
/// software-renderer PPM instead of running the renderer itself.
pub const REFERENCE_PPM_ENV: &str = "OPR_REFERENCE_PPM";

/// Environment override for the software renderer binary used to produce the
/// reference PPM.
pub const RENDERER_BIN_ENV: &str = "OPUROM_RENDER_BIN";

/// R5 Verified host on macOS. `native_embed` marks the real `NSView` /
/// `CAMetalLayer` native host surface, not an embedded widget tree; `gpu` stays
/// false because `present` writes a PNG instead of drawing to a Metal drawable.
/// `dpi_scale` is a documented 1.0 fallback; `appkit_capabilities()` replaces it
/// with `NSScreen.backingScaleFactor` when that value is available.
pub const APPKIT_SPEC: PlatformSpec = PlatformSpec {
    name: "appkit",
    emitter: "appkit",
    capabilities: Capabilities {
        gpu: false,
        native_embed: true,
        multi_window: true,
        lifecycle: false,
        dpi_scale: 1.0,
    },
    status: Status::Verified,
};

const fn caps(multi_window: bool, lifecycle: bool, dpi_scale: f32) -> Capabilities {
    // Target rows and the headless/terminal hosts claim neither GPU presentation
    // nor a native host surface; the AppKit host sets its own capabilities above.
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

/// AppKit capabilities with the real `NSScreen.backingScaleFactor` when the
/// main-thread screen query is available; the documented fallback is 1.0.
#[cfg(target_os = "macos")]
pub fn appkit_capabilities() -> Capabilities {
    let mut capabilities = APPKIT_SPEC.capabilities;
    capabilities.dpi_scale = appkit::screen_dpi_scale();
    capabilities
}

/// The R5 AppKit host row with its measured/fallback DPI scale.
#[cfg(target_os = "macos")]
pub fn appkit_spec() -> PlatformSpec {
    let mut spec = APPKIT_SPEC.clone();
    spec.capabilities = appkit_capabilities();
    spec
}

/// Fully implemented, `Verified` host adapters (not target emitters). R3 adds
/// `terminal`; R5 adds `appkit` on macOS only. Listed by `--hosts`.
pub fn host_platforms() -> Vec<PlatformSpec> {
    #[allow(unused_mut)]
    let mut hosts = vec![HEADLESS_SPEC.clone(), TERMINAL_SPEC.clone()];
    #[cfg(target_os = "macos")]
    hosts.push(appkit_spec());
    hosts
}

/// Query any known platform by name: the 14 targets, the headless oracle, the
/// R3 terminal host and (on macOS) the R5 AppKit host.
pub fn spec_by_name(name: &str) -> Option<PlatformSpec> {
    let mut known = all_platforms();
    known.push(TERMINAL_SPEC.clone());
    #[cfg(target_os = "macos")]
    known.push(appkit_spec());
    known.into_iter().find(|spec| spec.name == name)
}
