# Persistence completion audit

Scope: local Windows candidate, shared native/browser `.ntdr`, retained tool
settings, layer-color metadata, base snapshots, and bounded history. All files
used by acceptance are synthetic scratch projects. No installation, publication,
or user artwork changes are part of this audit.

## Corrected save failure

Before this change, opening a project with an unknown optional tool-state record
worked, but changing a color or tool made `WebProject::encode_ntdr` reject the
whole save. The recovery Worker calls the same operation, so a subsequent painted
stroke could not be saved or recovered either. An oversized optional record also
propagated its preference-read error into project opening.

The browser now treats unreadable tool settings as an isolated optional-state
failure: retain the original record, use temporary session settings, show an
import notice, and continue saving artwork. It does not replace the unreadable
record with browser defaults. Recovery carries the same write-block state.
Normal supported tool-state persistence remains unchanged.

The regression paints new pixels after changing the color/tool of a project with
unknown settings. Both direct `.ntdr` save and recovery Worker staging must
preserve the new pixels and the exact original optional bytes, and reopen with
the notice. This extends the existing unknown-record invariant rather than
adding UI mock tests.

## Durability coverage

The existing 132-edit retention acceptance now alternates enabled and disabled
layer-color configurations, with distinct endpoints per snapshot. Its fresh
process checks all 128 retained Undo states, restores Redo, and checks opaque
tool settings and active-layer selection. The default-feature variant also
exercises the legacy container conversion and storage rebuild before reopen.

A base-only regression starts with signed-coordinate raster pixels and disabled
color configuration in a durable initial snapshot with zero history nodes. It
reopens, commits a metadata edit, persists Undo, and reopens again to compare
pixels, snapshot ID, full layer metadata, and available Redo.

Save As was inspected: it drains/closes the source in the caller, copies the
whole container, validates the copy, and composes its PNG using copied layer
metadata. Optional state and immutable history are retained by the container
copy. This audit does not claim fresh desktop-window Save As acceptance.

## Results

- `cargo test -p nyatidraw-project-web --lib`: 12 passed. Includes fresh-process
  native/browser reopen, unsupported optional settings with continued artwork
  save, recovery failures, and repeated saves beyond 128 edits.
- `cargo test -p nyatidraw-project-redb retention_preserves_artwork_metadata_and_128_undo_steps_across_restart -- --exact tests::retention_preserves_artwork_metadata_and_128_undo_steps_across_restart --nocapture`:
  passed, including the child process and legacy conversion/rebuild (4.74 s).
  The `--no-default-features` current-container variant also passed (2.67 s).
- `cargo test -p nyatidraw-project-redb base_only_artwork_remains_editable_and_undo_restores_its_pixels_and_effect -- --exact tests::base_only_artwork_remains_editable_and_undo_restores_its_pixels_and_effect`:
  passed after correcting the new fixture's signed tile construction.
- `cargo clippy -p nyatidraw-project-web -p nyatidraw-project-redb --all-targets -- -D warnings`:
  passed.

The first default-feature retention run was blocked by sandbox `PermissionDenied`
while renaming its own temporary legacy fixture, before fresh-process reopen.
Clippy similarly could not read its configuration under the sandbox. Both exact
checks passed with filesystem access permitted. Incremental-cache hard-link
fallback warnings appeared during compilation; these are not artwork assertions.

These are core and process-restart checks, not physical-pen, installed-app,
desktop-window, browser-control, or cross-platform acceptance. Named brush
libraries remain app-owned files/localStorage, outside `.ntdr`; their UI and
global writer are outside this persistence lane.

## Core drawing follow-up: image import and group copy

`PNG를 레이어로 가져오기` now decodes a PNG into a separate provisional raster,
centered on the existing page without resizing it. Native decoding runs off the
UI thread; both file hosts bound reads to 72 MiB, and image pixels remain bounded
by the existing editing limits (web also keeps its 4096-edge limit). A document
epoch guard rejects a picker result after replacing the document. Native command
admission allows only one queued image payload. No input image is written.

Import reuses the existing preview/transform/confirm/cancel transaction without
reading or writing the clipboard. Unconfirmed pixels and the provisional layer
do not enter autosave; confirmation is one Undo step. Resource or decoding
failure leaves committed artwork unchanged. This is an artwork import, not a
non-exported reference picture or a new document.

The shared layer panel can duplicate an entire selected group. New IDs retain
all descendant order, raw signed/off-page pixels, visibility, lock, clipping,
opacity, blend and enabled/disabled color-effect metadata in one transaction.
The source remains independently editable. No format or dependency was added.

Review found that inserting directly above a clipping base could rebind its
existing clipped siblings. Import/paste and raster/group duplication now insert
above the existing contiguous clipping run, preserving the old binding.

New core invariants are
`image_import_preserves_page_clipboard_and_cancel_then_commits_one_undo`,
`inserting_import_or_duplicate_cannot_rebind_existing_clipped_artwork`, and
`imported_image_and_group_copy_preserve_sources_through_undo_and_fresh_reopen`.
The last test writes a synthetic shared `.ntdr` and checks native/browser readers
in a fresh child process. Central integration runs own the final test/build and
browser results; adding these tests alone is not a passing result. Native file
picker/window interaction and real illustration/physical-pen acceptance remain
separate from these core invariants.

The final whole-workspace test run passed, including all three named invariants.
The release browser then exercised import/cancel/placement/Undo, group drag/drop
and duplication, and separate-process PNG-equal reopen. Strict native/WASM
Clippy and both release builds passed. The final evidence and limitations are
recorded in [core workflow completion](../core-workflow-completion.md#통합-검증-결과).
