# Raster masks and fractional selection: implementation contract

The working tree now implements a persistent raster mask and a separate transient fractional selection. This replaces the initial design-only gap assessment. It describes the source contract; native/web visible use and fresh-process acceptance remain separate gates recorded in [workflow expansion validation](workflow-expansion-validation.md). See [layer mask behavior](layer-masks.md) for the user-facing scope.

## Ownership and preservation

One optional `LayerMask { id: MaskId, enabled, default_coverage }` belongs to a raster. `TileSnapshot` owns separate RGBA and R8 maps through typed `TileKey` and `CoverageTileKey`; masks are never hidden image layers. An absent R8 tile means the explicit default: Reveal All is 255, From Selection is 0. Signed off-page coverage is retained. Creating, painting, disabling and removing a mask do not replace source RGBA pixels.

The layer row exposes an explicit image/mask target. The original-image thumbnail remains an image thumbnail; the labeled mask control does not pretend to be a generated mask thumbnail. Black hides, white reveals, gray uses display-space sRGB luminance and eraser reveals. The owner edit lock protects both planes; alpha lock protects image alpha only. Target, mask identity, brush and selection are fixed at Begin. Cancel restores the original snapshot; a completed mask stroke is one structural history transaction. Image-only tools reject a selected mask before mutation.

## Durable representation

Mask IDs and R8 objects are typed throughout domain, snapshots, storage and recovery. Layer-tree wire v5 stores attached mask metadata; coverage manifests use the tagged mask extension. Existing image-only roots keep their v1 hash contract. A snapshot with R8 objects uses the domain-separated combined root. A default-only mask remains durable tree metadata even when no R8 object exists.

The current snapshot, tree ownership, timeline and history cursor are published atomically. Retained history keeps both planes reachable. Layer/cel duplication assigns fresh mask IDs and shares immutable R8 objects until editing; deletion removes the deleted owner’s coverage from the current snapshot without erasing prior history. Native `.ntdr`, browser native-format save/open and NYWORK07 full/delta recovery retain coverage. The legacy image-only NYWEB writer explicitly rejects masked documents. Unknown/corrupt formats fail closed.

## Consumer agreement

| Area | Implemented contract |
|---|---|
| CPU/GPU composition | Apply the raster color effect, multiply all four premultiplied channels by coverage with `(channel * coverage + 127) / 255`, then apply opacity/blend/clipping. The clipping base includes its mask. Sparse signed tiles, isolated groups and Solo keep their existing semantics. |
| Brush | Shared CPU scalar dab evaluation drives R8 preview and durable mask materialization. Document-aligned pencil grain and fractional selection apply per dab. GPU R8 coverage is disposable; it is never the only durable source. |
| Transform | Linked image/group affine and integer move/flip/rotate/resize use immutable originals and one commit. Explicit selection limits the changed area; whole-layer edits include nondefault coverage beyond image bounds with the image pivot. Cropping/rebasing moves both planes. Empty-image mask-only changes are detected by snapshot root, not image dirty keys. |
| Sampling/output | Active/reference/all-visible picker and wand, composed navigator, PNG/scaled PNG, page clipboard, onion and sprite-sheet composition apply enabled masks. Explicit image copy and image thumbnails retain raw-source semantics. |
| Hosts/history | Native and web retain authoritative masks for all cels; the current GPU scene contains the display projection. Begin capture, idle admission and stale generation checks prevent target changes mid-stroke. Undo/redo, preview cancellation and reopen restore both planes. |
| Bounds | Browser residency counts combined image and mask keys and deduplicated bytes. Edit scratch accounts for source/candidate R8 allocations. GPU mask residency has a separate 128 MiB cap in addition to the bounded image scene. Limits reject the edit rather than adopting a partial result. |

## Fractional selection and feather

`SelectionMask` now stores immutable signed 0–255 coverage, with explicit binary legacy conversion. Brush/stroke replay, GPU selection, fill/gradient, copy/cut/delete, color adjustment and integer/affine transforms consume fractional coverage. A stationary transformed sample restores its immutable original exactly instead of compositing its own soft split twice. Selection combination, inversion and morphology retain coverage instead of silently converting nonzero values to 1.

Feather uses three separable box passes approximating a Gaussian, with signed padding and a bounded radius up to 64 document pixels. The shared selection controls expose this operation. Feather changes the transient selection; creating a layer mask from that selection stores its fractional coverage. Stroke capture and persisted replay retain the coverage used at Begin. GPU UNORM precision comparisons must distinguish fractional-selection changes from existing unmasked pencil quantization; the integration record carries measured tolerances.

## Remaining limits and acceptance gates

Group masks, independent/unlinked mask transforms, mask application/baking, filter masks, generated mask thumbnails and appearance-preserving merge are outside this slice. Unsupported mask-target tools reject explicitly; they never edit image pixels as a fallback. This is not a claim of complete CSP/Krita/Photoshop parity.

Core invariants cover original preservation, signed/default coverage, cancellation and history, duplication/cel ownership, fractional edges, linked transforms and bounded rejection. The real-device `gpu_shading` probe adds exact raw masked CPU/GPU cases including clipping, blend, cache updates, toggle and removal. Storage tests cover native/web typed recovery separately.

Before marking the workflow complete, use scratch artwork to paint/hide/reveal, switch image/mask targets, disable/restore, transform, save, terminate and reopen in the actual host. Compare original RGBA, R8 coverage, tree/history and composed output; cover crash recovery and retained redo branches. Visible app use, physical pen, installed-product use and first-visible-pixel latency are distinct evidence and must remain explicitly unverified when not exercised.

Mask painting color semantics were corrected during browser acceptance: the captured linear-premultiplied brush color is unpremultiplied and encoded to display sRGB before computing weighted gray, then premultiplied by its original alpha for scalar coverage painting. Thus an opaque UI gray of 128 paints approximately 128/255 coverage, rather than linear-light 55/255. Black/white, brush opacity/flow, selection coverage and eraser-to-white are preserved. Existing mask objects remain stored coverage; they are not reinterpreted or rewritten. Very low-alpha brush colors retain the existing byte-quantization limit.
