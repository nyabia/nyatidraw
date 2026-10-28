# Web artwork movement preview

The **그림 이동** tool previously drew a start-to-end guide during pointer movement
and evaluated `MoveArtwork` only on pointer release. It therefore did not display
the moving artwork, independently of the input device. This is separate from the
free-transform tool and its explicit confirm/cancel controls.

The web host now pins an immutable move source at pointer Begin. Move events only
replace one pending integer offset; the existing animation-frame callback computes
and uploads the latest preview before rendering. Coalesced Pencil samples do not
cause a full pixel transformation for every sample. The original artwork, history,
selection and recovery checkpoint remain untouched during preview.

The preview and final edit use the existing integer movement kernel, including
signed off-page pixels, selected pixels and linked raster masks. All inactive cel
tiles remain in the source snapshot; only the active frame tree is uploaded.
Pointer End evaluates its exact final offset and commits one artwork operation.
Cancellation, lost pointer capture, resize, tool changes and other edits discard
the draft and restore the authoritative scene. A stale source or invalid offset
cannot commit an older preview. No desktop interaction or project format changes.

## Verification

Core regressions cover immutable repeated previews, masks and inactive cels,
selection translation, cancellation by discarding the draft, invalid offsets,
stale source rejection, one Undo/Redo and portable re-encoding/reopen.

Browser acceptance must compare the canvas before, **while the pointer is still
down**, and after release. Drag again and dispatch pointer cancellation or release
capture to verify exact restoration; repeat with a selection, linked mask and an
inactive animation cel. Save the committed result as NTDR and PNG, restart the
browser, reopen NTDR in a fresh process, and compare raw artwork/exported pixels.
One Undo must restore the original and one Redo the moved result.

Source formatting is not live browser evidence. Actual Apple Pencil, iPad Safari,
physical-display latency and sustained large-artwork interaction require device
acceptance; synthetic pointer input does not establish those results.
