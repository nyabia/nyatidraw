# Layer pointer drag acceptance

Layer rows now use WebView pointer capture rather than HTML/OLE drag-and-drop.
The old HTML-specific diagnostic and overlay path have been removed. Coordinates
and insertion feedback stay in JavaScript; only the final source/target/position
and starting revision cross IPC. Rust resolves the authoritative parent/index,
corrects bottom-to-top sibling indices and rejects cycles and stale revisions.

- Drag a thumbnail, read-only name or non-button row content.
- Names enter edit mode on double click; buttons and editable text remain separate.
- Above/below insertion uses a cyan bar; group-center insertion uses an outline.
- The list scrolls near its top/bottom edge while a drag is held.
- Escape, pointer cancel/capture loss, window blur and stale revisions cancel.
- The existing durable layer-reorder/history command is unchanged.

## Verified here

Focused Playwright CLI acceptance in installed Edge used the production JS/CSS
on an ignored scratch page. Real browser pointer capture (automation-driven
mouse, not synthetic DragEvent dispatch) produced exactly one semantic packet
for above, below and group-inside moves. Markers appeared. Escape, a cycle into
the moving group's child, and a changed revision emitted no packet. This is not
installed WebView2/manual pen evidence or full editor save/reopen proof.

A small core invariant covers compositing index adjustment, same-position no-op,
cycle and stale-revision rejection. Existing durable layer order/reopen tests
remain the persistence gate. Manual installed-app dragging, large-list scrolling
and pen dragging remain acceptance items.
