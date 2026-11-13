# opurom-platform - Platform Adapter ABI v0.1 + GPU presentation

Independent Rust crate implementing the normative
[ADAPTER_CONTRACT.md](../opurom-spec/ADAPTER_CONTRACT.md). The ABI, registry,
`Headless`, `Terminal` and `WindowSet` remain pure Rust. PNG encoding uses the
cached `png` crate; macOS AppKit/CoreGraphics dependencies are target-scoped,
so Linux CI builds the crate without AppKit. The macOS host adds a real `NSApplication`
/ `NSWindow` / `NSView` host descriptor with a `CAMetalLayer`-compatible surface,
a headlessly verifiable RGBA8-to-PNG path and deterministic multi-window state.
The live-window path turns `open_window()` into a **tested entry point**: it opens a real AppKit
window, captures the window's own pixels with `cacheDisplay(in:to:)` and
compares them against the software renderer. The native surfaces add compile-verified Win32
and X11 window hosts plus permission-aware macOS accessibility posting. The GPU
presentation path presents the GPU compute buffer into that live window's `CAMetalLayer`
drawable and verifies the on-screen pixels.

## Status

| name | emitter | status | gpu | native_embed | multi_window |
| --- | --- | --- | --- | --- | --- |
| headless | headless | **Verified** | false | false | false |
| terminal | terminal | **Verified** | false | false | false |
| appkit | appkit | **Verified** (macOS only) | false | true | true |
| wayland | native | Planned | false | false | false |
| android | native | Planned | false | false | false |
| ios | native | Planned | false | false | false |
| harmony-mobile | native | Planned | false | false | false |
| harmony-pc | native | Planned | false | false | true |
| windows | native | Planned | false | false | true |
| macos | native | Planned | false | false | true |
| linux | native | Planned | false | false | true |
| wechat | wechat | Scaffold | false | false | false |
| alipay | alipay | Scaffold | false | false | false |
| qq | qq | Scaffold | false | false | false |
| douyin | douyin | Scaffold | false | false | false |
| baidu | baidu | Scaffold | false | false | false |
| kuaishou | kuaishou | Scaffold | false | false | false |
| web | web | Scaffold | false | false | false |

`registry()` still returns the **14 target rows** named by ADAPTER_CONTRACT.md,
each unique and each `Scaffold` or `Planned`; the `macos` target row remains
`Planned`. `all_platforms()` is still the frozen v0.1 listing (`headless` + the
14 targets = 15, one `Verified`), so the regression oracle is unchanged. The
`Verified` rows above are **host adapters**, not target emitters. The
`Win32Host` and `X11Host` are compile-only host adapters behind their OS
`cfg`s; they are deliberately absent from `host_platforms()` and the `windows`
/ `linux` target rows stay `Planned` because no window was executed.
`host_platforms()` / `--hosts` list `headless`, `terminal` and, on macOS only,
`appkit`; other targets list only the two cross-platform hosts. `appkit` is
also reachable through `spec_by_name("appkit")` on macOS.

`known_platforms()` enumerates every platform the crate resolves by name: the
frozen `all_platforms()` listing plus the `terminal` host, the `wayland` host
row and, on macOS, the `appkit` host. `platforms_with(capability)` returns the
subset advertising one boolean capability flag, in `known_platforms()` order.
The closed `Capability` enum covers the four boolean flags (`Gpu`,
`NativeEmbed`, `MultiWindow`, `Lifecycle`); `dpi_scale` is a numeric
measurement. `PlatformSpec::supports(capability)` reports a single spec's flag.

`APPKIT_SPEC.native_embed: true` reports the real `NSView` / `CAMetalLayer`
native host surface. It does **not** claim an embedded AppKit control/widget
tree: the AppKit host embeds no native widgets. `gpu` stays `false` for the adapter because
`PlatformAdapter::present` writes a PNG. The GPU presentation path adds a **separate, real** Metal
drawable path through `--verify-gpu-presentation`; it does not change the frozen
capability row. `appkit_capabilities()` reports
`NSScreen.backingScaleFactor` on the macOS main thread when a screen is
available; the documented fallback is `1.0` on a non-main thread, with no
screen, for an invalid scale, and in the cross-platform `APPKIT_SPEC` constant.
The remaining target-row capability flags and `dpi_scale` values are nominal
logical defaults, not measurements. No fps, device-matrix or benchmark claims
are made.

The native surfaces add `src/win32.rs`, `src/x11.rs` and `src/wayland.rs` as OS
adapters. `windows-sys = "=0.52.0"` is a Windows-only dependency, so macOS and
Linux CI never need it. X11 resolves `libX11.so.6`/`libX11.so` and its Xlib
symbols at runtime with plain libc `dlopen`/`dlsym`; there is **no `-lX11`
link requirement** and no X11 headers or development libraries are needed to
build. A missing runtime library returns a documented `X11 unavailable` error.
Wayland resolves `libwayland-client.so.0`/`libwayland-client.so` and its
libwayland-client symbols the same way, so there is **no `-lwayland-client`
link requirement** either; a missing runtime library returns a documented
`Wayland unavailable` error. The Wayland wire encoding - the setup handshake,
object ID allocation, the opcode/size header and the `wl_shm` buffer contract -
is pure Rust in `src/wayland_wire.rs` and is unit-tested on every host.
Both cross-target `cargo check --all-targets` commands pass. No Windows binary
is linked (no mingw linker), and no X11 window is opened (no X server).
The live-window and GPU presentation paths exercise a live GUI
session, but only on macOS and only for the single
AppKit host below; the Linux CI runner has no window server and always reports
SKIP. macOS a11y posting requires OS Accessibility permission. The GPU
bridge also uses runtime `dlopen`/`dlsym` (no `-framework Metal` link in this
binary), matching the X11 fix; all AppKit/Metal code is `cfg`-gated.

## ABI summary

| Item | Contract |
| --- | --- |
| `PlatformAdapter` | `name`, `capabilities`, `viewport`, `poll_event`, `present` - the only platform-specific trait; kernel/compiler/renderer never change for a new host |
| `FrameView<'a>` | borrowed RGBA8 view `{ width: u32, height: u32, pixels: &'a [u8] }`, defined here so the renderer is not a compile-time dependency of adapters |
| `Size` | logical viewport `{ width: u32, height: u32 }` |
| `InputEvent` | `Tap`, `Input`, `Key`, `Resize`, `Lifecycle`; logical pixels, top-left origin, `node_id: u32::MAX` when hit-testing is unavailable |
| `Capabilities` | `gpu`, `native_embed`, `multi_window`, `lifecycle`, `dpi_scale` |
| `AdapterError` | `InvalidFrame` (bad RGBA8 length) and `Backend(&'static str)`; always a value, never a panic across the boundary |
| `Status` | `Verified`, `Scaffold`, `Planned` |
| `registry()` / `all_platforms()` / `spec_by_name()` | frozen target rows and host lookups |
| `host_platforms()` | implemented `Verified` hosts; 3 rows on macOS, 2 elsewhere |
| `known_platforms()` / `platforms_with()` / `supports()` | complete platform enumeration and queries over the four boolean `Capability` flags |
| `WindowSet` | 2..4 independent viewports with isolated sizes, event FIFOs and frame buffers |
| `frame_to_png(FrameView, path)` | real PNG output through a `CGImage` on macOS; documented `AppKitUnavailable` error elsewhere |
| `open_window()` (macOS) | real `NSWindow`/`NSView`; draw, `cacheDisplay(in:to:)` capture, real PNG |
| `compare_rgba_to_ppm` | captured window pixels vs the software renderer PPM, with a documented tolerance |
| `wayland_wire` | pure-Rust Wayland wire encoding: setup handshake, object IDs, opcode/size header, string arguments and the `wl_shm` stride/format contract; unit-tested on every host |
| `WaylandHost::connect()` (Linux) | runtime `dlopen`/`dlsym` of `libwayland-client.so.0`; no link-time Wayland dependency; a missing runtime library is a documented `Wayland unavailable` error |

### Headless reference adapter

`Headless::new(w, h)` starts with a viewport and an empty FIFO event queue.

- `push_event` queues an `InputEvent`; `poll_event` drains it FIFO.
- `resize(size)` updates the viewport and enqueues `InputEvent::Resize`.
- `present(frame)` copies the RGBA bytes into an internal buffer, exposes it via
  `frame()`, and stores a deterministic 16-char hex digest available via `digest()`.
- Wrong buffer length (anything other than `width * height * 4`, including an
  overflowing product) returns `AdapterError::InvalidFrame` and leaves state untouched.

The digest is an inline **FNV-1a 64-bit** checksum: deterministic across runs and
hosts so golden-pixel tests can assert byte-identical output. It is a checksum, not
a cryptographic hash - the spec's "sha256-like" only requires stability. Upgrade to a
real digest if golden hashes ever need collision resistance.

### Terminal host adapter (Verified)

`Terminal` implements the same `PlatformAdapter` trait with deterministic
truecolor ANSI half-block output. It is a thin host adapter, not an OS window or
GPU backend. `present(FrameView)` emits one `U+2580` per column and two pixel
rows per character row; `last_output()` / `render_text()` expose the exact ANSI
bytes. `push_key`, `push_input` and `push_event` queue normalized input;
`poll_event()` drains FIFO. Its capabilities are all false with `dpi_scale: 1.0`.

### AppKit host adapter (Verified on macOS)

`src/appkit.rs` uses `objc2-app-kit` 0.3.2 and `objc2-quartz-core` 0.3.2 to
describe a real AppKit host. `MacHost::describe()` reads the Objective-C class
names from the bindings and reports the selected `NSWindowStyleMask`:

```text
NSApplication | NSWindow | NSView | CAMetalLayer | Titled|Closable|Miniaturizable|Resizable (15)
```

`MacHost::open_window()` is the **live-window entry point**. It requires a
GUI-capable macOS main thread, calls `NSApplication.sharedApplication`, creates
an `NSWindow`, an `NSView` and a real `CAMetalLayer`, attaches the layer to the
view and calls `makeKeyAndOrderFront`. `MacHost::describe()`/the PNG path stay
headlessly verifiable; `open_window()` additionally needs a real window server.

`MacHost::new(w, h, path)` keeps a `Headless`-backed viewport, event FIFO and
frame buffer. `present(FrameView)` calls `frame_to_png`, then stores the frame
and digest; an invalid frame leaves both state and any previous PNG untouched.
`capabilities()` uses the real/fallback DPI scale described above.

### Live window and on-screen pixel verification

`open_window()` is a **real, tested** entry point on macOS. The pipeline is:

1. `open_window()` creates the `NSApplication`, `NSWindow` and `NSView` on the
   process main thread. A host without a window server gets an
   `AdapterError::Backend` with a concrete reason, never a fabricated window.
2. The frame under test is drawn into the view's `CALayer` as a real `CGImage`
   (built through `CGImageCreate` in `src/core_graphics_bridge.rs`).
3. The AppKit event loop runs for a fixed 0.25 s via
   `NSRunLoop.runUntilDate(_:)` so the compositor picks the frame up. The call
   returns after its deadline, so the verification **never hangs**.
4. `cacheDisplay(in:to:)` captures the window's own pixels into an
   `NSBitmapImageRep`. The whole window is captured through its theme frame, so
   the PNG includes the real system title bar.
5. The capture is written as a real PNG (`src/png.rs`, `png` crate).
6. The **content region** (title bar excluded - it is AppKit chrome, not the
   rendered frame) is compared against the software renderer's PPM for the same
   IR.

The comparison uses two documented constants, both in `src/pixel.rs`:

| Constant | Value | Why |
| --- | --- | --- |
| `CHANNEL_TOLERANCE` | `32` per channel (12.5 % of 255) | The capture passes through the display colour profile. On a Display-P3 screen a pure accent block (79, 70, 229) measured (71, 58, 201), a per-channel shift of up to 28. |
| `MISMATCH_BUDGET_PERCENT` | `2.0` % of the compared region | Colour-profile rounding affects only a small part of the frame; the measured run had 0 mismatching pixels. |

A pixel counts as mismatching when **any** of its R, G or B channels differs by
more than `CHANNEL_TOLERANCE`. Alpha is ignored. The check passes when the
mismatch percentage stays within `MISMATCH_BUDGET_PERCENT`. Both constants are
asserted by a test, so the tolerance is part of the contract and not a magic
number.

Run it:

```sh
cargo run --quiet --offline -- --verify-window --out /tmp/opurom-window.png
```

The verdict is one line on stdout:

```text
PASS window=200x150 mismatched_pixels=0 compared_pixels=30000 mismatch=0.0000% max_channel_delta=25 tolerance=32 png=/tmp/opurom-window.png
```

`PASS` means the live window's pixels match the software renderer within the
tolerance above. `FAIL` means they do not; it reports the measured mismatch
count and percentage and exits 1.

**SKIP.** When the host cannot show a window - no Aqua
session, no main screen, no window server, or not the main thread - the check
prints `SKIP <concrete reason>` and exits **0**. The reasons are checked with
`CGSessionCopyCurrentDictionary()` and `NSScreen.mainScreen`. The environment
override `OPR_SKIP_WINDOW=1` forces a SKIP on any host, which is what CI uses:

```sh
OPR_SKIP_WINDOW=1 cargo run --quiet --offline -- --verify-window
# SKIP OPR_SKIP_WINDOW is set
```

Two further overrides exist for harnesses: `OPUROM_RENDER_BIN` points at the
software renderer binary (default `../opurom-render/target/debug/opurom-render`)
and `OPR_REFERENCE_PPM` supplies a pre-rendered reference PPM instead of
running the renderer.

The measured live-window result on the reference host (macOS arm64, 200x150 frame) is
**200x182 captured pixels** (32 px of system title bar above the 200x150
content), **0 mismatching pixels out of 30 000 compared**, and a maximum
per-channel delta of **25**.

The live window is macOS-only; on other platforms the check reports SKIP with
that reason. `WindowSet` is deterministic viewport state, not a set of live OS
windows, and the window is created on the main screen only. The capture uses
the display profile, so the tolerance absorbs the colour shift rather than
correcting it. `native_embed: true` reports the real `NSView`/`CAMetalLayer`
host surface; no AppKit controls are embedded. The live-window path draws a CPU
`CGImage` into the layer (`gpu` stays `false`); the separate real drawable
path is described below.

### Real GPU presentation and on-screen verification

`--verify-gpu-presentation` is the project's first real GPU presentation.
It runs on the process main thread (AppKit window creation) and uses the same
live `NSWindow`/`NSView`/`CAMetalLayer` surface as the live-window path:

1. `--verify-gpu-presentation` resolves the sibling renderer, IR and GPU dylib
   from this crate's manifest, so the command works from the umbrella root. The
   software renderer emits both the reference PPM and the exact display-list
   JSON for the same frame.
2. `src/gpu_presentation.rs` loads
   `../opurom-gpu/target/debug/libopurom_gpu.dylib` at runtime with
   `dlopen`/`dlsym` and calls its `opr_present_display_list` C ABI. This binary
   has no `objc2-metal` dependency and no `-framework Metal` link requirement;
   Linux CI builds the pure-Rust status/path logic only.
3. The GPU crate runs the compute passes, retains the final `MTLBuffer`,
   obtains `nextDrawable()` from the live `CAMetalLayer`, blits the buffer into
   the drawable texture with a real `MTLBlitCommandEncoder`, calls
   `presentDrawable`, commits and waits for completion. No CPU `CGImage` is
   substituted for this path. The presented drawable's own IOSurface is exposed
   as layer contents (zero-copy) because Core Animation's offscreen
   `cacheDisplay(in:to:)` cannot see window-server drawable content directly.
4. `cacheDisplay(in:to:)` captures the whole window, the PNG is
   written, and the **content region** (title bar excluded) is compared with the
   software renderer using `CHANNEL_TOLERANCE = 32` and
   `MISMATCH_BUDGET_PERCENT = 2.0`.

Run it from the umbrella root (or from this crate):

```sh
./opurom-platform/target/debug/opurom-platform \
  --verify-gpu-presentation --out /tmp/opurom-gpu-presentation.png
# PASS backend=metal-blit captured=200x182 content=200x150 title_bar=32 mismatched_pixels=0 compared_pixels=30000 mismatch=0.0000% max_channel_delta=25 tolerance=32 budget=2.0% png=/tmp/opurom-gpu-presentation.png
```

`PASS` prints the measured mismatch count; `FAIL` prints them and exits 1.
`SKIP <concrete reason>` exits 0 when there is no Aqua session, no main screen,
not the main thread, no software renderer, no GPU dylib, no Metal device, no
drawable, or no IOSurface-backed presented texture. `OPR_SKIP_WINDOW=1` forces
the documented `SKIP OPR_SKIP_WINDOW is set`; `OPUROM_GPU_DYLIB` overrides the
dylib path; `OPUROM_RENDER_BIN` and `OPR_REFERENCE_PPM` remain available. A
headless Linux runner prints `SKIP GPU presentation is macOS/Metal only`.

Measured on the reference host (macOS arm64, Apple M6, 200x150 frame):
**200x182 PNG** (32 px title bar + 200x150 content), **0 mismatching pixels
of 30 000** (0.0000 %), max channel delta 25, **PASS**. The GPU presentation
integration test decodes that PNG independently and repeats the tolerance
comparison.

The path is macOS/Metal only: one window, one presented frame, no
Vulkan/ANGLE/D3D, no GPU text shaping and no measured fps claim.
`APPKIT_SPEC.gpu` stays `false` because the adapter's
`present` method is unchanged; the real drawable path is this separate CLI.
`cacheDisplay` sees the exact presented IOSurface, not a CPU re-render of the
software frame.

### Animated multi-frame GPU loop

`--verify-gpu-animation-loop` is a **separate** CLI flag. The single-frame
`--verify-gpu-presentation` output above is unchanged.

1. The software renderer emits the reference PPM and the exact display-list
   JSON at each of four distinct deterministic animation ticks
   (`0, 96, 192, 4000` ms) from
   `../opurom-render/tests/fixtures/animation-loop.opuir.json`, which uses the
   `ease` and `spring` entry-animation curves. `--ir` overrides the IR.
2. For each tick, `src/gpu_animation.rs` calls the GPU dylib's
   `opr_present_display_list_timed` C ABI. The Metal compute raster and the
   drawable blit/present are measured with `std::time::Instant` and reported as
   `dispatch_ms` and `present_ms` in that frame's line. Every frame rebinds
   the layer contents to the IOSurface of the drawable that was just
   presented, so Core Animation's offscreen `cacheDisplay` cannot keep showing
   the first frame's recycled surface.
3. The live window is captured **after each present** into one PNG per frame,
   and the content region is compared with the software renderer's frame at
   that same tick using `CHANNEL_TOLERANCE = 32` and
   `MISMATCH_BUDGET_PERCENT = 2.0`.
4. PASS additionally requires every captured frame to differ from the
   neighbouring captured frame and from the software references of the other
   ticks by more than the mismatch budget. Those two measured minima are
   `min_interframe_delta` and `min_cross_tick_delta`. A frozen layer surface
   therefore fails the check rather than producing a false PASS.

Run it from the umbrella root (or from this crate):

```sh
./opurom-platform/target/debug/opurom-platform \
  --verify-gpu-animation-loop --out /tmp/opurom-gpu-animation-loop.png
```

Measured on the reference host (macOS arm64, Apple M6, 200x150 content):
four frames, **120 000 compared pixels, 0 mismatching pixels**, max channel
delta 23, `min_interframe_delta=4548`, `min_cross_tick_delta=4548`, **PASS**.
Per-frame dispatch and present times are printed on each `frame=` line; they
vary per run and are reproduced verbatim in the verification report. The
per-frame numbers are commit-to-completion wall times, not GPU timestamps.

`SKIP <concrete reason>` exits 0 when there is no Aqua session, no main screen,
not the main thread, no software renderer, no animated IR, no GPU dylib, no
Metal device, no drawable, or no IOSurface-backed presented texture.
`OPR_SKIP_WINDOW=1` forces the documented SKIP; `OPUROM_GPU_DYLIB`,
`OPUROM_RENDER_BIN`, `--ir`, `--width`, `--height` and `--out` remain
available. Metal/API errors, a mismatched sequence or a stale surface print
`FAIL` with the measured numbers and exit 1. A headless Linux runner prints
`SKIP GPU animation loop is macOS/Metal only`.

This is a bounded deterministic sequence of four presented frames on one
window, not a real-time render loop. There is no fps claim, no smoothness or
vsync claim, no device conformance matrix and no Vulkan/ANGLE/D3D backend.

### Pure RGBA8 to PNG path

`frame_to_png(FrameView, path)` works headlessly on macOS:

1. Validate a non-empty RGBA8 frame with checked `width * height * 4`.
2. Wrap the bytes in a `core_graphics::CGDataProvider`, create a real
   `CGImage` (8-bit RGBA, device RGB colour space, big-endian alpha last) and
   read back the image data. No `NSApplication` or window is needed.
3. Encode those bytes with the cached `png` crate as an 8-bit RGBA PNG. Tests
   check the signature, IHDR dimensions/bit depth/colour type and decode the
   file with `png` to assert exact pixel bytes, including alpha.

On non-macOS the function returns
`AdapterError::Backend("AppKitUnavailable: frame_to_png requires macOS")`.
This is an honest error: no CoreGraphics call is faked and there is **no
std-only PNG writer** in this crate. `WindowSet`, the ABI, `Headless` and
`Terminal` remain cross-platform; target-scoped dependencies keep Linux CI free
of AppKit linkage.

### Multi-window state

`WindowSet::new([Size; N])` accepts exactly `N = 2..4` viewports; other counts
return `AdapterError::Backend("WindowSet requires 2..4 viewports")`. Each
viewport owns a separate `Headless` instance, hence its own `Size`, event FIFO
and RGBA8 frame buffer. `resize(index, size)`, `push_event`, `poll_event`,
`present` and `frame` operate on one index only. Any invalid index returns
`AdapterError::Backend("invalid window index")`.

`viewports()` returns every viewport size in index order for hosts that lay out
a multi-window surface in one pass. `resize_all(size)` applies one viewport size
to every window, which covers split-screen and foldable transitions.
`broadcast_event(event)` delivers one
normalized input to every window, which supports synchronized multi-device
debugging. Each window keeps its own event FIFO, so a subsequent per-window
`push_event` reaches only that window. This is deterministic viewport state,
not a set of live OS windows.

### Degradation contract

An adapter that returns `Err` three times consecutively is degraded: the kernel
swaps in `Headless` and keeps the app alive. `tests/conformance.rs` demonstrates
this with a `Flaky` mock adapter returning `Err(AdapterError::Backend(..))`
three times before the headless fallback takes over.

### Windows, Linux and accessibility surfaces (compile-verified)

- `Win32Host::open()` registers a wide-character class with `RegisterClassW`,
  creates a window with `CreateWindowExW`, and `run()` pumps `GetMessageW` /
  `TranslateMessage` / `DispatchMessageW`. `present()` converts RGBA8 to the
  BGRA byte order of a top-down 32-bit `BI_RGB` DIB; `WM_PAINT` calls
  `StretchDIBits`. `WM_DESTROY` calls `PostQuitMessage`. Key, tap and resize
  messages become `InputEvent`s. The shortcut is one window per process.
- `X11Host::open()` dynamically resolves and calls `XOpenDisplay`,
  `XCreateSimpleWindow`, `XMapWindow`, `XSelectInput` and `XFlush`;
  `present()` packs RGBA8 through the TrueColor visual masks into an
  `XCreateImage` DIB and draws it with `XPutImage`. `pump()` / `run()` use
  `XNextEvent`, and `close()` uses `XFreeGC`, `XDestroyWindow` and
  `XCloseDisplay`. Symbols come from `dlopen`/`dlsym` at runtime only when a
  window is opened, so the default Linux build has no `-lX11` link requirement
  and needs no X11 headers. A missing `libX11.so.6` returns
  `X11 unavailable: dlopen(...) failed`; a missing symbol returns
  `X11 unavailable: libX11 is missing a required Xlib symbol`. A 24/32-bit
  TrueColor visual is also required at runtime.
- `WaylandHost::connect()` resolves `wl_display_connect`,
  `wl_display_disconnect`, `wl_display_get_error` and `wl_display_get_fd` from
  `libwayland-client.so.0`/`libwayland-client.so` with `dlopen`/`dlsym` at
  runtime, so the Linux build has no `-lwayland-client` link requirement and
  needs no Wayland headers. `handshake()` writes the setup handshake - the
  `wl_display.get_registry` and `wl_display.sync` requests - to the
  compositor's socket and collects the announced globals and the sync
  callback. A missing `libwayland-client.so.0` returns
  `Wayland unavailable: dlopen(...) failed`; a missing symbol returns
  `Wayland unavailable: libwayland-client is missing a required symbol`.
- The Wayland wire encoding is pure Rust in `src/wayland_wire.rs` and runs its
  unit tests on **every** host, including macOS. It covers the setup handshake
  message layout, the client object ID allocator, the 8-byte opcode/size header
  with its native-endian words and word-aligned padding, the string argument
  encoding, the `wl_shm` stride/format buffer contract, and the host-independent
  event framing. The handshake assertions are concrete wire bytes: for example
  `wl_display@1.get_registry(wl_registry@2)` is
  `01 00 00 00 01 00 0c 00 02 00 00 00` on a little-endian host; event framing
  is covered by fixed-table round-trip tests.
- The Windows and X11 adapters were compiled here but not executed: there is no
  Windows host or mingw linker on this macOS machine, and there is no X server
  for a Linux window. The two target-scoped compile checks are the evidence.
  The Wayland adapter is the same on this host - there is no Wayland
  compositor - so `--verify-wayland` reports `SKIP` with the concrete reason.
- `--post-a11y [tree.json]` parses the renderer's deterministic accessibility tree JSON,
  maps visible nodes to AX roles, labels and bottom-left frames, then posts
  `NSAccessibilityElement`s with `NSAccessibilityPostNotification`. It checks
  `AXIsProcessTrusted()` and `AXUIElementCreateApplication()` first. Denied
  trust reports `SKIP AXIsProcessTrusted returned false: ...`; other OS/API
  failures report `FAIL`. `OPR_SKIP_A11Y=1` forces a
  deterministic SKIP without any Accessibility API call.
- The tree JSON has no parent ids, so the poster builds a flat AX group in
  pre-order. There is no claim of a nested AX hierarchy, VoiceOver speech
  verification or native widget embedding. The pure parser/mapping tests need
  no OS permission.

### Wayland wire adapter

The Wayland client adapter follows the X11 pattern: `libwayland-client` is
opened at runtime, so the crate builds on a host without Wayland development
libraries and the wire encoding is verified by `cargo test` everywhere.

| Piece | Where | What it does |
| --- | --- | --- |
| Runtime loader | `src/wayland.rs` | `dlopen`/`dlsym` of `wl_display_connect`, `wl_display_disconnect`, `wl_display_get_error`, `wl_display_get_fd`; no link-time dependency |
| Setup handshake | `src/wayland_wire.rs` | `wl_display.get_registry` + `wl_display.sync` requests, client object IDs from 2 upwards |
| Message header | `src/wayland_wire.rs` | 32-bit object ID, then a word with the size in the upper 16 bits and the opcode in the lower 16 bits, native-endian, word-aligned padding |
| String argument | `src/wayland_wire.rs` | 32-bit length including the NUL, the bytes, then zero padding to a word boundary |
| `wl_shm` contract | `src/wayland_wire.rs` | 32-bit `ARGB8888`/`XRGB8888` formats at four bytes per pixel, a word-aligned stride of at least `width * 4`, and a pool size of `offset + stride * height` |
| Event framing | `src/wayland_wire.rs` | resize and press/release/move events as two-word messages, a bounded batch decoder, and fixed-table round-trip tests |

Native hosts normalize resize and pointer events into the same two 32-bit
words: Win32 packs `WM_SIZE` and `WM_LBUTTONDOWN` coordinates into `lparam`,
while X11 reads `c_int` fields from `XEvent`. `WireEvent` frames them as
messages on a server-owned object. Fixed tables cover resize widths 0, 1, 320,
1600 and 4096, press/release/move at signed coordinate boundaries, and
multi-event batches that decode back to identical values. `decode_batch` caps
the event count and rejects a trailing partial message, so malformed or
truncated buffers produce an `AdapterError` instead of panicking.

The handshake assertions use concrete wire bytes; event framing uses
fixed-table round-trip tests. On a little-endian host the handshake is:

```text
01 00 00 00 01 00 0c 00 02 00 00 00   wl_display@1.get_registry(wl_registry@2)
01 00 00 00 00 00 0c 00 03 00 00 00   wl_display@1.sync(wl_callback@3)
```

`--verify-wayland` reports the live status. Without a compositor it prints
`SKIP <concrete reason>` and exits **0**; it never claims a PASS:

```sh
cargo run --quiet --offline -- --verify-wayland
# SKIP WAYLAND_DISPLAY is not set
```

The `wayland` registry row stays `Planned` until a live compositor has been
exercised: the wire encoding, the loader and the handshake are implemented and
unit-tested, while a presented surface needs a running compositor. The row is
reachable through `spec_by_name("wayland")` on every host and stays out of the
frozen 14-row target listing.

## Standard new-platform checklist

1. Add a `PlatformSpec` row to `registry()` with the correct IR_SCHEMA.md emitter and
   honest capabilities; keep status `Planned` (or `Scaffold` once an emitter exists).
2. Implement `PlatformAdapter` for the host: normalize touch/mouse/keyboard into
   `InputEvent` (logical pixels, top-left origin, `node_id: u32::MAX` when the host
   cannot hit-test), and present `FrameView` through the host surface.
3. Return `AdapterError` values instead of panicking. Never claim `gpu` without
   real presentation; claim `native_embed` only for a real native host surface.
4. Add conformance tests against the headless oracle: identical IR + viewport =>
   identical RGBA bytes/digest, FIFO event ordering including `Resize`, invalid frame
   length rejection, and the three-`Err` degradation path.
5. Only after all of those pass, flip the host row to `Verified` and update the status
   table above. Never ship a `Verified` label for a scaffold.

## CLI

`cargo run --quiet` (binary `opurom-platform`) prints the 14 target rows as plain
text; line 1 is the header and the next 14 lines are data rows. `--hosts` uses
the same format but lists `Verified` hosts. On macOS it prints header +
`headless` + `terminal` + `appkit`; the default output remains byte-identical.
All unrecognised extra argv is ignored.

`--png <out.png>` writes a deterministic 4x4 RGBA8 gradient through the same
headless `frame_to_png` path and prints nothing on success. The cross-layer
harness can verify the PNG signature and IHDR without starting a GUI session.
A missing path exits 2; a PNG/AppKit error exits 1 with the error on stderr.

`--verify-window` runs the live-window pixel verification described above.
It accepts `--ir <path>`, `--width <n>`, `--height <n>` and `--out <path>`,
prints one `PASS`, `FAIL` or `SKIP` line and exits 0 for PASS and SKIP, 1 for
FAIL.

`--verify-gpu-presentation` runs the real Metal drawable presentation and
on-screen comparison described above. It accepts the same `--ir`, `--width`,
`--height` and `--out` options, prints one line containing
`PASS ... mismatched_pixels=...` or `SKIP <reason>` and exits 0 for PASS and
SKIP, 1 for FAIL. `OPR_SKIP_WINDOW=1` forces a SKIP; `OPUROM_GPU_DYLIB`
overrides the runtime-loaded dylib path.

`--verify-gpu-animation-loop` runs the bounded four-frame animated GPU loop
described above. It accepts the same options (defaulting to the animated IR
fixture), prints one measured `frame=` line per tick plus a final `PASS`,
`FAIL` or `SKIP` line, and exits 0 for PASS and SKIP, 1 for FAIL. Timings are
`dispatch_ms` and `present_ms` per frame; no fps claim is made.

`--post-a11y [tree.json]` posts the accessibility tree on macOS; without a
path it uses the checked-in renderer-tree fixture. It prints
`PASS a11y_nodes=N` only after real AppKit/AX calls, or `SKIP <concrete reason>`
when Accessibility trust is denied or `OPR_SKIP_A11Y=1` is set. A parse or API
failure prints `FAIL <reason>` and exits 1. PASS and SKIP exit 0.

`--verify-wayland` reports the live Wayland status. It prints
`PASS wayland globals=N callback_done=true` after a real handshake with the
compositor, or `SKIP <concrete reason>` when `WAYLAND_DISPLAY` is missing, the
runtime library is absent or the compositor cannot be reached. SKIP exits 0.

```text
name | emitter | status | gpu | native_embed | multi_window
headless | headless | Verified | false | false | false
terminal | terminal | Verified | false | false | false
appkit | appkit | Verified | false | true | true
```

## Verify

```sh
cargo fmt --all
cargo fmt --all -- --check
RUSTFLAGS="-D warnings" cargo build --offline
RUSTFLAGS="-D warnings" cargo test --offline
cargo run --quiet --offline -- --hosts
cargo run --quiet --offline -- --png /tmp/opurom-platform.png
cargo run --quiet --offline -- --verify-window --out /tmp/opurom-window.png
cargo run --quiet --offline -- --verify-gpu-presentation --out /tmp/opurom-gpu-presentation.png
cargo run --quiet --offline -- --verify-gpu-animation-loop --out /tmp/opurom-gpu-animation-loop.png
cargo run --quiet --offline -- --post-a11y
cargo run --quiet --offline -- --verify-wayland
RUSTFLAGS="-D warnings" cargo check --offline --all-targets --target x86_64-pc-windows-gnu
RUSTFLAGS="-D warnings" cargo check --offline --all-targets --target x86_64-unknown-linux-gnu
```

From the umbrella root the built CLI can be invoked directly as
`./opurom-platform/target/debug/opurom-platform --verify-gpu-presentation --out /tmp/opurom-gpu-presentation.png`;
the renderer, IR and GPU dylib paths are absolute and CWD-independent.

`cargo test --offline` runs **95 tests** on macOS: 40 library unit tests
(including four pure animation-loop checks and fifteen pure Wayland wire checks),
two accessibility tests, eleven
AppKit host tests, two CLI tests, twenty-two conformance tests, seven
animation-loop integration tests, four GPU presentation integration tests
(presentation-on-screen, forced SKIP, missing-dylib SKIP and status honesty),
four native-window tests and three Wayland integration tests
(missing display, empty display and a named display without a compositor).
Win32 adds two, X11 adds three and Wayland adds one pure unit test that are
compiled, not
executed, by their target's `--all-targets` check. The Linux source has no
`#[link(name = "X11")]` attribute and therefore no `-lX11` link requirement;
Xlib is loaded only by `X11Host::open()`. It has no
`#[link(name = "wayland-client")]` attribute and therefore no
`-lwayland-client` link requirement either; libwayland-client is loaded only by
`WaylandHost::connect()`. The GPU presentation bridge likewise has no
`#[link(name = "Metal", kind = "framework")]` in this crate; Metal lives in
the runtime-loaded GPU dylib. `cargo fmt --all`, `cargo build --offline` and
`cargo test --offline` produce zero warnings on macOS with
`RUSTFLAGS="-D warnings"`; both cross-target checks are compile-only. No
Windows `.exe` or Linux X11 binary is linked here.

On a host with a GUI session `--verify-window` and
`--verify-gpu-presentation` report `PASS` with the measured numbers. On a
headless host they report `SKIP <reason>` and exit 0. `--verify-wayland`
reports `SKIP <reason>` and exits 0 on a host without a compositor. The three
V0.1 golden digests remain unchanged.
