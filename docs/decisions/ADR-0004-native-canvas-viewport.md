# ADR-0004: Renderer-owned native canvas viewport admission

- Status: **Shell-specific parts superseded by [ADR-0008](ADR-0008-dioxus-desktop-child-canvas.md)**
- Date: 2026-09-01

## Decision

Own `BlitzApplication<DioxusNativeWindowRenderer>` in the desktop executable
instead of wrapping the opaque `DioxusNativeApplication`. This preserves the
app-owned winit event loop and its pre-dispatch Windows message hook while also
making the live Blitz `View` and resolved `DioxusDocument::inner` available at
the redraw boundary.

Native pen admission uses one revisioned snapshot containing the canvas
content-box origin in physical window-client pixels, its physical width and
height, and the live render scale. The snapshot becomes complete only after:

1. Blitz resolves and paints the current document;
2. the shell queries `#shared-gpu-canvas` in that same document;
3. the shell reconstructs the untransformed content-box origin using the same
   public layout values and traversal order as Blitz Paint; and
4. its calculated physical width, height, and scale exactly match the values
   most recently published by `CustomPaintSource::render`.

Missing nodes, a detached or cyclic layout path, invalid geometry, a non-zero
canvas-local scroll offset, a missing custom-paint surface, or any surface
mismatch clears the origin and keeps admission fail-closed. Resize, scale,
scroll, zoom, resource load, and VDOM changes invalidate the origin until a
subsequent redraw publishes another complete snapshot.

The Win32 recorder installs the complete mapping before decoding the message.
The mapping subtracts the physical content-box origin, leaves the result in
physical canvas pixels, stamps the viewport revision, and admits only points
inside `[0,width) x [0,height)`. No Dioxus signal is updated per sample.

## Public seam and coordinate derivation

The resolved packages are:

- `dioxus-native 0.7.9` and `dioxus-native-dom 0.7.10`;
- `blitz-shell 0.2.3`, `blitz-dom 0.2.4`, and `blitz-paint 0.2.1`;
- `anyrender 0.6.2` and `anyrender_vello 0.6.2`.

`blitz-shell 0.2.3` publicly exposes `BlitzApplication::windows`, `View::doc`,
`View::window`, and `View::downcast_doc_mut`. `DioxusDocument::inner` is a
public `BaseDocument`, whose selector lookup, tree, viewport scroll, node
`layout_parent`, `unrounded_layout`, and `scroll_offset` are public. These APIs
are sufficient for the fixed shell even though the custom-paint callback
itself still contains only width, height, and scale.

Blitz Paint begins at negative viewport scroll. For each node from the root
element through the canvas, it adds that node's unrounded layout location; on
descent it subtracts each ancestor's scroll offset. The canvas content origin
then adds its unrounded border and padding. The shell mirrors that calculation:

```text
logical box origin
  = -viewport_scroll
  + sum(root..canvas unrounded layout locations)
  - sum(root..canvas parent ancestor scroll offsets)

physical content origin
  = (logical box origin + canvas border start + canvas padding start) * scale
```

Physical content extents use the same scaled unrounded size minus scaled
border and padding operations that `blitz-paint` uses to construct its CSS
content box, followed by the same non-negative `u32` conversion boundary.
Exact equality with the width and height observed by the custom-paint source is
the independent guard against layout drift or a mistaken reconstruction.

Relevant upstream sources:

- [Dioxus Native application initialization](https://docs.rs/dioxus-native/0.7.9/src/dioxus_native/dioxus_application.rs.html)
- [Blitz shell application](https://docs.rs/blitz-shell/0.2.3/src/blitz_shell/application.rs.html)
- [Blitz shell view](https://docs.rs/blitz-shell/0.2.3/src/blitz_shell/window.rs.html)
- [Blitz canvas painting](https://docs.rs/blitz-paint/0.2.1/src/blitz_paint/render.rs.html#604-626)
- [Blitz DOM selector lookup](https://docs.rs/blitz-dom/0.2.4/src/blitz_dom/query_selector.rs.html)

The desktop already directly pinned `blitz-shell = 0.2.3`; this change added no
root or package dependency and did not vendor or fork an upstream crate.

## Shell initialization boundary

The app-owned application follows Dioxus Native's public initialization order:
it calls `View::init`, injects a small implementation of the public
`dioxus_document::Document` event contract and the cloned
`DioxusNativeWindowRenderer` into the root VDOM scope before `initial_build`,
inserts the view, and then lets `BlitzApplication::resumed` resolve the document
and resume/render the one renderer. Its document adapter sends
`DioxusNativeEvent::CreateHeadElement`; the application mutates the matching
document and polls its view, matching the upstream native head-event path.

Dioxus Native's convenience launcher also installs history, native asset/net,
navigation, optional HTML parser, and devserver providers. This bounded shell
does not claim parity for the first four integrations. In particular, a
`document::Link` can create its head element, but `DocumentConfig::default()`
uses Blitz's dummy net provider, so a `dioxus://` stylesheet produced by
`asset!` is not proven to load. The current UI deliberately retains an inline
`style` node. The app-owned loop now restores the public
`dioxus_devtools::connect` and `DevserverEvent` apply/reload path used upstream.
A bounded `dx serve` run reached the native Vulkan canvas, but an actual source
or asset edit was not performed, so end-to-end hot-reload delivery remains an
explicitly unverified gate.

## Transform and clip invariant

Blitz's public layout tree does not expose the final composed paint affine or a
custom-paint clip in the callback. This implementation is therefore not a
general transformed-DOM hit-testing API. The current app shell explicitly sets
`transform: none` on every canvas ancestor and the canvas itself, constrains
overflow at the fixed shell boundaries, and keeps the canvas fully inside its
panel. CSS transforms and partial ancestor clipping are prohibited app-shell
invariants for native ink. A future style/layout change that needs either must
first add a renderer-owned affine and clip seam; it must not extend this
arithmetic by guessing.

## Redraw and present boundary

Wake coalescing is unchanged:

```text
first accepted bounded-queue push
  -> LiveInkBridge::push returns Ok(true)
  -> payload-free EventLoopProxy marker
  -> one redraw and queue drain
```

Later samples return `Ok(false)` while that marker is pending. Transition
enqueue failures remain explicit. `WM_POINTER` observation is independent of
winit's `WindowEvent::Touch`; the proxy marker is the redraw guarantee.

No public post-present callback exists at this boundary. The custom-paint log
continues to say `first-custom-paint-submit`, never first present.

## Validation

The following completed without warnings:

```powershell
cargo fmt --all -- --check
cargo check -p nyatidraw-desktop
cargo clippy -p nyatidraw-desktop --all-targets -- -D warnings
cargo check --release -p nyatidraw-desktop
cargo clippy --release -p nyatidraw-desktop --all-targets -- -D warnings
cargo build --release -p nyatidraw-desktop
```

A bounded release smoke ran on Windows 11 build 26200 with an NVIDIA GeForce
RTX 3080 using Vulkan and `NAYATI_SYNTHETIC_INK=1`. The custom paint source
published `642x530@1`; after the first resolved paint, the independent layout
query published the matching complete viewport:

```text
canvas-viewport-complete revision=3 origin_x=19 origin_y=91
width=642 height=530 scale=1 admission=enabled
```

The explicitly labelled 49-dab synthetic GPU batch used zero input samples and
submitted at 881 ms; the custom-paint callback returned at 881 ms after process
entry. This is functional smoke evidence only, not pen latency, visual
correctness, or present timing. The exact smoke PID 24160 was force-stopped
after six seconds and its absence was confirmed. No GUI automation was used.

No physical pen was available, so actual `WM_POINTER` admission and a visible
stroke remain unverified. No post-present evidence was collected.

## Remaining limitations

- The calculation is deliberately bounded to this one untransformed,
  non-partially-clipped canvas in one window.
- The custom-paint callback still omits origin, transform, clip, and present
  completion; exact surface-size matching guards this shell but is not an
  upstream general solution.
- `RecordedStylusEvent` does not expose Windows pointer identity to the
  desktop. Preserving a stroke's end/cancel after it leaves the canvas still
  needs a pointer-identity-aware admission policy.
- Physical pen behavior, first visible ink, and end-to-end latency remain
  unmeasured.

## Sprint 3 viewport model (2026-09-01)

`nyatidraw-input` now provides a backend-neutral `ViewportTransform` with
explicit physical window origin/extent, DPI scale, revision, pan, zoom, and
rotation. It composes physical-window ↔ logical-canvas ↔ document mappings and
rejects empty surfaces, non-finite values, DPI outside `0.1..=16.0`, or zoom
outside `0.01..=100.0`. The Windows recorder freezes each active pointer to
its Begin viewport, so Move/End coordinates and revision remain stable across
resize/zoom; capture-loss Cancel likewise preserves that mapping. The
`InStrokeViewportPolicy` admission guard remains as a defense-in-depth check,
while transitions are preserved without sending samples through Dioxus state.

Scoped tests and checks passed for the input/platform crates and desktop crate;
the transform round-trip and revision policy invariant are covered by one
compact core test. Physical-pen behavior and DPI changes on real hardware
remain unverified.

The Windows canvas now publishes the affine actually used by its display
renderer into the same revisioned snapshot. `recorder_mapping` derives the
client-physical-to-document matrix from `ViewportTransform`; the display shader
derives the matching local-physical-to-document matrix. Semantic canvas changes
use a bounded metadata-only command lane, while raw samples remain confined to
the native input queue.

A real-device release readback passed combined pan, 2x zoom, and 90-degree
rotation quadrant sampling on RTX 3080/Vulkan. A bounded desktop synthetic smoke
published complete custom-paint/layout geometry after the affine pass. Dioxus
panel controls, physical pen, live resize/DPI interaction, and first-present
timing remain unverified.
