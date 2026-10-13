# Shape correction safety audit

Scope: W2 selected-raster shape correction in the Windows local candidate.
Existing selection, clipboard, transform-session and raw-pixel boundaries were
inspected. No user artwork, publishing, installation, or source-upload operation
was performed.

## Reproduced defect and correction

The affine transform rejected empty transformed selection coverage, but coverage
can survive after every selected artwork pixel disappears. A signed selection at
`[-4, -1]` with size `[5, 1]`, containing one `[128, 0, 0, 128]` pixel at its left
edge, lost all its ink at `scale_ppm = [200000, 1000000]`: the bilinear samples
only reached transparent selected pixels. The original source was cut and the
remaining nonempty selection allowed that result to be accepted.

`crates/paint-cpu/src/affine.rs` now records whether the selected source and the
resampled output contain nonzero alpha. A result that erases all selected ink is
rejected with `InvalidTransform` before any replacement snapshot is published.
The same check catches an alpha-1 pixel translated by half a pixel on both axes,
where each output alpha rounds to zero. Legitimate transforms of fully
transparent selections remain valid. Existing immutable-source sampling, raw
layer pixels, source-over composition and resource bounds remain in use.

## Verification

- The signed five-pixel minification regression failed before the correction.
- `cargo test -p nyatidraw-paint-cpu --locked`: 37 passed, 0 failed.
- The final table-driven regression passed for both minification and alpha
  rounding, and checked that transparent selections still transform without
  changing unrelated artwork.
- Existing passing tests cover exact identity, signed off-page movement, mask
  holes, unrelated layers, premultiplied edge interpolation, repeatable previews,
  bounded failure, selection morphology and clipboard cut/paste atomicity.
- `cargo clippy -p nyatidraw-paint-cpu --all-targets --locked -- -D warnings`:
  passed with host access.
- `cargo test -p nyatidraw-desktop --locked
  transform_worker::tests::provisional_paste_cancels_exactly_and_only_current_preview_commits
  -- --exact --nocapture`: parent test passed and its fresh-process child test
  passed. The scratch-project flow checked provisional paste cancellation,
  failed/stale preview rejection, retry/commit, retained selection, exact later
  cancellation, one-step Undo/Redo, the saved tile root and decoded PNG pixel
  equality after process restart. The test removed its scratch project and PNG.

The shared editor maps a failed affine preview to `TransformError::Rejected`;
the desktop worker preserves that classification as `EditFailure::Rejected`.
It disables commit while retaining the original source and last displayed
preview, allowing a new preview or exact cancellation. This propagation was
checked in the implementation; the new guard does not enter the fatal
persistence-failure path.

These are CPU tests in the unoptimized test profile, not latency benchmarks or
physical-pen evidence. Sandbox configuration discovery initially blocked
formatting and Clippy; both succeeded with normal host access.

## Remaining acceptance boundaries

This guard prevents complete ink loss; bilinear minification can still remove
small individual details while other selected ink survives. No area filter or
fractional selection coverage was added.

This audit establishes the existing desktop transform worker's automated
save/process-restart/reopen flow. It does not establish native-window input or
physical-pen acceptance, or acceptance of the complete drawing workflow.
