# Raster layer masks

The illustration mask stores a separate typed R8 coverage plane owned by one raster layer. A missing tile uses the mask’s explicit default (255 for Reveal All; 0 for From Selection). Original RGBA tiles remain untouched by mask creation, painting, disabling or removal. The layer image thumbnail continues to describe original image pixels; the adjacent small grayscale mask preview identifies the active editing plane.

Black hides, white reveals, intermediate foreground colors use the displayed sRGB gray value, and the eraser reveals. The brush, selection coverage, owner and mask identity are captured at stroke Begin. The owner’s edit lock protects both planes; alpha lock protects image pixels only. Mask strokes use the shared deterministic CPU dab rasterizer, with document-aligned pencil grain, and upload disposable R8 coverage to the GPU. A completed gesture adds one history step; cancel restores both planes. Selected masks accept brush and selection/picker tools. Image-only edits explicitly require selecting the image target first.

Coverage multiplies premultiplied RGBA after the layer’s nondestructive color effect and before opacity, blend and clipping-stack composition. CPU export, active/reference/all-visible sampling, navigator, onion proxies and the GPU display consume enabled coverage. Disabling a mask restores the original composition without deleting coverage. Copying source pixels from the explicit image target retains its existing raw-image semantics; whole-page clipboard/export uses the composed result.

Undo/Redo retains the mask editing target when the same selected layer still owns
that exact mask ID. Removing the mask or changing the selected layer invalidates
the target; later restoring the old mask does not silently select it again. This
prevents an ordinary mask undo from redirecting the next stroke into source RGBA.

The existing **Fill Selection** action also fills the selected mask. It requires
an explicit finite selection and an unlocked, visible owner; it never fills the
mask’s infinite default region. Foreground display-sRGB gray uses the same scalar
source-over rule as mask painting, including foreground alpha and fractional
selection coverage. The operation changes only that owner's selected R8 values,
preserves mask defaults/enabled state, and commits one undo step through the
existing native/web mask transaction. Bucket fill and clipboard operations still
require the image target. Its subsequent actual web save/restart/reopen check
passed; the incremental evidence is recorded below, separately from the earlier
mask acceptance.

Image and group transforms keep attached coverage linked with the same pivot and immutable source. Without an explicit selection they include nondefault coverage beyond image bounds. Explicit selections transform only their coverage; vacated coverage returns to the mask default. Layer/cel duplication assigns fresh mask identities and shares immutable source objects until edited. Deleting an owner removes its coverage objects from the resulting snapshot. Cropping/rebasing moves both planes uniformly and retains signed off-page artwork.

Both planes participate in the document content root, structural history and native NTDR save/recovery. The older image-only NYWEB portable format rejects masked documents. The normal browser save/open and NYWORK07 recovery paths use the typed mask-aware codecs. Default-only masks are durable metadata even when no R8 tile exists.

Limits remain explicit: browser admission counts combined RGBA and R8 tiles; edit scratch uses bounded aggregate allocation; GPU coverage residency is capped at 128 MiB in addition to the existing bounded image scene. This implementation does not add group masks, unlinking, independent mask transforms or mask effects. It must not silently flatten these features into original images.

## Verification

The [actual browser acceptance](mask-web-acceptance.md) passed mask editing,
soft selection, save/restart/reopen and original-pixel preservation. Native actor
artwork/recovery checks also passed, but the Windows window capture can lose the
visible mask hole after UI redraw despite correct sampled GPU output.
[Native presentation investigation](research/native-mask-presentation.md) remains
open; masks are not yet accepted as a complete stable native visual workflow.

Core artwork invariants cover mask stroke cancellation, captured targets, exact original-image preservation, undo/redo, fractional selection-to-mask, disable/remove, duplication and linked affine changes including mask-only/off-image coverage. The `gpu_shading` example compares exact raw CPU/GPU composition for defaults 0/255, explicit zero/gray, signed tiles, clipping, group opacity, all supported blends, cache updates, toggle and removal. Runtime and fresh-process results are recorded separately by the integration run; source/test coverage alone is not physical-pen or visible-latency evidence.

Mask painting color semantics were corrected during browser acceptance: the captured linear-premultiplied brush color is unpremultiplied and encoded to display sRGB before computing weighted gray, then premultiplied by its original alpha for scalar coverage painting. Thus an opaque UI gray of 128 paints approximately 128/255 coverage, rather than linear-light 55/255. Black/white, brush opacity/flow, selection coverage and eraser-to-white are preserved. Existing mask objects remain stored coverage; they are not reinterpreted or rewritten. Very low-alpha brush colors retain the existing byte-quantization limit.

Native and web mask strokes now admit cumulative dab work against the same limit
used by closed CPU replay. A batch that would exceed that limit is rejected
before preview adoption; the entire current gesture rolls back with a recoverable
message, and the next clean Begin can draw again. This prevents long strokes from
looking accepted in incremental preview and then failing only during native
save. Begin, End flush and close-sealing use the same admission counter.


`MaskStrokeBudget` is shared by native and web hosts and counts cumulative
128×128 tile visits across batches. With the default work budget this permits
65,536 single-tile visits; a dab spanning several tiles consumes several visits.
This is an admission bound, not a latency or stroke-duration guarantee. Before
this correction, individually accepted preview batches could exceed the total
closed-replay limit. Exceeding it now cancels the whole current mask stroke,
restores its original image/coverage state, adds no history entry and leaves
prior history usable. Late End/close flush must not enqueue the rejected stroke
or turn this recoverable rejection into a global workspace failure. Admission
resumes only at the next clean Begin.

The focused regression tests for this final budget correction **passed**, as
recorded in `target/mask-budget-tests.log`: paint-cpu
`accepted_mask_batches_fit_replay_and_rejection_is_atomic`, web-core
`mask_work_rejection_restores_original_and_keeps_history_usable`, and desktop
`mask_work_limit_cancels_before_durability_without_poisoning_the_workspace`.
The log independently records all three tests passing; it does not establish
native visual acceptance or a new browser runtime run.


## Compact target previews

Image and mask targets now appear side by side in the existing layer row. The
image thumbnail still shows original RGBA. The mask thumbnail samples the actual
stored R8 plane and explicit default, even while disabled; an outline identifies
the selected target and a diagonal mark identifies a disabled mask. Its tooltip
states white/reveal, black/hide and the finite-page preview boundary. If a preview
is unavailable, a labeled neutral placeholder is used rather than fabricated
white mask pixels.

The shared disposable preview is at most 50×56 pixels, uses the same page-centered
sample grid as the image thumbnail, and never flattens the full document or reads
GPU state. It encodes white premultiplied coverage to PNG and displays it on black,
so scalar coverage is shown as gray without applying an extra transfer function
to mask values. The native worker refreshes the owner after mask painting and
metadata changes; existing history, transform and cel updates carry the same
mask preview. Equal native cache frames retain their existing Arc; web regeneration
remains behind the existing preview generation check. A typed MaskId check prevents
a removed/replaced mask from briefly showing another mask's cached preview.

Actual web inspection caught the first mask thumbnail's intrinsic image height
overflowing its button. The corrected layout fits the image inside a measured
30×30 region, with a separate label beneath it; black is applied only behind the
PNG image itself, so surrounding letterbox space cannot masquerade as zero
coverage. The all-white mask displayed without overflow in
`output/playwright/masks/followup-mask-thumbnail-fixed.png`. The main image
thumbnail remains 50×56. New keyboard-only target-switch acceptance is not
claimed by this layout check.

The actual selection-fill operation was saved as `followup-filled.ntdr`, the
browser was closed and restarted, and the same file was opened through the file
picker. Both exported 512×512 PNGs (`followup-filled.png` and
`followup-file-reopened-export.png`) have raw RGBA SHA-256
`76cfb2f1509bbd2d26bbd7a8e05bf36b6ccb6b046c5a3c8cfaedc6c13fe1bab2`.
Undo's `followup-undo.png` matches the pre-fill output exactly:
`3411bff5938e9966ad702c6d383a31d415b25209f84c8442c35ed8f783c54f89`.
Artifacts are under ignored `output/playwright/masks/`. This is actual browser
save/restart/reopen evidence, not a native visual or physical-pen result.

Actual timeline images now use a separate bounded, generation-checked cache;
see [frame preview scheduling and incremental acceptance](frame-previews.md).
None of these refinements resolves the native canvas presentation issue or
implies installation, push or deployment.
