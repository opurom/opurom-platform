# opurom-platform — Platform Adapter ABI v0.1

Independent Rust crate (zero dependencies, std only) implementing the normative
[ADAPTER_CONTRACT.md](../opurom-spec/ADAPTER_CONTRACT.md). It defines the one
platform-specific ABI, a queryable capability registry, and `Headless` — the
reference adapter and conformance oracle for every future host.

## Status (honest, v0.1)

| name | emitter | status | gpu | native_embed | multi_window |
| --- | --- | --- | --- | --- | --- |
| headless | headless | **Verified** | false | false | false |
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
and each `Scaffold` or `Planned`. `HEADLESS_SPEC` and `all_platforms()` additionally
expose the headless reference adapter — the **only** `Verified` platform in v0.1.
`Scaffold` means a registry row plus a known target emitter (from IR_SCHEMA.md) with
no runtime adapter; `Planned` means the host is on the roadmap with neither.

**Only headless is implemented and verified.** OS-native adapters (android, ios,
harmony-mobile, harmony-pc, windows, macos, linux), mini-program and web host adapters,
native widget embedding (`native_embed`), and GPU/ANGLE/Metal/Vulkan presentation
(`gpu`) are later rounds (R2+). That is why `gpu` and `native_embed` are `false` for
every row: v0.1 has no GPU backend and no native widget host. The remaining capability
flags and `dpi_scale` values (1.0 desktop/web, 2.0 mobile) are nominal logical defaults
describing the intended host, **not** measurements. No performance figures (fps, cold
start, binary size) are reported — there is no real GPU backend or device matrix to
measure them on.

## ABI summary

| Item | Contract |
| --- | --- |
| `PlatformAdapter` | `name`, `capabilities`, `viewport`, `poll_event`, `present` — the only platform-specific trait; kernel/compiler/renderer never change for a new host |
| `FrameView<'a>` | borrowed RGBA8 view `{ width: u32, height: u32, pixels: &'a [u8] }`, defined here so the renderer is not a compile-time dependency of adapters |
| `Size` | logical viewport `{ width: u32, height: u32 }` |
| `InputEvent` | `Tap { x, y, node_id }`, `Input { node_id, text }`, `Key { code, pressed }`, `Resize { size }`, `Lifecycle(Lifecycle)`; adapters normalize touch/mouse/keyboard; pass `node_id: u32::MAX` when hit-testing is unavailable |
| `Lifecycle` | `Resumed`, `Paused`, `Destroyed` |
| `Capabilities` | `gpu`, `native_embed`, `multi_window`, `lifecycle`, `dpi_scale` |
| `AdapterError` | `InvalidFrame` (bad RGBA8 length) and `Backend(&'static str)`; always a value, never a panic across the boundary |
| `Status` | `Verified`, `Scaffold`, `Planned` |
| `PlatformSpec` / `registry()` / `spec_by_name()` | name, target emitter, capabilities, status; lookup by name |

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
cryptographic hash — the spec's "sha256-like" only requires stability. Upgrade to a
real digest if golden hashes ever need collision resistance.

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
4. Add conformance tests against the headless oracle: identical IR + viewport ⇒
   identical RGBA bytes/digest, FIFO event ordering including `Resize`, invalid frame
   length rejection, and the three-`Err` degradation path.
5. Only after all of those pass, flip the row to `Verified` and update the status table
   above. Never ship a `Verified` label for a scaffold.

## CLI

`cargo run --quiet` (binary `opurom-platform`) prints the registry as plain text —
no arguments required, extra argv ignored, exit 0:

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

## Verify

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test
```
