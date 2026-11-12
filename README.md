# opurom-platform - Platform Adapter ABI v0.1 + R7 live native window

Independent Rust crate implementing the normative
[ADAPTER_CONTRACT.md](../opurom-spec/ADAPTER_CONTRACT.md). The ABI, registry,
`Headless`, `Terminal` and `WindowSet` remain pure Rust. PNG encoding uses the
cached `png` crate; macOS AppKit/CoreGraphics dependencies are target-scoped,
so Linux CI builds the crate without AppKit. R5 adds a real macOS `NSApplication`
/ `NSWindow` / `NSView` host descriptor with a `CAMetalLayer`-compatible surface,
a headlessly verifiable RGBA8-to-PNG path and deterministic multi-window state.
R7 turns `open_window()` into a **tested entry point**: it opens a real AppKit
window, captures the window's own pixels with `cacheDisplay(in:to:)` and
compares them against the software renderer.

## Status (honest, R7)

| name | emitter | status | gpu | native_embed | multi_window |
| --- | --- | --- | --- | --- | --- |
| headless | headless | **Verified** | false | false | false |
| terminal | terminal | **Verified** | false | false | false |
| appkit | appkit | **Verified** (macOS only) | false | true | true |
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
14 targets = 15, one `Verified`), so the R2 regression oracle is unchanged. The
`Verified` rows above are **host adapters**, not target emitters.
`host_platforms()` / `--hosts` list `headless`, `terminal` and, on macOS only,
`appkit`; other targets list only the two cross-platform hosts. `appkit` is
also reachable through `spec_by_name("appkit")` on macOS.

`APPKIT_SPEC.native_embed: true` reports the real `NSView` / `CAMetalLayer`
native host surface. It does **not** claim an embedded AppKit control/widget
tree: R5 embeds no native widgets. `gpu` stays `false` because `present` writes
a PNG rather than drawing to a Metal drawable. `appkit_capabilities()` reports
`NSScreen.backingScaleFactor` on the macOS main thread when a screen is
available; the documented fallback is `1.0` on a non-main thread, with no
screen, for an invalid scale, and in the cross-platform `APPKIT_SPEC` constant.
The remaining target-row capability flags and `dpi_scale` values are nominal
logical defaults, not measurements. No fps, device-matrix or benchmark claims
are made.

OS-native windows for Windows/Linux/Harmony, native widget embedding, full GPU
UI rasterization, Vulkan/ANGLE/D3D backends, and real device conformance remain
**R8+**. R7 does exercise a live GUI session, but only on macOS and only for the
single AppKit host below; the Linux CI runner has no window server and always
reports SKIP.

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
| `WindowSet` | 2..4 independent viewports with isolated sizes, event FIFOs and frame buffers |
| `frame_to_png(FrameView, path)` | real PNG output through a `CGImage` on macOS; documented `AppKitUnavailable` error elsewhere |
| `open_window()` (macOS) | real `NSWindow`/`NSView`; draw, `cacheDisplay(in:to:)` capture, real PNG |
| `compare_rgba_to_ppm` | captured window pixels vs the software renderer PPM, with a documented tolerance |

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

### Terminal host adapter (R3, Verified)

`Terminal` implements the same `PlatformAdapter` trait with deterministic
truecolor ANSI half-block output. It is a thin host adapter, not an OS window or
GPU backend. `present(FrameView)` emits one `U+2580` per column and two pixel
rows per character row; `last_output()` / `render_text()` expose the exact ANSI
bytes. `push_key`, `push_input` and `push_event` queue normalized input;
`poll_event()` drains FIFO. Its capabilities are all false with `dpi_scale: 1.0`.

### AppKit host adapter (R5, Verified on macOS)

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

### Live window and on-screen pixel verification (R7)

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

**SKIP, never a fake PASS.** When the host cannot show a window - no Aqua
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

The measured R7 result on the reference host (macOS arm64, 200x150 frame) is
**200x182 captured pixels** (32 px of system title bar above the 200x150
content), **0 mismatching pixels out of 30 000 compared**, and a maximum
per-channel delta of **25**.

#### R7 scope (honest)

- **macOS only.** The live window and the pixel verification do not exist on
  other platforms; they report SKIP with that reason.
- **One window at a time in R7.** `WindowSet` is still deterministic viewport
  state, not a set of live OS windows.
- **No multi-display.** The window is created on the main screen only.
- **No HiDPI colour management beyond the system default.** The capture uses
  the display profile; the tolerance absorbs the shift, it does not correct it.
- **No native widget embedding.** `native_embed: true` reports the real
  `NSView`/`CAMetalLayer` host surface; no AppKit controls are embedded.
- **No native windows on Windows, Linux or Harmony** (R8+), and no GPU
  rasterization or Metal drawable presentation (`gpu` stays `false`).

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
`AdapterError::Backend("invalid window index")`. This is deterministic viewport
state, not a set of live OS windows.

### Degradation contract

An adapter that returns `Err` three times consecutively is degraded: the kernel
swaps in `Headless` and keeps the app alive. `tests/conformance.rs` demonstrates
this with a `Flaky` mock adapter returning `Err(AdapterError::Backend(..))`
three times before the headless fallback takes over.

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

`--verify-window` runs the live-window pixel verification described above. It
accepts `--ir <path>`, `--width <n>`, `--height <n>` and `--out <path>`, prints
one `PASS`, `FAIL` or `SKIP` line and exits 0 for PASS and SKIP, 1 for FAIL.

```text
name | emitter | status | gpu | native_embed | multi_window
headless | headless | Verified | false | false | false
terminal | terminal | Verified | false | false | false
appkit | appkit | Verified | false | true | true
```

## Verify

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
cargo build --offline
cargo test --offline
cargo run --quiet --offline -- --hosts
cargo run --quiet --offline -- --png /tmp/opurom-r5.png
cargo run --quiet --offline -- --verify-window --out /tmp/opurom-window.png
```

All 30 R6 tests stay green; R7 adds 14 tests (the live-window pixel
verification, the `OPR_SKIP_WINDOW=1` SKIP path, the documented tolerance, the
PPM reader, the tolerant pixel comparison and the `CGImage` bridge). `cargo
fmt --all`, `cargo build --offline` and `cargo test --offline` produce zero
warnings on macOS and on the Linux CI target.

On a host with a GUI session the `--verify-window` run reports `PASS` with the
measured numbers. On a headless host it reports `SKIP <reason>` and exits 0.
