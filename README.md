# opurom-platform - Platform Adapter ABI v0.1

Independent Rust crate (zero dependencies, std only) implementing the normative
[ADAPTER_CONTRACT.md](../opurom-spec/ADAPTER_CONTRACT.md). It defines the one
platform-specific ABI, a queryable capability registry, and two fully implemented,
`Verified` host adapters: `Headless` (the v0.1 reference adapter and conformance
oracle) and `Terminal` (R3, deterministic truecolor ANSI half-block output). Both are
thin host adapters that demonstrate the platform boundary.

## Status (honest, v0.1)

| name | emitter | status | gpu | native_embed | multi_window |
| --- | --- | --- | --- | --- | --- |
| headless | headless | **Verified** | false | false | false |
| terminal | terminal | **Verified** | false | false | false |
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

`registry()` returns the **14 target rows** named by ADAPTER_CONTRACT.md, each unique
and each `Scaffold` or `Planned`. The `Verified` rows are **host adapters**, not target
emitters: `headless` (the v0.1 oracle) and `terminal` (R3). `Scaffold` means a registry
row plus a known target emitter (from IR_SCHEMA.md) with no runtime adapter; `Planned`
means the host is on the roadmap with neither.

`all_platforms()` is the frozen v0.1 listing (`headless` + the 14 targets = 15, one
`Verified`); keeping its R2 shape means the R2 regression suite stays a stable oracle.
R3 exposes the two implemented hosts through `host_platforms()`, `HEADLESS_SPEC`,
`TERMINAL_SPEC`, `spec_by_name("terminal")` and the CLI `--hosts` flag - all additive,
no existing row or test changed. **Only `headless` and `terminal` are implemented and
verified.**

OS-native window adapters (android, ios, harmony-mobile, harmony-pc, windows, macos,
linux), mini-program and web host adapters, native widget embedding (`native_embed`),
and **Metal/Vulkan/ANGLE GPU backends** (`gpu`) are later rounds (**R4+**). The terminal
adapter is explicitly **not** one of them: it is a thin host adapter that prints ANSI
text, with no GPU, no OS window and no native widget. That is why `gpu` and
`native_embed` are `false` for every row. The remaining capability flags and
`dpi_scale` values (1.0 desktop/web/terminal, 2.0 mobile) are nominal logical defaults
describing the intended host, **not** measurements. No performance figures (fps, cold
start, binary size) are reported - there is no real GPU backend or device matrix to
measure them on.

## ABI summary

| Item | Contract |
| --- | --- |
| `PlatformAdapter` | `name`, `capabilities`, `viewport`, `poll_event`, `present` - the only platform-specific trait; kernel/compiler/renderer never change for a new host |
| `FrameView<'a>` | borrowed RGBA8 view `{ width: u32, height: u32, pixels: &'a [u8] }`, defined here so the renderer is not a compile-time dependency of adapters |
| `Size` | logical viewport `{ width: u32, height: u32 }` |
| `InputEvent` | `Tap { x, y, node_id }`, `Input { node_id, text }`, `Key { code, pressed }`, `Resize { size }`, `Lifecycle(Lifecycle)`; adapters normalize touch/mouse/keyboard; pass `node_id: u32::MAX` when hit-testing is unavailable |
| `Lifecycle` | `Resumed`, `Paused`, `Destroyed` |
| `Capabilities` | `gpu`, `native_embed`, `multi_window`, `lifecycle`, `dpi_scale` |
| `AdapterError` | `InvalidFrame` (bad RGBA8 length) and `Backend(&'static str)`; always a value, never a panic across the boundary |
| `Status` | `Verified`, `Scaffold`, `Planned` |
| `PlatformSpec` / `registry()` / `spec_by_name()` | name, target emitter, capabilities, status; lookup by name |
| `host_platforms()` | the implemented `Verified` host adapters (`headless`, `terminal`); listed by `--hosts` |

### Headless reference adapter

`Headless::new(w, h)` starts with a viewport and an empty FIFO event queue.

- `push_event` queues an `InputEvent`; `poll_event` drains it FIFO.
- `resize(size)` updates the viewport and enqueues `InputEvent::Resize`.
- `present(frame)` copies the RGBA bytes into an internal buffer, exposes it via
  `frame()`, and stores a deterministic 16-char hex digest available via `digest()`.
- Wrong buffer length (anything other than `width * height * 4`, including an
  overflowing product) returns `AdapterError::InvalidFrame` and leaves state untouched.

The digest is an inline **FNV-1a 64-bit** checksum: deterministic across runs and
hosts so golden-pixel tests can assert byte-identical output. It is a checksum, not a
cryptographic hash - the spec's "sha256-like" only requires stability. Upgrade to a
real digest if golden hashes ever need collision resistance.

### Terminal host adapter (R3, Verified)

`Terminal` is the second fully implemented and `Verified` host adapter. It implements
the same `PlatformAdapter` trait as `Headless` with **zero kernel changes** - the
platform-boundary demonstration of
[R3_ADDENDUM.md section 6](../opurom-spec/R3_ADDENDUM.md).

> **Scope:** `Terminal` is a **thin host adapter** that demonstrates the platform
> boundary. It is **NOT** a Metal/Vulkan/ANGLE GPU backend, not an OS window and not a
> native widget host. It writes deterministic text to a `String`; nothing is drawn on a
> GPU. Real OS-native window adapters and GPU backends remain **R4+** work.

- `Terminal::new(width, height)` sets the logical `viewport()` (pixel columns x pixel
  rows); `Terminal::with_defaults()` uses 80x24.
- `present(FrameView)` converts the RGBA8 frame into truecolor **ANSI half-block text**:
  one `U+2580` (upper half block) character per pixel column and **two pixel rows per
  character row**, so a `w x h` frame yields `w` characters x `ceil(h / 2)` lines. The
  foreground is the top pixel and the background the bottom pixel, emitted as
  `\x1b[38;2;R;G;Bm\x1b[48;2;R;G;Bm` before each glyph, with `\x1b[0m` at each line
  end. Odd heights pad the missing bottom row with transparent/black `(0,0,0,0)`.
  Output is a pure function of the frame bytes, byte-identical for identical frames,
  with no TTY, threads, randomness or wall clock.
- `last_output()` (alias `render_text()`) returns the exact ANSI bytes so tests assert
  them without a terminal. Wrong buffer length (anything other than
  `width * height * 4`, including overflow) returns `AdapterError::InvalidFrame` and
  leaves the last output untouched.
- `push_key(code, pressed)`, `push_input(InputEvent)` and `push_event(InputEvent)` queue
  parsed input; `poll_event()` drains FIFO into `InputEvent::Key` / `Input` / `Tap`.
- `capabilities()`: `gpu:false, native_embed:false, multi_window:false,
  lifecycle:false, dpi_scale:1.0`; `viewport()` reflects `width`/`height`.

It is registered as a `Verified` **host** (`TERMINAL_SPEC`), reachable via
`spec_by_name("terminal")` and listed by `host_platforms()` / `cargo run -- --hosts`.
It is deliberately **not** one of the 14 target rows, so `registry()` and the default
CLI output are unchanged.

### Degradation contract

An adapter that returns `Err` three times consecutively is degraded: the kernel swaps
in `Headless` and keeps the app alive. `tests/conformance.rs` demonstrates this with a
`Flaky` mock adapter returning `Err(AdapterError::Backend(..))` three times before the
headless fallback takes over.

## Standard new-platform checklist

1. Add a `PlatformSpec` row to `registry()` with the correct IR_SCHEMA.md emitter and
   honest capabilities; keep status `Planned` (or `Scaffold` once an emitter exists).
2. Implement `PlatformAdapter` for the host: normalize touch/mouse/keyboard into
   `InputEvent` (logical pixels, top-left origin, `node_id: u32::MAX` when the host
   cannot hit-test), and present `FrameView` through the host surface.
3. Return `AdapterError` values instead of panicking; never claim `gpu` or
   `native_embed` until real presentation/embedding exists.
4. Add conformance tests against the headless oracle: identical IR + viewport =>
   identical RGBA bytes/digest, FIFO event ordering including `Resize`, invalid frame
   length rejection, and the three-`Err` degradation path.
5. Only after all of those pass, flip the row to `Verified` and update the status table
   above. Never ship a `Verified` label for a scaffold.

## CLI

`cargo run --quiet` (binary `opurom-platform`) prints the registry as plain text -
no arguments required; `--hosts` switches to the host list; all other extra argv is
ignored; exit 0:

```text
name | emitter | status | gpu | native_embed | multi_window
android | native | Planned | false | false | false
...
web | web | Scaffold | false | false | false
```

Line 1 is the header; the following **exactly 14 lines are data rows**, one per target,
with ` | ` as the separator and lowercase booleans. No separator/underline line, no
JSON. `headless` is a host adapter rather than a target emitter, so it is not a row
here; it is the `Verified` row in the status table above and is queryable via
`spec_by_name("headless")`.

`cargo run --quiet -- --hosts` uses the **same parseable format** but lists the two
implemented `Verified` host adapters instead (header + `headless` + `terminal`):

```text
name | emitter | status | gpu | native_embed | multi_window
headless | headless | Verified | false | false | false
terminal | terminal | Verified | false | false | false
```

This keeps the default 14 target rows byte-identical for the CLI e2e while exposing the
new R3 host through a separate flag. `terminal` is deliberately **not** a 15th target
row.

## Verify

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test
```
