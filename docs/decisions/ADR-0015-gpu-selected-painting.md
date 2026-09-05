# ADR-0015: GPU and durable strokes share immutable selection coverage

Status: accepted for selected brush/eraser rendering and native painting;
native selection gestures and overlay remain pending.

## Decision

Upload the canonical packed mask once when the edit worker result is adopted.
Brush and eraser share the same immutable GPU storage buffer (at most 2 MiB).
Fragment coverage uses document pixel coordinates, including sparse tile
origins, and discards every pixel outside the finite selection. An empty mask
rejects all paint; removing the mask restores unrestricted painting. Mask
allocation and packing do not run per raw sample.

The desktop captures the corresponding CPU mask at Begin and carries it through
End or normal-close materialization into the selected-stroke storage contract
in ADR-0014. CPU and GPU mask adoption complete before raw input resumes.
Pixel-only edit results retain selection; history and layer mutations clear
both masks. These mutations atomically pause admission while changing artwork,
so a new Begin cannot race selection replacement. Synchronous history/metadata
work still needs isolation from the renderer in a subsequent gate.

No dependency or project-format changes are introduced by this connection.
Selection session state remains transient; recorded stroke masks are durable.

## Evidence

`gpu_selected_stroke` ran in release profile on Windows 11 Home build 26200,
Core Ultra 7 155H / Intel Arc integrated graphics, separately with DX12 and
Vulkan. Its 129x129 page crosses sparse tile boundaries and includes negative
tiles, page padding, nonrectangular coverage and an empty selection. Partial
pressure brush and eraser dabs run in multiple batches. Actual GPU readback is
compared with CPU replay: maximum RGBA channel delta was 2 for brush and 1 for
eraser; all unselected pixels matched exactly. Cancel restores exact GPU before
bytes, including after a previous commit; clearing selection restores ordinary
painting. The probe renders the viewport first to load lazy sparse tiles.
Readback is a diagnostic path, not part of desktop input processing.

Logs: `target/gpu-selected-dx12.log`, `target/gpu-selected-vulkan.log`.
These are correctness checks, not latency measurements or cross-platform proof.

The installed Windows release was controlled with the computer-use native
mouse/UI API on a marked scratch project. A probe creates only a 64x65 selection
on a 129x65 page; it injects no brush samples. Mouse brush input at the boundary
visibly clipped, then Save and normal close joined the writer. A separate
process verified snapshot 3 and three history nodes, 1,092 changed selected
pixels, exact unselected/negative/padding bytes, recorded mask, CPU replay and
exported PNG. The drag command yielded only Begin/End samples, so this evidence
is mouse dab coverage, not continuous drag fidelity.

After restarting, the UI eraser changed 297 alpha values in the decreasing
direction; snapshot 4 passed the same independent checks. UI Undo after another
restart removed the session selection and restored snapshot 3 while retaining
four history nodes. UI Redo and another save/reopen verification restore the
selected eraser result without requiring a session selection.

Scratch evidence: `target/gpu-selected-ui-1/` contains app logs and
`verify-brush.log`, `verify-eraser.log`, `verify-undo.log`, `verify-redo.log`.
Workspace tests remain 68; no new unit tests were added for GPU/UI wiring.
All-target/all-feature Clippy and formatting checks passed.

## Remaining gates

Native Wand/Lasso/Fill/Gradient gestures and selection boundary display are
still disconnected; their ordinary toolbar buttons remain disabled. Large-mask
adoption and edit-result upload need measurements before any responsiveness
claim. Physical pen, high-refresh and first-visible-pixel latency remain
unverified. Inline mask storage overhead is unchanged from ADR-0014.
