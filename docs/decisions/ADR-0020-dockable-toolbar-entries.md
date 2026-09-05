# ADR-0020: Dockable toolbar entries and side stacks

Status: accepted for toolbar placement and restart restore. Pointer capture and
drag cancellation across native-window boundaries remain separate work.

## Decision

Canvas actions, viewport controls and quick colors are independent entries in
the authoritative `DockTree`. Its ordered top list and workspace tree are
validated together: all ten entries must appear exactly once, and only the three
toolbars may occupy the top list. Commands move entries into the workspace or
before an existing top entry/at the end of the top row. Invalid changes never
replace the current tree. Menu through Redo remains fixed.

The top row renders a title-free vertical grip followed by horizontal controls.
Side entries reuse the same command/projection components in vertical panels.
The grip contains no controls. Top drop targets show the existing cyan insertion
line and endpoints; side targets continue to expose Left/Top/Right only.

Vertical workspace subtrees without the canvas render as one side stack.
Leading panels have preferred content heights and can shrink with scrolling;
the trailing panel fills the remaining height with a 160px basis. Flattening
the view avoids nested split ratios progressively reducing the last layer panel
to its toolbar. The persisted tree retains its structure, and splits containing
the canvas retain their ratios. Long panel titles are ellipsized within their
header, with the full label available in the title.

`NYDOCK02` extends the layout record with a bounded ordered top-entry prefix and
three appended stable panel IDs. `NYDOCK01` still loads its original workspace
and supplies the three default top entries. The first accepted layout change
writes version 2 through the existing independent atomic settings worker. No
artwork format, dependency, native canvas owner or project session changes.

## Evidence

Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, DX12 installed release
build. Computer-use automation operated real installed controls against
`target/toolbar-ui/edit-source-scratch.ntdr`, a 129×65 copy of the source/tolerance
fixture. `NAYATI_LAYOUT_PATH` selected only the sibling scratch preferences.

- Started from the prior version-1 default layout. Dragged Canvas actions above
  Brush on the left, Viewport above Navigator on the right, then Quick colors
  above Canvas actions. The top row retained only the fixed commands.
- In the right-side Viewport panel, actual Zoom changed 731% to 913% and the
  navigator followed. Selecting red in the left Quick colors panel updated both
  its current chip and the regular Color panel to `#E83030`.
- After normal close and a fresh process, all three toolbar placements restored
  from version 2. The corrected flattened side stack retained usable space for
  the trailing Layers/History panel instead of squeezing it to its header.
- Returned Quick colors to the empty top row, inserted Viewport before it, then
  returned Canvas actions at the end. Direct top-to-top dragging then produced
  Quick colors / Viewport / Canvas actions. Another process restart visibly
  restored that order. Fixed commands remained in their original segment.
- Each observed session logged one GPU canvas resume and one native canvas
  creation, followed by `close-ready writer=joined`. After each close, a separate
  fixture process verified all 8385 page pixels, other layers, negative tiles,
  boundary padding, snapshot 2/history 2 and independent PNG expectations.

App/verification logs are in `target/toolbar-ui/`: `first-*`, `reopen-*` and
`top-reopen-*`. `all-side.layout` and `reordered-top.layout` retain the two accepted
version-2 arrangements. Workspace/all-target/all-feature Clippy, the two existing
API invariant tests and the final release install passed (`target/toolbar-clippy-final.log`,
`target/toolbar-api-check.log`, `target/toolbar-install-final.log`). No new tests
were added for this UI integration.

## Remaining work

Current docking still uses the existing mouse-event path. Explicit capture,
release outside a target/window, capture loss and cancellation across the child
HWND need implementation and focused acceptance. This evidence does not establish
active physical-pen continuity, arbitrary long-session layouts or input/display
latency percentiles. Quick-color swatches still use the existing preset palette;
a true shared recent-color history is also unfinished. These remain open Sprint 3
requirements, independent of placing the controls.
