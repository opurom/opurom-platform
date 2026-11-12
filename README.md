# opurom-platform - Platform Adapter ABI v0.1 + R5 macOS host

Independent Rust crate implementing the normative
[ADAPTER_CONTRACT.md](../opurom-spec/ADAPTER_CONTRACT.md). The ABI, registry,
`Headless`, `Terminal` and `WindowSet` remain pure Rust. PNG encoding uses the
cached `png` crate; macOS AppKit/CoreGraphics dependencies are target-scoped,
so Linux CI builds the crate without AppKit. R5 adds a real macOS `NSApplication`
/ `NSWindow` / `NSView` host descriptor with a `CAMetalLayer`-compatible surface,
a headlessly verifiable RGBA8-to-PNG path and deterministic multi-window state.

## Status (honest, R5)

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
**R6+**. No live GUI session was exercised in tests or CI.

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
view and calls `makeKeyAndOrderFront`. It does not run an event loop. **Tests and
CI never call `open_window()`**; no GUI session is available or exercised, and
`MacHost::describe()`/the PNG path are the headlessly verified surface.

`MacHost::new(w, h, path)` keeps a `Headless`-backed viewport, event FIFO and
frame buffer. `present(FrameView)` calls `frame_to_png`, then stores the frame
and digest; an invalid frame leaves both state and any previous PNG untouched.
`capabilities()` uses the real/fallback DPI scale described above.

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
```

All 18 R4 tests stay green; R5 adds 12 tests (including CLI `--png`, PNG pixel
decoding, AppKit description/capabilities, `WindowSet` bounds/isolation and the
documented non-macOS fallback). `cargo fmt --all`, `cargo build --offline` and
`cargo test --offline` produce zero warnings.
