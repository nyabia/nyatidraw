# ADR-0007: Validated editor trees and coordinate-scoped composites

- Status: **Accepted for the Sprint 3 core boundary**
- Date: 2026-09-01

## Context

The flat `Vec<LayerNode>` could not represent group ownership and did not state
which cached composite tiles become stale after a raster edit. Invalidating an
entire document for every dab would make a 20-layer canvas scale with unrelated
tiles and groups. The shell also lacked a backend-neutral session layout or an
explicit state contract between the editor and Dioxus.

## Decision

`nyatidraw-document` owns a validated `LayerTree`. Its implicit document-root
group contains raster nodes and nested groups in bottom-to-top compositing
order. Tagged raster/group IDs are globally unique. Raster nodes own immutable
content roots; groups do not. Root visibility and opacity are fixed, and
reorder validates the destination, post-removal index, and group cycles before
detaching a node.

Group composite cache values are defined as the composite of a group's children
before that group's visibility and opacity are applied by its parent. Therefore:

- changing one raster tile produces exact `(ancestor group, tile coordinate)`
  invalidations from its immediate parent through the document root;
- sibling groups and every other coordinate remain valid;
- visibility and opacity invalidate every cached coordinate only for ancestors
  of the changed node, not the changed group's reusable child composite; and
- reorder invalidates the union of all old and new ancestors.

`nyatidraw-tiles::CompositeCache<Value>` is only an index. Render backends may
store opaque handles as values, but document logic emits backend-neutral keys.
Whole-group invalidation enumerates only entries already resident in the cache.

`nyatidraw-api` owns the backend-neutral `DockTree`, `UiProjection`, and typed
command/event envelopes. A valid dock tree contains canvas, layers, brush,
color, and history exactly once, with bounded depth, valid tab selection, and
non-degenerate split ratios. Decode failure or validation failure replaces the
whole candidate with one complete safe default and emits a recovery status.
It never partially preserves corrupt session geometry.

Every semantic projection publish uses a checked monotonic revision. Commands
carry the revision on which they were based so the editor can reject stale UI
actions explicitly. The API crate has no input dependency, and no command can
carry a `StylusSample`, tile pixels, or GPU handles. Native samples continue to
bypass Dioxus and use the bounded hot-path queue.

## Evidence

One 20-layer invariant fixture addresses the bottom-right tile of a 3840x2160
canvas. A raster edit removes exactly the changed layer's parent and root cache
entries at that coordinate while retaining the sibling group and other
coordinates. The same test covers structural visibility invalidation,
old/new-parent reorder invalidation, and failure-atomic cycle rejection. A
second compact test proves corrupt decoded layouts and decode failures both
recover to the complete safe default.

Focused check, tests, Clippy with warnings denied, and desktop compilation pass.
No dependency was added. No UI/framework test was added.

## Consequences and open boundaries

The current cache is an invalidation/index contract, not a GPU blending
implementation. Blend modes beyond normal raster composition, GPU timing,
zoom-out mip generation, and renderer cache population remain open.

The dock model does not include floating multi-window docks or workspace sync.
The desktop uses the typed command/event mailbox. UI callbacks obtain the
authoritative revision at execution time, older projections cannot overwrite a
newer mailbox, and the root-owned WGPU source survives dock remount. Explicit
dock controls and a post-dock viewport command passed actual Dioxus Native
acceptance. Pointer drag, programmatic focus, live DPI/resize, and physical-pen
sample acceptance during panel rerender remain unverified.

LayerTree v1 records are durable and bounded (4096 nodes, depth 64, 1024-byte
names, 5 MiB record). Visibility, opacity and cross-parent reorder survived
save/process-restart/reopen exactly. Checksum, unknown-tag and trailing-payload
corruption are rejected without rewriting the project file.

## Sprint 3 GPU cache consumer (2026-09-01)

`GpuCompositeScene` now consumes this ADR's `CompositeInvalidation` and
`CompositeCache<()>` contract. Each raster and group/root has a renderer-owned
document texture. A cache miss rebuilds only the requested 128x128 coordinate,
recursively ensuring child-group pixels before applying group visibility and
opacity at the parent. Children retain bottom-to-top `LayerTree` order and use
premultiplied source-over; advanced blend modes remain excluded.

The real GPU release probe populated four coordinates for a raster nested in a
group. Replacing one CPU tile rebuilt exactly the nested-group/root pair at that
coordinate while retaining unrelated cache entries. Visibility and reorder
rebuilt four resident root coordinates without rebuilding reusable child-group
pixels. RTX 3080/Vulkan readback matched expected opacity, visibility, order,
and affine colors within two RGBA8 channel bytes.

This first consumer uses bounded full-document textures rather than a sparse
atlas. It rejects persistent raster/group surfaces above 512 MiB before
allocation; an 8K/20-raster fixture is rejected at 5,637,144,576 required bytes.
This is functional evidence, not a 20-layer timing result. The desktop has the
typed dispatcher, durable LayerTree reopen, root-owned WGPU source, and actual
explicit-control acceptance through revision r8. Physical pen and measured
present latency remain open.
