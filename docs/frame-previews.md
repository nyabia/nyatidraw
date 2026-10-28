# Timeline frame previews

The shared timeline requests only visible frames and two nearby entries on each
side, capped at 24 IDs. Each preview samples the finite page at up to 48×36 pixels
through the CPU compositor, including layer masks, groups, clipping and blend
modes. It never uses onion overlays, a GPU readback or a full-page export buffer.

Native rendering admits one private preview request at a time to the existing
materialization worker after artwork work is idle. Web rendering handles one
frame per deferred background turn on the browser main thread and pauses during
drawing, playback and transform/color drafts. This bounds admission to one frame,
not execution time: there is no hard millisecond budget or large-document latency
claim. Neither path marks artwork busy or writes history.

Hosts retain at most 64 encoded images of at most 16 KiB each, plus the single
in-flight request. The cache keeps source fingerprints and PNG strings, never
old source pixel objects. Fingerprints include the selected cel's layer tree,
owned image/mask tile keys and hashes, and page metadata. Unchanged cels reuse
their PNG after global preview invalidation; timing/order changes may revalidate
fingerprints but do not require re-rendering those images.

Project epoch and monotonic preview generation accompany both requests and
results. Current snapshots omit invalidated or failed entries and retain the
frame number/duration fallback. A late result from another project, history
state or no-longer-visible frame is discarded. No image string enters the domain
projection or project format.

## Incremental browser acceptance

A fresh headless Edge session against the actual release web app displayed both
36×36 previews for a two-frame square-page fixture. The first run exposed missing
web `host_update` propagation: previews were generated but equal timeline props
prevented their display. Passing the editor refresh revision into shared chrome
fixed this observed failure. No mock UI was used.

The following checks passed, with evidence in ignored
`output/playwright/masks/`:

| Actual operation | Result | Evidence |
|---|---|---|
| Initial two-frame display | Both actual images loaded | `previews-refresh-fixed.txt` |
| Enable frame 1's mask | Frame 1 preview changed; frame 2 remained identical | `previews-enabled.txt` |
| Undo mask enable | Frame 1 preview restored exactly; frame 2 still identical | `previews-undo.txt` |
| Change duration 240→350 ms | Label changed; both preview image strings remained identical | `previews-duration.txt` |
| Collapse and reopen timeline | Both previews restored | `previews-collapse-reopen.txt` |
| Playback then stop | Editing frame and both preview images restored | `previews-playing.txt`, `previews-stopped.txt` |
| Replace with static project, then reopen animation | Static project exposed no frame buttons; reopened animation showed its original preview hashes | `previews-project-replaced.txt`, `previews-replaced-back.txt` |
| Close browser and reopen animation in a fresh process | Both preview hashes matched the reopened original project | `previews-process-restarted.txt` |
| Duplicate from 2 to 30 frames, scroll to the end, then press Home | Selected frame previews loaded at both ends; frame 1 selection restored; visible images stayed within the 24-request cap | `frame-preview-scroll-result.txt`, `frame-previews-scroll-end.png`, `frame-previews-scroll-start.png` |

The recorded SHA-256 values hash the encoded image **data URI strings**, not raw
RGBA. Frame 1 changed from
`f5ebe9228f87f514c5a042f4be79dd6222bb89084fb2bd5615f788bab8efb28a` to
`2a4e5b68edefe6459812434cebd4b8ba101ea2713cf885e503541d81ff0f7a9d`, then Undo
restored the first value. Frame 2 stayed
`a1c48d07d536c3093f6cb7d87ce86c6e0632f4e9c1448402423b887d818e92d1`.

Reopening the original project restores its saved mask/time state, not the
unsaved duration/Undo state used in the earlier checks. Its first preview is
`2a4e5b68edefe6459812434cebd4b8ba101ea2713cf885e503541d81ff0f7a9d` and second
preview remains `a1c48d07d536c3093f6cb7d87ce86c6e0632f4e9c1448402423b887d818e92d1`.
The 30-frame release-web run recorded 15 ready images at the end, with
`scrollLeft=1872`, `scrollWidth=3312` and `clientWidth=1440`. Clicking frame 30 and
pressing Home selected frame 1 and returned to `scrollLeft=8`; 2 images were ready
at that recorded instant, including the selected frame. These are actual browser
observations, not a claim that all 30 thumbnails render at once or a latency
benchmark. The request cap remained 24.

An additional A→B→A file-open sequence used two snapshots cloned from the same
32×32 animated project, retaining identical frame IDs. A had a partly hidden red
first frame; B had that frame fully hidden. The first preview changed from
`e02bb48a9b4d11465c8294deb57ed68facbdc523436b22f5ed72702cd438d060` to
`3cf36613bc93e8512993afbadc0bf7721a49ced855139f03a6e26c8a7fc7ee67`, while the blue
second frame retained `dbdf02a7f6ea7228f6c8719ec47571164727acd96b43ebf852dd6b40dc9f0338`.
Reopening A restored both original hashes. Evidence: `replace-a-previews.txt`,
`replace-b-previews.txt`, `replace-restored-previews.txt`. This checks sequential
document replacement, not every possible late-worker race.

The native scratch `fill-check.ntdr` was saved and normally closed after a black
mask fill, retained selection, and Undo/Redo checks. A fresh GUI process reopened
snapshot 116, root `e684cc2a46b4c9ae086d3a2769d343ba`, with a transparent first
frame and a blue second-frame preview. The fresh CLI export and saved PNG matched
as raw RGBA; the complete artwork evidence is in
[workflow validation](workflow-expansion-validation.md). The mask-selection
keyboard-focus regression was subsequently fixed and verified there. This
observation does not complete the
[native canvas presentation gate](research/native-mask-presentation.md). These
local results do not establish physical display/pen latency, installation, push
or deployment.
