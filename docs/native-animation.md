# Native raster animation integration

The native writer retains one authoritative layer tree and tile snapshot for all
cels. The renderer receives only the selected frame's tree. Layer commands edit
the selected cel subtree and merge it into the authoritative tree before the
existing structural commit; an active-frame tree must never replace the master
tree in storage, where absent layer IDs mean deletion.

Timeline changes use the shared editor reducer and commit the timeline, layers,
page, pixels, and history together. Undo/Redo loads the timeline associated with
the destination cursor before accepting it. Frame selection is session state and
does not add history or mark artwork dirty. Reopen chooses the first exposure
containing the persisted active layer, falling back to the first frame. Exact
exposure position is not independently persisted for linked cels.

Picker, reference fill, navigator, thumbnails, page copy, ordinary PNG, and
reduced PNG export use the editing frame. Group transforms and provisional
paste retain the complete authoritative tree, with the target and requested
group checked against the current cel. Paste insertion uses the target's actual
parent. Page resize remains global; crop is rejected for animation until a
document-wide rebase has its own acceptance evidence.

Playback uses monotonic elapsed time and a separate display frame. A single
bounded wake channel and clock thread request redraw at 16 ms intervals. Stop
restores the editing frame. Drawing admission remains paused throughout
playback; editing and export commands require Stop first. All frame scene
budgets are checked before playback starts. This is scheduling behavior, not a
claim of exact display refresh or first-visible-pixel latency.

Onion display is built on the writer thread using the shared two-neighbor tinted
proxy implementation. Its aggregate scratch/overlay budget is 128 MiB, followed
by the existing GPU scene budget check. Disposable onion pixels never enter
authority, history, picker input, or export. Structural, history, and completed
transform changes refresh the display asynchronously. Oversized onion requests
report an edit error and leave artwork intact.

## Acceptance boundary

The root task completed compilation, strict workspace Clippy, focused invariants,
the fresh-process actor probe, and the bounded visible checks recorded below.
These do not claim physical-input or every native interaction has passed.

Required native acceptance includes enabling animation on layered artwork,
blank/duplicate/reorder/duration/delete, drawing into different cels, Save and
fresh-process reopen, branch Undo/Redo across timeline edits, protected group
transform and paste inside one cel, picker/export isolation, playback Stop and
rapid Stop/Play input recovery, onion budget rejection, and close/reopen during
ordinary pending writer work. Scratch projects only.

## Actor probe invocation

Use a fresh scratch directory containing the empty marker
`.nyatidraw-scratch-animation-probe`. Set `NAYATI_PROJECT_PATH` to its
`animation-scratch.ntdr` (initially absent) and `NAYATI_ANIMATION_PROBE=write`.
Set `APPDATA` and `LOCALAPPDATA` to separate directories below that scratch
directory before launching the freshly built debug desktop binary. Do not pass
another positional project path. The process refuses to route into an existing
desktop instance. It performs commands through the actual actor and writer,
prints `native-animation-probe event=write-complete`, and writes the sibling
`animation-proof` file only after PNG Save reports Current.

Close that scratch desktop normally. In a fresh process with the same scratch
environment, change only `NAYATI_ANIMATION_PROBE` to `reopen`. The probe compares
the full tree, timeline, exact tile root, editing frame, and independently
flattened red/blue frames against the first process; it checks that the actual
GPU scene topology excludes inactive cels and onion proxies. Success prints
`native-animation-probe event=reopen-complete`. It does not automatically close
the window, fake pointer input, or claim visible-pixel/readback acceptance.

## Recorded native acceptance

On Windows 11 Pro 10.0.26300 with NVIDIA GeForce RTX 3080 / DX12, the final
DX desktop debug bundle passed a fresh scratch write and reopen sequence.
The executable SHA-256 was
`8605A3DE1958EBE6986167B607CFAC851DBCFDB863F034AAB747720E02D374FC`.

- The actual native actor created red and blue 32x32 cels, changed the first
  duration to 40ms, verified duration Undo/Redo, playback changing display cel,
  Stop/Play/Stop, temporary onion installation/removal, and saved companion PNG.
- The first process closed normally and joined the writer. A fresh process
  verified identical full layer tree, timeline, tile root, selected frame, both
  independent composites, zero RGBA outside the page, and active GPU topology.
- Computer Use inspection of the actual native window showed first-frame red
  and 40ms, then clicking the second frame showed blue and 100ms. Both frames
  stayed inside the expected page border. The final scratch window closed normally.
- Logs and exact comparison proof remain under ignored
  `target/native-animation-final-6829d8aa68224cdfb2dd4d5ba9578740/`.
  The proof SHA-256 is
  `AD8387197F41C1BE8B3CA6BBE4D8DEE8BEBCB200018181679AB1770450C08432`.

The first run exposed stale duration input after frame changes and Select All
including transparent allocated-tile padding. Both were fixed and the final
probe was rerun in a new directory. The earlier blue off-page pixels were real
CPU fill results, not a GPU rendering error. The final probe checks every stored
pixel outside the page rather than only the flattened page image.

Native physical-pen drawing, native group/paste interaction through all dialogs,
frame-time precision, installed release packaging, and high-refresh display
latency remain outside this acceptance. Web group/transform/reference/export
interaction evidence is recorded separately in `animation-acceptance.md`.
