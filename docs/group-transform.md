# Group artwork transformation

Local implementation, 2026-10-28. Group settings expose **그룹 변형** for moving,
rotating, flipping and scaling a subject assembled from multiple raster layers.
The existing free-transform handles, numeric panel, confirm and cancel apply.
This adds one selected-group operation, not arbitrary layer multi-selection.

## Artwork rules

- Every raster descendant participates, including invisible layers and layers
  below invisible nested groups. Any pixel-locked or alpha-locked descendant
  rejects the whole operation. Unlocking just the active drawing layer is not
  sufficient. Empty groups and the implicit document root cannot transform.
- An existing selection applies identically to each descendant. Without a
  selection, the union of occupied descendant pixels supplies a rectangular
  selection, including signed off-page artwork. All layers share this mask,
  its pivot, and one affine mapping; they do not rotate about individual bounds.
- Every preview samples the same immutable original snapshot. Targets remain
  separate rasters; clipping flags, group nesting, blend modes, opacity, color
  effects and visibility retain their original values. No display effect is
  flattened into raw pixels. Non-target layers and unselected pixels remain.
- The active drawing raster is retained even if it lies outside the group.
  Confirm publishes one structural artwork edit. One Undo restores all targets;
  Cancel restores the exact original artwork and optional selection. A failed
  or stale preview cannot commit an earlier candidate accidentally.

This inherits the existing bilinear transform's minification limit: a target
whose selected ink would disappear entirely causes rejection of the whole group
operation. It is not nearest-neighbor pixel-art scaling or an area resampler.

## Bounded computation

`AffineDraft` pins one original tree, tile snapshot and source mask for the
entire group. The existing previous-candidate budget remains in effect.
Before transforming each raster, accumulated replacement payload, final tile
object headroom and the shared transformed mask are subtracted from the working
budget. Full snapshot metadata is counted for the final merged root. Pixel and
polygon work limits are divided across the target count, so a large group cannot
multiply a single-raster budget unchecked. The default workspace limit remains
256 MiB; original immutable artwork is not copied into a flattened group image.
Conservative per-target quotas may reject a large sparse group sooner than a
future geometry-sharing implementation would.

The source-bound scan is also divided across target count. A widely separated
group whose rectangular mask exceeds the existing selection limits is rejected
before constructing that mask. The operation does not silently crop to the page.

## Verification boundary

Added focused invariants cover a common rotation pivot, repeated previews from
originals, whole-candidate failure after a per-layer budget would otherwise pass,
hidden locked/alpha-locked rejection, outside-art preservation, selection-only
group edits, cancellation, exact structure/effect retention and single Undo/Redo.
A native NTDR subprocess probe checks committed raw content hashes, layer bytes,
RGBA export and the original Undo target after process restart.

Integration commands (results must be recorded by the integration owner):

```powershell
cargo test -p nyatidraw-paint-cpu group_affine_keeps_one_pivot_and_rejects_partial_or_over_budget_candidates
cargo test -p nyatidraw-web-core group_transform_preserves_hidden_children_structure_cancel_and_single_undo
cargo test -p nyatidraw-project-web group_transform_commit_survives_native_process_restart_with_exact_raw_tiles
```

Source additions are not runtime evidence. Compilation, Clippy, existing single
raster/paste transform regressions, native GPU preview adoption and browser
group-button/handle interaction must be checked during integration. Physical pen,
installed-app, first-visible-pixel and other platform/backend acceptance remain
separate and unverified by these tests.
