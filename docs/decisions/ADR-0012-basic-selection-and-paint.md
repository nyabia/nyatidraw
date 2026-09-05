# ADR-0012: Bounded CPU selection and basic paint

- Status: CPU contract and durable results accepted; desktop integration pending
- Date: 2026-09-05

## Source and selection contract

Selection is a binary mask of pixel centers inside the finite output page.
Signed artwork outside that page remains in the project. Selection masks are
disposable session state, not PNG pixels or new project records. Desktop ownership,
overlay, cancellation and gesture integration remain subsequent work.

| Source | Sampled pixels |
|---|---|
| Active layer | Raw active raster pixels, independent of its visibility/opacity and ancestor groups |
| Reference layers | Only marked raster leaves, with ordinary visible ancestor/group/raster opacity semantics |
| All visible | Ordinary visible document composite |

Solo never changes the source. Reference mode with no effectively visible marked
raster returns `MissingReference`; it does not silently sample another layer.
Active-layer mode rejects an unknown raster. Locked layers may be sampled but
cannot be paint destinations.

Wand selects the 4-connected seed region. Tolerance is an inclusive maximum
absolute channel difference from the original seed in premultiplied linear RGBA8,
including alpha. It does not compare against the last neighbor, expand diagonally
or work in gamma-encoded RGB. Seeds outside the page are rejected.

Lasso uses even-odd pixel-center coverage and clips to the page. Integer vertices
may be signed; horizontal/repeated edges are permitted. Rational scanline
intersections use i128 arithmetic, making reversal and very large signed
coordinates deterministic. Edges are not antialiased; refinement is out of scope.

## Paint and resource boundaries

Solid fill and clamped linear gradient paint source-over into one unlocked raster
under the mask. Gradient endpoints use integer page coordinates and colors are
interpolated in premultiplied linear RGBA at pixel centers. A zero-length gradient
or color channel exceeding alpha is rejected. Transparent paint is a no-op.
Unselected pixels, other layers, signed off-page tiles and padding beyond the
page in boundary tiles remain exact.

All operations consume immutable input and return a candidate result. A failure
never publishes partially painted tiles. The writer must still commit that result
and history before adoption; this module does not perform I/O or own project state.

Hard defaults, which callers can tighten but not enlarge, are 16,777,216 page
pixels, 256 MiB estimated temporary workspace, 1,048,576 flood frontier entries,
4,096 polygon vertices and 67,108,864 polygon edge/scanline visits. Source
preflight includes recursive full-page compositor scratch surfaces. Paint first
counts only selected destination tiles and checks their candidate storage before
copying pixels; a small selection does not reserve every tile on a large page.
These are allocation/work guards, not measured process RSS or latency guarantees.

## Evidence and next boundary

Three core invariant tests cover seed tolerance/connectivity/reference scope;
clipped, reversed, self-crossing and extreme-coordinate lasso coverage; and
paint/gradient boundary padding, other layers, locks, limits and no-op preservation.
Workspace tests total 65, with focused tests repeated after tile-loop changes and
all-feature/all-target Clippy passing with `-D warnings`. No UI mock tests were added.

`basic_edit_reopen_probe --release` uses six fresh child processes to reopen an
actual scratch redb project. Independent expected tile bytes cover reference-wand
fill, lasso gradient, Undo, a new fill branch and explicit Redo to the gradient
branch. Reopened trees/tiles/cursors/sibling count are exact, and fresh PNG outputs
match PNGs encoded from independently constructed expected pixels. Scratch exports
use distinct filenames, so this probe does not exercise production replacement
or generation checks. Existing production export semantics remain unchanged.

Logs: `target/basic-edit-tests.log`, `target/basic-edit-core-tests.log`,
`target/basic-edit-clippy.log`, `target/basic-edit-release.log`.
The [CPU measurement](../performance.md#2026-09-05-basic-selection-and-fill-cpu-measurement)
shows that even the improved 4K operation belongs off the input/render thread.

No external dependency or project format changed. Desktop tools remain disabled
until native gestures, bounded asynchronous work, selection display and history
adoption are connected. Brush/eraser clipping to an active selection also needs
an explicit durable/GPU contract before selection can be presented as generally
affecting those tools. This checkpoint is not desktop tool, physical-pen or
input-to-visible acceptance.
