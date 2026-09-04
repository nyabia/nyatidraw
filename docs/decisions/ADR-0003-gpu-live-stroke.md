# ADR-0003: Persistent GPU working texture for first visible ink

- Status: **Conditional — GPU mutation path accepted, Sprint 1 gate open**
- Date: 2026-09-01

## Decision

Use a `nyatidraw-paint-gpu` backend built on the already pinned `wgpu 26.0.1`
device and queue supplied by Dioxus Native's `CustomPaintSource::resume`. The
backend clones those lightweight handles but never creates a WGPU instance,
adapter, device, queue, surface, or readback buffer.

The first vertical slice maintains one viewport-sized persistent
`Rgba8Unorm` working texture. It clears the texture once, registers the same
texture with Vello, and applies each non-empty `BrushDab` batch with one
instanced render pass using `LoadOp::Load`. Prior pixels therefore survive
between redraws. A batch upload uses one reusable vertex buffer write and one
queue submission; it does not submit or read back once per dab. A CPU-computed
dirty rectangle records the changed pixel union, although the first backend is
not yet a sparse tile atlas.

The shader follows the CPU reference contract:

- target pixels are premultiplied linear-light RGBA8;
- effective source alpha is `coverage * dab.opacity * dab.flow`;
- source-over blending uses a straight-alpha, linear-light brush uniform and
  fixed-function alpha blending, which stores premultiplied output;
- circle coverage uses the same fixed 4x4 subpixel offsets as
  `nyatidraw-paint-cpu`.

The matching formula and sample locations are verified with an explicit
closed-stroke readback probe. The acceptance contract is
`GPU_UNORM_CLOSED_STROKE_TOLERANCE`: every RGBA8 channel may differ by at most
four bytes, and zero pixels may exceed that four-byte difference. It is not a
percentage allowance. CPU source-over rounds after every dab, while the
`Rgba8Unorm` attachment performs fixed-function blend/UNORM conversion on the
GPU. The two valid rounding paths can accumulate on repeated low-alpha
build-up, so a one-byte limit is disproven by the probe corpus. A result above
four bytes is a failure, not a backend-specific waiver.

## Native input and redraw path

The desktop integration is:

```text
winit pre-dispatch MSG hook
  -> WindowsPenRecorder
  -> InputQueue(capacity 512)
  -> payload-free LiveInkRedraw user event
  -> custom paint callback drains queued samples
  -> RoundBrushEvaluator emits BrushDab batch
  -> one persistent-texture GPU pass
  -> Vello samples the same registered texture
```

Only `StylusSample` values enter the bounded input queue. The event-loop user
event carries no sample or GPU data; it is only a redraw marker. No per-sample
Dioxus signal, component state update, UI command, or async task is created.
The hook sends that marker only when pending work changes from empty to
non-empty, so a burst of pen samples does not create one user event per sample.
The application wrapper records actual winit `WindowId` values and consumes the
marker independently of whether winit translates a particular `WM_POINTER`
message into `WindowEvent::Touch`.

The native adapter expands coalesced `WM_POINTERUPDATE` history before this
queue boundary and preserves oldest-to-newest timestamp/sequence order. The
latest history item is not appended a second time. Its reusable scratch storage
has a 4,096-sample hard limit; overflow is an explicit input error rather than
an unbounded allocation on the event-loop thread.

The marker schedules the real winit window redraw and remains latched until the
corresponding `RedrawRequested` arrives. It does not directly call the renderer
from the user-event handler. Within one custom-paint drain, adjacent dab output
for the same live generation is appended to one GPU operation, so a burst of
Move samples creates one instanced brush submission rather than one submission
per retained sample.

`InputQueue` retains ordinary move samples until saturation. At saturation it
evicts the least-curved move while retaining the latest endpoint and accumulated
pressure extrema; begin/end/cancel transitions evict moves first. If a queue is
temporarily made entirely of transitions, `LiveInkBridge` places the transition
in a second fixed-capacity retry lane and returns immediately; the next redraw
drains the regular lane before that retry lane, preserving event order. The
retry lane is bounded by the configured queue capacity, so a pathological burst
cannot grow memory or block the Win32 hook. Exhaustion of both lanes latches the
first lost sequence/phase exactly once, wakes the consumer, and quarantines
later samples. The consumer drains preserved input, cancels the partial
evaluator/GPU transaction without materialization, acknowledges the
discontinuity, and resumes only at a later clean `Begin`.

## Dependency record

No new third-party version was introduced for this slice.

| Package | Relevant dependencies | License |
|---|---|---|
| `nyatidraw-paint-gpu 0.1.0` | workspace `nyatidraw-brush`; exact `wgpu 26.0.1` | workspace / WGPU MIT OR Apache-2.0 |
| `nyatidraw-paint-cpu 0.1.0` | workspace `nyatidraw-brush`, `nyatidraw-input` | workspace |
| `nyatidraw-input-queue 0.1.0` | workspace `nyatidraw-input` | workspace |
| `nyatidraw-desktop 0.1.0` | the three workspace boundaries above plus ADR-0001 shell dependencies | workspace |

`Cargo.lock` continues to resolve one `wgpu 26.0.1`, one `winit 0.30.12`, and
one `raw-window-handle 0.6.2` for the desktop boundary.

## Runtime evidence

Focused compile and lint validation:

```powershell
cargo fmt --all -- --check
cargo check -p nyatidraw-desktop
cargo clippy -p nyatidraw-paint-gpu -p nyatidraw-desktop --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p nyatidraw-desktop
```

On Windows 11 build 26200 with an NVIDIA GeForce RTX 3080 using the Vulkan
backend, a bounded release launch with `NAYATI_SYNTHETIC_INK=1` produced:

- custom source resume at 0.871 seconds after process entry;
- one 642x530 persistent `Rgba8Unorm` texture;
- one explicitly labelled `synthetic-gpu-proof` batch with 49 dabs;
- dirty union `(94,153)..(548,377)`;
- accepted custom-paint submission at 0.897 seconds.

The process was stopped with Ctrl+C after observation. Its
`STATUS_CONTROL_C_EXIT` is the bounded harness termination, not a runtime
failure. WGPU accepted the WGSL shader, render pipeline, persistent-texture
load pass, and Vello registration without a validation error.

These elapsed values are neither input latency nor first-present measurements.
The seed does not traverse Win32 input, and the custom-paint callback runs before
Vello composition/present. No p50/p95/p99 distribution was collected.

The separate release hardware probe below compares six closed batches on a
37x29 `Rgba8Unorm` tile: overlapping blue dabs, red dabs clipped at all four
edges, four low-flow green overlaps, dense yellow overlap, eight low-alpha cyan
overlaps, and twelve mid-alpha magenta overlaps. It varies fractional
centers/radii, colours, opacity, flow, clipping, and repeated source-over.

```powershell
cargo run -p nyatidraw-paint-gpu --example gpu_cpu_diff --release
```

On that same RTX 3080/Vulkan configuration, the corpus maxima were respectively
1, 1, 2, 1, 4, and 2 bytes; the eight-dab low-alpha cyan overlap establishes
the smallest observed zero-excess threshold of 4. Every fixture passed that
explicit threshold. The former four-dab blue fixture had 136 differing pixels
under exact comparison (max channel delta 1, total absolute error 142), which
is expected UNORM rounding evidence rather than exact-byte equality. The probe
prints JSON and exits non-zero whenever any fixture exceeds the threshold.
This is a hardware probe rather than an automated test because it creates a
real adapter/device. It is evidence for this fixed corpus, not a proof that an
arbitrarily long stroke or a changed blend contract remains within four bytes;
those changes require another measurement.

## Synthetic proof boundary

Synthetic ink is disabled by default. Setting `NAYATI_SYNTHETIC_INK=1` inserts
one deterministic 49-dab wave directly into the GPU batch path after the first
working texture is created. Logs always label it `synthetic-gpu-proof` with
`samples=0`. It proves shader/pipeline submission and persistent texture
mutation only; it must never be cited as pen-input, queue-latency, or
input-to-visible evidence.

## Open limitations and reversal conditions

The first six items below document the original viewport-sized working-texture
spike. They are retained as history; the Sprint 3 extension at the end of this
ADR supersedes its identity-viewport and no-materialization statements. The
physical-pen, first-visible/present, latency, cross-backend, and high-refresh
limitations remain current.

- No physical pen event occurred in this run. Raw Win32 enqueue, marker-driven
  redraw, brush evaluation, and visible mutation as one runtime chain remain
  unverified.
- The recorder used an identity whole-window viewport snapshot in this original
  spike. The current Windows slice publishes canvas origin, DPI, affine view,
  and revision, but physical-pen acceptance of that mapping remains unverified.
- The queue carries pressure extrema from evicted moves, but the current desktop
  evaluator consumes the retained endpoint sample only. Saturation-to-dab
  pressure-extrema semantics remain unresolved.
- The reusable GPU instance buffer grows to the next power of two for a larger
  dab batch, and the evaluator's dab vector has no independent hard cap yet.
  The input queue is bounded, but malformed large coordinate jumps and the 8K
  batch-memory stress case remain unverified.
- Transition-only saturation now has a bounded, non-blocking desktop retry
  path plus explicit fail-closed discontinuity recovery. The C=512 probe
  preserved 2C transitions, latched one discontinuity at 2C+1, cancelled the
  partial layer-1 stroke without materialization, and recovered on a clean
  layer-2 Begin. Physical-pen delivery, sustained transition bursts, and
  first-visible-pixel timing remain unverified.
- The original viewport-sized texture was replaced by the Sprint 3 fixed
  document-space layer scene. Sparse signed atlas, device-loss acceptance, and
  GPU readback/hybrid checkpoint comparison remain open.
- The RTX 3080/Vulkan GPU/CPU golden probe is passed for its six-fixture
  corpus, but D3D12, OpenGL, software/fallback adapters, other GPU vendors,
  other driver versions, longer build-up sequences, and changed blend/shader
  contracts remain unverified. 4K p95 input-to-visible measurement, 8K stress,
  continuous UI-panel interaction, and a real present timestamp are unrun.
- Visual pixels were not captured by an automated screenshot in this run; the
  evidence is successful GPU command validation and submission logging.

Keep this backend only if physical-pen acceptance preserves begin/end under UI
interaction and the later GPU/CPU diff falls inside an explicitly measured
tolerance. If same-texture mutation conflicts with Vello sampling on any target,
retain the shared Device/Queue but introduce a bounded working/display texture
copy or double buffer; do not create a second GPU context.

## Sprint 3 display/composite extension (2026-09-01)

The earlier viewport-sized working-texture limitations above are superseded for
the Windows Sprint 3 slice. Live dabs now mutate a stable 1024x768 raster-layer
texture. Closed CPU replay sends changed 128x128 tiles back to that renderer
layer, so retained GPU pixels are not the only representation of a closed
stroke. A separate viewport-sized texture samples the cached root composite
through pan/zoom/rotation; display resize does not discard document textures.

The renderer still uses only the Dioxus-supplied `Device`/`Queue`. Two raster
textures plus nested-group/root composites use coordinate-scoped
`CompositeCache` invalidation. The temporary full-document strategy rejects a
scene above a 512 MiB persistent-surface budget before allocation; sparse signed
tiles remain open.

`gpu_layer_viewport --release` passed on RTX 3080/Vulkan for premultiplied
source-over, group visibility/opacity/order, one-tile incremental rebuild, and
combined pan/2x zoom/90-degree rotation readback. It also read back both values
of the view-only checkerboard behind transparent artwork.
`gpu_live_stroke_transaction` proved exact Cancel restoration, Commit
retention, replacement cancellation, stale-token rejection, and zero
full-document readbacks on the live path. This does not add physical-pen,
first-present, D3D12, or cross-platform evidence.
