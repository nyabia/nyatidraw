# Browser drawing core

`nyatidraw-web-core` is a CPU-authoritative, platform-neutral browser MVP adapter.
It uses the existing brush evaluator (including the real 2H/2B dry-pencil models),
pressure mapping, smoothing, CPU dab rasterizer, linear premultiplied tiles and
layer compositor. It has no Dioxus, wgpu, filesystem, platform-input, redb, project
wire, Zstd, browser or WASM-bindgen dependencies. Native project persistence is
provided by the separate `nyatidraw-project-web` adapter.

## Host contract

Create `WebDocument::default()` for a transparent 1280×720 page, or
`WebDocument::new(CanvasSpec)`. There is one raster initially. `layers()` contains
the shared bottom-to-top `LayerTree`; `active_layer()` identifies the paint target.
This bounded MVP exposes flat raster layers, not user groups.

Feed document-space `StrokePoint { x, y, pressure, time_ms }` into
`begin_stroke`, `move_stroke`, and `end_stroke`. Feed coalesced pointer events in
order. The host converts viewport coordinates and supplies mouse pressure (1),
tablet pressure (0..1), pointer capture and pointer-cancel handling. Do not feed
viewport-relative coordinates or button-only temporary tools into brush settings.

Every successful paint update returns `CanvasUpdate`. Upload only `dirty_tiles`
from `snapshot()`; a dirty key absent from the snapshot means delete that GPU tile.
`structure_changed` requires rebuilding composition. The snapshot includes live
ink. `committed` means a completed non-empty artwork edit and is an appropriate
autosave trigger. Rendering can run without a UI-state update per input sample.

On `cancel_stroke`, upload its restoration keys. On a drawing `Err`, the complete
stroke has already been cancelled: resync the scene from `snapshot()` and show the
error. Partial input or resource exhaustion must not be silently accepted. A new
clean Begin is required. Tool switches/layer edits during a stroke are rejected.

`WebTool` has Pencil2H, Pencil2B, Pen, SoftBrush and Eraser. Each retains its own
`BrushSettings`; `set_foreground` and `set_background` use straight sRGB8. Captured
stroke color/settings are immutable. Tool preferences stay outside Undo.

Raster creation/deletion/rename/visibility/opacity/lock/alpha-lock/blend, page
dimensions, clear, Undo and Redo use `CanvasUpdate` too. Active-layer changes are
session state. Layer delete preserves at least one raster. `sample_rgba8` samples
composited artwork including outside-page pixels, never the checkerboard.

## Resource bounds

- Page: 1..4096 pixels per axis, 1..9600 PPI; outside artwork is retained.
- Input: finite document coordinates within ±32768, brush diameter 0.1..200 px.
- Current artwork: 1024 nonempty 128×128 RGBA8 tiles (64 MiB pixel allocation).
- Layers: 32 rasters, names up to 128 characters/512 UTF-8 bytes.
- Undo/Redo: at most 128 operations, additionally pruned to a conservative
  128 MiB current-plus-history pixel/metadata budget. New edits discard linear
  redo, and no-op strokes do not consume Undo. Branch UI is deferred.
- A cancellable live stroke may additionally hold at most 64 MiB replacement
  pixels; one event stages at most 128 tiles (8 MiB). Export/import buffers are
  separate, bounded by the 4096² page. These are allocation bounds, not measured
  browser peak RAM or input-latency guarantees.
- One update allows at most 2048 dabs and bounded raster work. A stroke allows
  65536 samples/262144 dabs. Exceeding a bound cancels atomically.

## Recovery / native project adapter

New browser saves and downloads use the same `.ntdr` container and record codecs
as desktop through `nyatidraw-project-web`. Hosts must use that adapter, not the
legacy portable encoder, for recovery and downloads. Unsupported native content
is rejected, not flattened. See [ADR-0061](../../docs/decisions/ADR-0061-shared-ntdr-browser-storage.md).

### Legacy browser format

The retained legacy `encode_portable` returns browser-specific bytes with MIME
`application/vnd.nyatidraw.web-document`. The host chooses a clearly web-only
download extension, not `.ntdr`. This is no longer the host save path.
`decode_portable` constructs a separate document;
replace the current one only after successful decoding. IndexedDB stores these
bytes transactionally; failed recovery must preserve the old byte record.

Version 1 starts with `NYWEB001`, uses explicit little-endian bounded fields,
stores raw or lossless RGBA-pixel runs per tile, and ends in BLAKE3 over its entire
payload. It preserves canonical linear premultiplied tile bytes, signed positions,
flat-layer properties/order, page, active layer, all five tool settings and both
colors. There is no Rust-layout serialization or unbounded claimed output length.
Input and encoded-output growth are capped at 68 MiB; output reservation does not
geometrically jump to 128 MiB at the largest raw-tile document. Unknown version, trailing bytes, duplicate
keys, invalid alpha, oversized counts and malformed runs are rejected. Checksums
detect accidental corruption, not malicious authenticity.

RAM history and unfinished strokes are intentionally not serialized. Save at
stroke End/committed edits and after idle tool preference changes, not mid-stroke.

`import_rgba8(width,height,bytes)` creates a document from tightly packed straight
sRGB8; `export_rgba8()` returns the finite page in the same layout, without
checkerboard. PNG decoding/encoding belongs to the host and must validate encoded
size and decoded dimensions before allocating. RGBA8 linear conversion can
quantize low-alpha/low-light sRGB input; this is not a lossless PNG round trip.
Portable bytes preserve artwork exactly and are the recovery authority.

## Evidence

Three focused tests cover deterministic material brushes, cancellation/no-op/Undo
integrity, portable pixel/layer/preferences round-trip, malicious bounded records,
and the linear history cap. Compilation/native/WASM test results are recorded by
the coordinating host task; physical pen and browser first-pixel latency are not
proved by these tests.
