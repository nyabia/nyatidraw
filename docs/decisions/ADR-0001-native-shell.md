# ADR-0001: Conditional Dioxus Native shell

- Status: **Superseded by [ADR-0008](ADR-0008-dioxus-desktop-child-canvas.md)**
- Date: 2026-09-01

## Decision

Keep Dioxus Native as the provisional desktop shell using its stable 0.7 custom
WGPU paint integration. Pin `dioxus-native = 0.7.9`, `winit = 0.30.12`,
`wgpu = 26.0.1`, and `raw-window-handle = 0.6.2` exactly. The workspace does
not declare an MSRV; `rust-toolchain.toml` follows the current stable compiler.

The desktop executable now constructs the one winit event loop, then gives it a
public `DioxusNativeApplication` handler. Blitz still owns the window, WGPU
surface, and Vello compositor. The canvas registers a `CustomPaintSource`
through `dioxus_native::use_wgpu`. On `resume`, Blitz supplies a `DeviceHandle`;
the canvas clones that handle's device and queue, renders into a registered
texture, and returns the texture to the adjacent Dioxus `<canvas>` element. The
canvas creates no second instance, adapter, device, queue, surface, or event
loop.

On Windows, the executable installs winit 0.30's public `with_msg_hook` callback
while building that event loop. The callback runs before Win32 dispatch, passes
the hook's exact `MSG` to `nyatidraw-input-platform`, and invokes
`WindowsPenRecorder` before Dioxus or Blitz handles the event. It always returns
`false`, leaving message dispatch and window-procedure ownership with winit.
There is no global hook, foreground-window lookup, cursor polling, or HWND
subclass.

Winit 0.30 translates Windows `WM_POINTER` pen traffic into
`WindowEvent::Touch`, but Blitz 0.2.3 currently ignores that variant. The
app-owned `ApplicationHandler` converts a touch gesture that begins outside the
renderer-measured canvas into `CursorMoved` plus left-button transitions for
Blitz. Canvas-origin gestures stay on the raw pen lane. Ordinary mouse gestures
that begin inside the canvas are observed in the same pre-dispatch hook. Windows
mouse history is recovered through a reusable 64-slot
`GetMouseMovePointsEx` buffer, anchored to the last accepted global-history
record and emitted oldest-first as pressure-1 `StylusSample` values. Missing
anchors fall back to the current message point, and the `MI_WP` signature
excludes compatibility mouse messages promoted from pen/touch. The hook still
returns `false`; winit and Blitz retain UI dispatch ownership.

The mouse backend deliberately uses `GMMP_USE_DISPLAY_POINTS`. The earlier
high-resolution normalized-coordinate interpretation failed physical Windows
acceptance by mapping Move samples outside the canvas; display-point history
retains the missing temporal samples without inventing an unverified pixel
conversion. Negative virtual-desktop coordinates are recovered through the
documented low-16 sign extension before subtracting the window client origin.

The pre-dispatch pen adapter uses `GetPointerPenInfoHistory` for in-contact
updates. Windows returns that list newest-first and includes the current sample,
so the adapter emits only the reversed history list, once, into a reusable
bounded buffer. Down/up remain singular transitions. `GetPointerType` filters
mouse/touch before the pen-only API, hover is ignored, cancelled flags map to
`Cancel`, and an update without a known down cannot manufacture a stroke.
Fractional client coordinates come from the HIMETRIC device/display mapping;
the adapter does not quantize the live pen path through integer pixel points.

The diagnostic observer keeps only event-loop-thread counters. Detailed output
is capped at 32 transition records, 8 move records, and 8 errors; the counters
continue after those log caps. Raw samples do not update a Dioxus signal or
enter UI command state. This slice introduces no drawing queue or Sprint 1
stroke rendering.

This validates the ownership direction needed by the architecture:

```text
desktop-created winit event loop
  -> pre-dispatch Windows MSG hook
     -> WindowsPenRecorder + bounded diagnostic observation
  -> DioxusNativeApplication / Blitz
  -> one Blitz/Vello WGPU device and queue
     -> Dioxus UI composition
     -> Nayati custom canvas texture clear
```

The integration follows Dioxus Native's public
[`use_wgpu`](https://docs.rs/dioxus-native/0.7.9/dioxus_native/fn.use_wgpu.html)
API and the official Blitz
[`wgpu_texture` example](https://github.com/DioxusLabs/blitz/tree/v0.2.0/examples/wgpu_texture).
The raw input integration uses winit's public
[`EventLoopBuilderExtWindows::with_msg_hook`](https://docs.rs/winit/0.30.12/winit/platform/windows/trait.EventLoopBuilderExtWindows.html#tymethod.with_msg_hook)
API.

## Exact dependency and feature record

| Direct dependency | Enabled features | License |
|---|---|---|
| `blitz-shell 0.2.3` | defaults disabled; application/event-loop public types only | MIT OR Apache-2.0 |
| `dioxus-native 0.7.9` | `hot-reload`, `prelude`, `system-fonts`; defaults disabled | MIT OR Apache-2.0 |
| `dioxus-devtools 0.7.10` | direct public devserver connect/apply API; no crate features | MIT OR Apache-2.0 |
| `nyatidraw-input-platform 0.1.0` | Windows `WM_POINTER` adapter | workspace |
| `winit 0.30.12` | upstream defaults plus `rwh_06` selected by Dioxus/Blitz | Apache-2.0 |
| `wgpu 26.0.1` | upstream defaults selected by AnyRender/Vello | MIT OR Apache-2.0 |
| `raw-window-handle 0.6.2` | `std` | MIT OR Apache-2.0 OR Zlib |

Relevant resolved transitive versions in `Cargo.lock` are
`dioxus-native-dom 0.7.10`, `dioxus-core 0.7.10`, `blitz-shell 0.2.3`,
`blitz-dom 0.2.4`, `blitz-paint 0.2.1`, `anyrender 0.6.2`,
`anyrender_vello 0.6.2`, and `wgpu_context 0.1.2`.

`dioxus-native 0.7.9` does not compile with only `prelude` and `system-fonts`:
its `Config::default` references the optional `dioxus-cli-config` crate without
feature gating. Enabling `hot-reload` is the narrow available packaging
workaround; the desktop crate directly pins the public matching
`dioxus-devtools 0.7.10` API used by its app-owned event loop. Network, HTML
parsing, SVG, accessibility, clipboard, and file-dialog features remain off.
Recheck this workaround on every Dioxus Native upgrade.

The desktop does not call `dioxus_native::launch_cfg`: that convenience API
immediately creates and builds its own event loop and exposes no builder hook.
Instead it assembles `DioxusDocument`, `DioxusNativeWindowRenderer`,
`WindowConfig`, and `DioxusNativeApplication` using their public APIs. The
minimal launcher leaves the optional net, HTML parser, and navigation providers
unset. Those providers are outside the current offline shell spike; if later UI
requires one, it must be restored explicitly without losing event-loop
ownership. The debug devserver connection is restored separately below.

## DX CLI integration

`apps/desktop` owns `Dioxus.toml` and its `assets/` directory, matching the
Cargo package boundary required by Dioxus 0.7's `asset!` macro. DX 0.7.9 is the
canonical owner of the desktop package's asset collection, check, build, serve,
and bundle workflows. The app-owned native shell remains the runtime entry
point; it is deliberately not replaced with a generated Desktop WebView
template. DX 0.7.9 offers no `dx new` Native renderer template, so re-scaffolding
is not a supported integration route.

DX 0.7.9's `check --file` selects a Rust source file; it does not select a
`Dioxus.toml`. From the repository root, use this location wrapper so discovery
uses the app-local configuration:

```powershell
Push-Location apps/desktop
dx check -p nyatidraw-desktop --windows --renderer native --locked
dx build -p nyatidraw-desktop --windows --renderer native --locked
dx serve -p nyatidraw-desktop --windows --renderer native --locked
dx bundle -p nyatidraw-desktop --windows --renderer native --locked
Pop-Location
```

`assets/styles.css` is the only stylesheet source. `asset!` registers it for DX
collection, while the native app retains `include_str!` inline styling as a
fallback: this private shell has not enabled the upstream native net provider,
so fetching a `dioxus://` stylesheet via `document::Link` is not guaranteed.

In debug builds the app-owned loop restores upstream's public devserver path:
it connects `dioxus_devtools`, forwards `DioxusNativeEvent::DevserverEvent`
through the existing `BlitzShellEvent` proxy, applies hot-reload templates,
reloads changed resources, and exits on a devserver shutdown message. The cfg
matches the effective upstream runtime boundary: debug assertions and
non-Android/non-iOS. `dioxus-native 0.7.9` is built with its `hot-reload`
feature unconditionally because disabling it leaves that crate's own
`dioxus-cli-config` reference unresolved; the release handler remains removed
by `debug_assertions`.

For the Windows target, `cargo tree -d` and inverse trees show exactly one
resolved version of each boundary dependency:

- `wgpu 26.0.1`
- `winit 0.30.12`
- `raw-window-handle 0.6.2`

The full tree contains unrelated duplicate utility crates, but no duplicate
major or version for these three boundary crates.

## Alternatives considered

### Dioxus Native 0.8 alpha

Rejected for this slice. `dioxus-native 0.8.0-alpha.1` pins Blitz 0.3 beta and
winit 0.31 beta. Blitz declares Rust 1.89, while its Vello-hybrid path uses WGPU
29, which declares Rust 1.87. Adopting it would add multiple pre-release
compatibility surfaces.

### Standalone winit + WGPU shell

A standalone `winit 0.30.12` + `wgpu 26.0.1` surface is the fallback if Dioxus
Native loses event-loop or shared-device viability. It is not selected now
because the stable `CustomPaintSource` path successfully demonstrated a shared
device/queue without a second surface.

## Evidence

Validated on:

- Windows 11 Pro 64-bit, build 26200
- NVIDIA GeForce RTX 3080, driver 32.0.15.9621
- WGPU-selected Vulkan backend
- current stable `rustc` from `rust-toolchain.toml`; no declared workspace MSRV

The debug and release applications both created a Blitz window, resumed the
custom source with the renderer's device handle, sized the custom canvas to
642x530 physical pixels at scale 1, and submitted its clear pass. The release
run produced these single-run observations:

- custom canvas resume: 1.580 seconds after process entry
- first custom paint submit: 1.581 seconds after process entry
- release executable size: 21,245,952 bytes (20.26 MiB)

These are startup-impact observations, not a benchmark. There is no fixed
fixture, repeated sample set, or p50/p95/p99 result yet. The custom-paint
callback occurs before Vello composites and presents, so the 1.581-second value
must not be reported as measured first-present latency.

Validation commands:

```powershell
cargo fmt --all -- --check
cargo check -p nyatidraw-desktop
cargo clippy --workspace --all-targets -- -D warnings
cargo metadata --no-deps --format-version 1
cargo tree -p nyatidraw-desktop -d --target x86_64-pc-windows-msvc
cargo build --release -p nyatidraw-desktop
target\release\nyatidraw-desktop.exe
```

After adding the Windows hook, `cargo check -p nyatidraw-desktop`, formatting,
and workspace/all-target Clippy all passed. A bounded debug launch printed
`windows-pen-hook-installed stage=pre-dispatch`, resumed on the RTX 3080 Vulkan
device at 1.189 seconds, and submitted the 642x530 custom canvas clear at 1.191
seconds. The run was intentionally stopped with Ctrl+C after startup, so its
`STATUS_CONTROL_C_EXIT` is the harness termination rather than an application
failure.

## Remaining gates and limitations

- The later Sprint 3 DX build received actual Windows GUI acceptance. The
  compact shell showed checkerboard transparency and completed Zoom r3, layer
  Hide r4, Canvas Bottom dock r5, post-dock Rotate r6, durable white background
  r7, and Fit r8. Root-owned `use_wgpu` prevented dock remount from creating a
  second renderer/recovery repository.
- Dioxus Native 0.7.9 exposes no post-present callback in this integration;
  first-window and first-present need an external measurement harness or a
  bounded shell hook before the Sprint 0 gate can pass.
- Dock remount/resume authority has runtime acceptance. Live window resize,
  monitor DPI change, and device-loss-driven suspend/resume remain unverified.
- Windows selected Vulkan in this run. DX12, macOS/Metal, and Wayland/Vulkan
  remain unverified.
- No pen hardware event occurred during the bounded launch. The public
  pre-dispatch route compiles and the hook installs, but a real
  `WM_POINTERDOWN`/update/up sequence reaching the recorder is not yet
  runtime-verified. Synthetic posting was deliberately not used as evidence.
- The production recorder now admits against renderer-owned canvas origin,
  DPI, affine viewport and revision, freezing the Begin mapping through the
  active stroke. Physical-pen acceptance of this mapping remains open.
- The bounded queue software probe preserves primary+retry capacity, then
  records an explicit discontinuity, cancels the partial stroke without
  materialization, and resumes from a clean Begin. It is not physical-pen or
  input-to-visible evidence.

Therefore this ADR retains Dioxus Native only conditionally. It does not mark
the whole Sprint 0 gate as passed.

## Reversal conditions

Replace the shell while keeping the core crate contracts if Dioxus Native
introduces duplicate WGPU/winit/raw-window-handle majors, cannot preserve a
single shared device across resize/suspend, cannot expose enough event-loop
control for raw stylus input, or loses/delays stroke transitions during Dioxus
rerenders. A standalone winit shell with Dioxus confined to a panel texture is
the first fallback; replacing `ui-dioxus` entirely remains the final fallback.
