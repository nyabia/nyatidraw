# Raster mask storage and reader compatibility

Implemented foundation, 2026-10-28. This records the persistent contract; it does
not by itself claim completed brush/GPU/UI acceptance. See
[the mask workflow design](layer-mask-design.md) for the required consumer gates.

`LayerNode.mask` optionally owns `LayerMask { id: MaskId, enabled,
default_coverage }`. Mask identities are nonzero and unique across the full
authoritative layer tree, including inactive animation cels. Missing coverage
tiles mean the owner's explicit default, so Reveal All needs no tile allocation.
An all-zero R8 tile is meaningful and remains stored. Mask removal deletes its
typed coverage plane, leaving the source RGBA plane unchanged.

`TileSnapshot` owns both maps: the existing RGBA `TileKey` and a separate typed
`CoverageTileKey { mask, mip, x, y }` with immutable 128×128 R8 objects. Signed
coordinates use Euclidean addressing; only base-mip coverage is accepted.
`iter/len` retain their RGBA meaning; `coverage_iter/coverage_len/total_len` make
plane handling explicit. RGBA `with_replacements` preserves every coverage tile.
`from_planes` reconstructs both maps; `without_masks` is explicit removal.
Consumers rebuilding snapshots must retain or intentionally remap both planes.

Image-only roots and their original wire bytes remain unchanged. Roots with R8
entries append a `NYMASK01` extension and hash the complete ordered payload with
`nyatidraw-content-root-planes-v2`. R8 objects use their own hash domain and
`RecordKind::CoverageTile` tag 11. Their exact decoded length is checked before
allocation. RGBA and R8 records cannot be substituted for one another.

Layer-tree wire version 5 carries mask ownership, enabled state and default
coverage. Unmasked documents keep version 3/4. The sticky `MASK_SCHEMA_FLAG`
(`0x4_0000`) prevents older readers opening a maskless approximation, even after
Undo or removal returns the current tree to an unmasked state. Native structural
and stroke commits validate ownership against the applicable complete tree.
Every retained snapshot is checked on open; orphan planes and missing objects,
wrong kinds, malformed metadata and missing feature flags fail closed.

Existing immediate transactions publish root, tree, timeline, canvas and history
together. Coverage objects join the existing unique-root reference index, so
retention pins masks through current, rebased initial and retained Undo/Redo
roots. Same-pixel metadata edits still receive separate snapshot identities.
Migration copies both object kinds and hashes their decoded records while
preserving the original source file.

Current web Save continues to use native NTDR with both planes. Recovery version
`NYWORK07` carries complete coverage keys and full/delta R8 data; older recovery
packets cannot replace masked content. The combined RGBA+R8 key budget remains
1024 for browser import/recovery. Legacy portable `NYWEB001/002` export rejects
masks explicitly; this restriction does not apply to current NTDR Save.

Fractional selection is separate transient coverage. Semantic strokes use
`NYSEL003` with signed origin, dimensions and exact coverage bytes, guarded by
sticky `FRACTIONAL_SELECTION_SCHEMA_FLAG` (`0x8_0000`). Binary selections retain
the existing `NYSEL001/002` encodings. Decoding re-seals the stroke and checks its
identity; fractional selection never silently becomes Boolean membership.

Focused source tests cover signed/default coverage, unchanged RGBA, typed root
and object corruption, 130 revisions with inactive animation cels and retained
Undo/Redo, migration source preservation, fresh-process reopen, recovery full and
delta packets, old protocol rejection, and fractional stroke wire/replay. The
integration owner records execution results and actual native/web interaction.

## Native integration and runtime evidence

The Windows host now captures the selected image or exact attached mask at
stroke Begin. Mask dabs use the shared deterministic CPU coverage painter for
both live display and durable replay. The native input queue, discontinuity
quarantine, FIFO writer and close drain remain the existing stroke path. A
sealed mask stroke is one structural history commit; image pixels and inactive
cels remain in the full authoritative snapshot. Metadata changes, mask removal,
and Undo use the same atomic tree/plane commit and history adoption.

Image-only edits reject a selected mask before mutation. Selecting another
layer, switching frames, or restoring history returns to the image target.
Layer/cel duplication remaps coverage identities; transform preview and commit,
PNG clipboard capture, reopening, playback and onion display install coverage
alongside image pixels. Saved native NTDR and ordinary/scaled/sprite PNG exports
continue to use the worker's full immutable snapshot.

`NAYATI_MASK_PROBE=write` requires a new `mask-scratch.ntdr` in a directory with
a `.nyatidraw-scratch-mask-probe` marker. The native actor constructs two colored
cels, attaches and paints a mask through synthetic native input, disables and
enables it, removes it, undoes removal, and saves. It writes `mask-scratch.mask-proof`
only after PNG export succeeds and source RGBA/inactive-cel checks pass. Running
`NAYATI_MASK_PROBE=reopen` in a separate process verifies the exact stored planes
and metadata against that proof. The DX desktop debug build, scratch write,
normal close, and fresh-process reopen passed. The web editor also restored
default-white and default-black masks after browser restart and NTDR import;
a web feather-mask NTDR exported through the Windows CLI produced identical
RGBA pixels without changing the source file. See
[workflow validation](workflow-expansion-validation.md) and
[browser acceptance](mask-web-acceptance.md) for artifacts and boundaries.
Native window presentation is tracked separately from these persistence checks.
Synthetic input is not physical-pen, latency, or hardware acceptance.
