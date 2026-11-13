//! OPUROM platform adapter ABI v0.1 (normative: `opurom-spec/ADAPTER_CONTRACT.md`).
//!
//! `Headless` is the v0.1 reference adapter and conformance oracle; the terminal host adds
//! `Terminal`. The macOS host adds the `MacHost` with a real AppKit/CAMetalLayer
//! host descriptor, a headlessly verifiable PNG path and `WindowSet`. The native surfaces
//! add compile-verified Win32/X11 window hosts, a runtime-loaded Wayland client
//! adapter with a host-independent wire encoder, and macOS accessibility posting.
//! The GPU presentation path adds a runtime-loaded Metal drawable presentation for the live
//! AppKit window. The AppKit/CoreGraphics dependencies are target-scoped, so
//! Linux CI builds the ABI and the cross-platform hosts without AppKit.

use std::fmt;

mod a11y;
pub mod gpu_animation;
pub mod gpu_presentation;
mod headless;
mod pixel;
mod png;
mod ppm;
mod terminal;
mod verify;
mod window_set;
// Pure X11 pixel maths: used by the Linux adapter and by `cargo test` on every
// host; excluded from non-Linux production builds to avoid dead-code warnings.
#[cfg(any(target_os = "linux", test))]
mod x11_pixels;
// Pure Wayland wire encoding: the handshake, object IDs, the opcode/size
// header and the wl_shm buffer contract. Verified by `cargo test` on every
// host, including macOS workstations without a Wayland compositor.
#[cfg(any(target_os = "linux", test))]
pub mod wayland_wire;

#[cfg(target_os = "macos")]
mod appkit;
#[cfg(target_os = "macos")]
mod core_graphics_bridge;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "linux")]
mod x11;

pub use a11y::{
    map_a11y_tree, parse_a11y_json, post_a11y_tree, A11yNode, A11yPost, AxElement, AxRole,
    DEFAULT_A11Y_TREE, SKIP_A11Y_ENV,
};
pub use gpu_animation::{verify_gpu_animation_loop, ANIMATION_FLAG, ANIMATION_TICKS};
pub use gpu_presentation::{
    classify, gpu_library_path, present_animation_frame, present_display_list, workspace_root,
    FrameTimings, PresentationOutcome, GPU_DYLIB_ENV, PRESENT_ERROR, PRESENT_INVALID_ARGUMENT,
    PRESENT_METAL_UNAVAILABLE, PRESENT_NO_DRAWABLE, PRESENT_OK, PRESENT_UNSUPPORTED,
};
pub use headless::Headless;
pub use pixel::{compare_rgba_to_ppm, PixelComparison, CHANNEL_TOLERANCE, MISMATCH_BUDGET_PERCENT};
pub use png::frame_to_png;
pub use ppm::{read_ppm, PpmImage};
pub use terminal::Terminal;
pub use verify::{
    compare_capture, render_animation_frame, render_reference_ppm, render_reference_with_commands,
    software_renderer_path, Verification,
};
pub use window_set::WindowSet;

#[cfg(target_os = "macos")]
pub use appkit::{
    capture_window, draw_and_capture, window_availability, write_png, CapturedWindow, MacHost,
    MacHostDescriptor, MacWindow, WindowUnavailable,
};
#[cfg(target_os = "linux")]
pub use wayland::{Handshake, WaylandHost};
#[cfg(target_os = "windows")]
pub use win32::Win32Host;
#[cfg(target_os = "linux")]
pub use x11::X11Host;

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
/// a target emitter, so it is not one of the 14 target rows. The terminal host adds
/// `TERMINAL_SPEC` as a second Verified host.
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

/// The terminal Verified host adapter: truecolor ANSI presentation
/// (COMPONENTS_AND_A11Y.md section 6). A thin host adapter demonstrating the platform boundary, not a GPU
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

/// The environment variable that names the Wayland compositor socket.
pub const WAYLAND_DISPLAY_ENV: &str = "WAYLAND_DISPLAY";

/// The concrete reason a Wayland check cannot run, or `None` when a display is
/// named. A missing or empty display is a SKIP, never a fake PASS.
pub fn wayland_skip_reason(display: Option<&str>) -> Option<&'static str> {
    match display {
        None => Some("WAYLAND_DISPLAY is not set"),
        Some(value) if value.trim().is_empty() => Some("WAYLAND_DISPLAY is empty"),
        Some(_) => None,
    }
}

/// Environment override that points the verification at a pre-rendered
/// software-renderer PPM instead of running the renderer itself.
pub const REFERENCE_PPM_ENV: &str = "OPR_REFERENCE_PPM";

/// Environment override for the software renderer binary used to produce the
/// reference PPM.
pub const RENDERER_BIN_ENV: &str = "OPUROM_RENDER_BIN";

/// The Verified host on macOS. `native_embed` marks the real `NSView` /
/// `CAMetalLayer` native host surface, not an embedded widget tree; `gpu` stays
/// false because `PlatformAdapter::present` writes a PNG. The separate
/// `--verify-gpu-presentation` CLI uses a real Metal drawable; it does not
/// change the adapter ABI or this frozen capability row.
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

/// The Wayland client host row (TRANSPORT_AND_LANGUAGE_ADDITIONS.md section 4).
/// The wire encoder, the runtime `dlopen`/`dlsym` loader and the setup
/// handshake are implemented and unit-tested; a live surface needs a running
/// compositor, so the row stays `Planned` until one is available.
pub const WAYLAND_SPEC: PlatformSpec = PlatformSpec {
    name: "wayland",
    emitter: "native",
    capabilities: caps(false, false, 1.0),
    status: Status::Planned,
};

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
/// `Verified`). Kept in its original shape as a stable regression oracle; the terminal
/// and AppKit Verified hosts are listed by `host_platforms()` and covered by `spec_by_name()`.
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

/// The AppKit host row with its measured/fallback DPI scale.
#[cfg(target_os = "macos")]
pub fn appkit_spec() -> PlatformSpec {
    let mut spec = APPKIT_SPEC.clone();
    spec.capabilities = appkit_capabilities();
    spec
}

/// Fully implemented, `Verified` host adapters (not target emitters). The terminal host
/// is always listed; `appkit` is added on macOS only. Listed by `--hosts`.
pub fn host_platforms() -> Vec<PlatformSpec> {
    #[allow(unused_mut)]
    let mut hosts = vec![HEADLESS_SPEC.clone(), TERMINAL_SPEC.clone()];
    #[cfg(target_os = "macos")]
    hosts.push(appkit_spec());
    hosts
}

/// A single boolean capability flag from `Capabilities`, usable as a registry
/// query key. `dpi_scale` is a numeric measurement rather than a boolean flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Gpu,
    NativeEmbed,
    MultiWindow,
    Lifecycle,
}

impl PlatformSpec {
    /// Whether this platform advertises `capability`.
    pub fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::Gpu => self.capabilities.gpu,
            Capability::NativeEmbed => self.capabilities.native_embed,
            Capability::MultiWindow => self.capabilities.multi_window,
            Capability::Lifecycle => self.capabilities.lifecycle,
        }
    }
}

/// Every platform the crate can resolve by name: the frozen v0.1 listing, the
/// terminal host, the Wayland host row and (on macOS) the AppKit host.
pub fn known_platforms() -> Vec<PlatformSpec> {
    let mut known = all_platforms();
    known.push(TERMINAL_SPEC.clone());
    known.push(WAYLAND_SPEC.clone());
    #[cfg(target_os = "macos")]
    known.push(appkit_spec());
    known
}

/// Known platforms that advertise `capability`, in `known_platforms()` order.
/// Hosts select feature-capable platforms without depending on the frozen row
/// layout.
pub fn platforms_with(capability: Capability) -> Vec<PlatformSpec> {
    known_platforms()
        .into_iter()
        .filter(|spec| spec.supports(capability))
        .collect()
}

/// Query any known platform by name: the 14 targets, the headless oracle, the
/// terminal host, the Wayland host row and (on macOS) the AppKit host.
pub fn spec_by_name(name: &str) -> Option<PlatformSpec> {
    known_platforms().into_iter().find(|spec| spec.name == name)
}
