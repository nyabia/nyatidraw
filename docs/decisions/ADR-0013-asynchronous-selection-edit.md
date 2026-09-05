# ADR-0013: Bounded asynchronous selection and fill admission

Status: accepted for semantic worker integration; native selection gestures,
mask display and selection-clipped brush/eraser remain pending.

## Decision

Completed Wand/Lasso/Fill/Gradient commands use the semantic editor lane.
Lasso admission bounds its completed polygon to 3..=4096 points; this lane never
contains raw stylus samples. The existing serial project writer owns the mask,
computes against immutable CPU tiles and commits changed pixels/history before
returning them. No-op paint adds no history. Validation/resource failures retain
the previous mask and artwork; transaction/adoption failures latch a workspace
error and require reopening. There are no new dependencies or wire changes.

One active edit and one buffered response are allowed. Enqueue and completion
polling do not wait on the renderer. A one-slot response lets the writer finish
after the canvas retires; Close joins the writer on the existing background path.
Save is retained until adoption, or appended after the edit in the writer FIFO
when Close retires display. A Save already queued at the pre-completion revision
gets the same single-frame compatibility as native stroke closure. Other stale
commands retain normal revision rejection.

Under the raw-input mutex, edit admission checks both the bounded queues and an
admitted active gesture. An empty queue alone is insufficient. While paused, a
new Begin is explicitly declined and its tail cannot enter replay after resume;
only a clean Begin resumes. Layer/history mutations are refused while an edit is
pending. Viewport/docking and Save remain available. Project activation clears
the pause and mask state. History/layer artwork changes clear the session mask.

`UiProjection.edit` publishes busy, presence, count and error without pixels.
Presence is separate from count so an empty Lasso selection can still be cleared.
The installed status strip exposes busy/error/count and Clear Selection. Until
brush clipping is connected, a present mask keeps brush admission paused; the
ordinary selection/fill tool buttons remain disabled. This intermediate behavior
must not be presented as finished desktop selection UX.

## Evidence

One input/artwork invariant test checks active-but-empty queues, queued End,
declined Begin/tail, clean-Begin recovery and project activation reset. No UI
mock tests were added. Workspace total: 66 tests; all-target/all-feature Clippy
uses `-D warnings`. Logs: `target/async-edit-tests.log`,
`target/async-edit-clippy.log`, `target/async-edit-install-final.log`.

`desktop_edit_fixture` creates a 129x4 scratch project with signed off-page
pixels and a nontransparent boundary-padding pixel at x=129. Its separate verify
process checks exact tile bytes, tree, history/cursor and PNG against independently
constructed green/transparent pixels, rather than rerunning the fill algorithm.

The installed DX 0.7.9 release was exercised through Windows Computer Use
(`@oai/sky`), on Windows 11 / Intel Arc / DX12. A marked scratch-only barrier
holds the real worker before fill; it is not a computation performance sample.
With Wand complete and fill pending, Save remained queued, a new raw gesture was
explicitly declined, and clicking Zoom changed the displayed 731% to 913%.
Alt+F4 displayed the saving dialog while the worker remained paused. Releasing
the barrier completed snapshot 2, exported generation 1 and joined cleanly.
A fresh verifier process matched all 516 page pixels and preserved off-page data.
Evidence: `target/async-edit-ui-1/app-out.log`, `app-err.log` and this task's
Computer Use observations. This is injected mouse/keyboard UI evidence, not
physical pen, first-visible-pixel latency, power-loss or cross-backend proof.

On the final installed build, a second scratch case released the barrier while
the canvas remained open: green pixels appeared on the GPU canvas, PNG reached
current, and Clear Selection removed the 516-pixel status. UI Undo restored the
transparent page, then Save/Close and a fresh verifier matched snapshot 1 with
both history nodes. A new desktop process displayed that transparent page; UI
Redo restored green, and Save/Close plus another verifier matched snapshot 2.
Logs: `target/async-edit-ui-2/app-out.log`, `reopen-out.log`,
`verify-undo.log`, `verify-redo.log`; first-case verification is retained in
`target/async-edit-ui-1/verify-close.log`.

## Remaining gates

Connect native gestures, a bounded visible selection overlay, color conversion
from UI sRGB, brush/eraser CPU/GPU mask agreement and full installed tool flows.
GPU adoption currently reuses the history snapshot uploader; large-page adoption
must be measured and reduced if it exceeds the input/render budget. Synchronous
metadata/history requests and activation joins still need hot-path isolation.
