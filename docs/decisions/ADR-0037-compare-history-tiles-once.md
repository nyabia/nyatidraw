# ADR-0037: Share one history tile comparison between CPU and GPU adoption

- Date: 2026-09-06
- Status: Accepted for bounded installed artwork restoration and adoption improvement; whole-Undo latency target remains open

After ADR-0036, installed history adoption still used p95 19.455 ms. GPU upload
selection compared all mutable CPU tiles with the incoming validated snapshot,
then CPU cache reconciliation repeated the same byte comparisons. The 4K fixture
has 902 keys and about 59 MB of mutable pixels, although only 56 keys change.

Build one short-lived adoption plan containing the union of old/new keys and
whether each differs. The plan borrows the mutable cache and immutable snapshot
until it is consumed, so those decisions cannot become stale during upload.
Surviving surfaces receive changed/new/deleted keys; recreated surfaces still
receive all keys, including unchanged artwork. Deleted tiles upload transparent
pixels only when their layer still exists, as before.

Only after all GPU uploads succeed does the plan update changed CPU buffers or
remove deleted keys. It retains existing allocations and untouched pixels.
An upload error drops the unconsumed plan, preserving the previous CPU cache and
the existing workspace-error quarantine. This is per-adoption state, not a
persistent cache of hashes for mutable artwork. Input admission, durable storage,
snapshot validation, selection clearing and page/tree rebuild rules are unchanged.

The existing table-driven artwork invariant now also checks the exact upload
key sets for both surviving and recreated surfaces. It covers signed tiles,
modified/unchanged/deleted pixels, restored layers and Undo to an empty map.
No new test cases or framework mocks were added. All 16 desktop tests and
workspace all-target/all-feature Clippy with warnings denied passed. The known
upstream vendored Wry lifetime warning is unchanged.

Logs: `target/history-compare-once/{tests,clippy}.log`. The nested
`history_cpu_snapshot` span now measures applying the precomputed differences;
the one comparison is inside the surrounding `history_adoption` span. Comparing
that nested span alone across versions would overstate total improvement.

No dependencies or file formats changed.

## Installed 4K acceptance

Installed source `e4f9f4c`, executable SHA-256
`68339D4E425A072E481869413048DB065D1C88C0194A4D6CCEE708E62F9AA056`.
Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc DX12, pinned Dioxus CLI
0.7.9 release; 2096×1458 surface, scale 2. The historical 3840×2160 fixture has
two raster layers, 32 strokes, 33 history nodes and 902 tiles. Configured display
records are inherited from the earlier campaign, not new physical measurements.

One toolbar Undo click and 19 alternating keyboard Redo/Undo operations completed
ten pairs. Every operation retained 846 GPU tiles and uploaded 56. Queue,
changed-adoption and adoption-frame-present counters each equal 20; first sample
included, no warmups omitted. No build/test ran during the 20 operations. Other
host activity was uncontrolled; unrelated processes were not inspected or altered.

| Interval, histogram upper bounds in ms | p50 | p95 | p99 |
|---|---:|---:|---:|
| History adoption | 10.239 | 11.775 | 12.192 |
| Apply precomputed CPU differences | 0.383 | 1.023 | 1.234 |
| Worker queue admission → adoption | 27.647 | 30.719 | 149.249 |
| Worker queue admission → restored frame present API | 45.055 | 47.103 | 157.958 |

The preceding ADR-0036 session measured adoption 17.407/19.455/19.833 ms and
queue-to-present 51.199/55.728/55.728 ms. Both used the same default WebView
profile and missing scratch-layout configuration. These are separate,
uncontrolled sessions, not randomized A/B measurements. Adoption and whole-Undo
p95 improved, but the single longest whole-Undo sample worsened p99 and is
retained. The queue-to-adoption maximum is also long while adoption itself is
bounded by 12.192 ms here. Histograms do not correlate individual stages; they
do not establish which operation was slow or whether storage, scheduling or
another cause delayed it. Further worker-stage evidence is needed.

The whole-Undo 16 ms target still fails. Queue admission excludes earlier OS/UI
delivery; present API return excludes GPU completion and visible pixels. Sparse
keyboard Undo measurements do not replace paced drawing/export interference.

Save completed PNG generation, normal Close drained and joined the writer, and
a separate verifier matched all 902 tiles, tree/page, snapshot/history and decoded
PNG. An ordinary installed restart without overrides displayed the artwork;
normal Close and a second complete comparison passed. Measured startup logged
the handled focus error; ordinary restart had empty stderr.
Logs: `target/history-compare-once/ui/`. The original executable is recorded in
`binary-hash.json`; all rows and verification paths are in the
[measurement JSON](../measurements/history-compare-once-4k-2026-09-06.json).

## Page and layer surface rebuild acceptance

A separate copy of the previously accepted page fixture began at the 2×2 crop,
snapshot 2/history 5. Actual installed UI exercised these states:

| Action and save/close checkpoint | Page | Snapshot / history | Result |
|---|---|---|---|
| Undo crop | 16×16 | 1 / 5 | Full independent comparison passed |
| Restart, Redo crop, delete Crop ink | 2×2 | 6 / 6 | Deleted layer and PNG comparison passed |
| Restart, Undo deletion | 2×2 | 2 / 6 | Restored layer and PNG comparison passed |
| Ordinary restart, normal Close | 2×2 | 2 / 6 | Full independent comparison passed again |

The first three sessions had timing enabled; the last had no overrides. All four
normally closed and joined their writers. Every independent checkpoint checked
the full tree, signed/outside-page tiles, locked and hidden artwork and an exact
PNG generated from explicit expected points. These checks are separate processes,
not screenshot-only comparisons. Recreated surfaces uploaded 7 keys on crop Undo,
7 on Redo, 4 on deletion and 6 on restoration; no unchanged-key reuse was claimed
for recreated surfaces. Raw logs and oracle results: `target/history-compare-once/page/`.

This closes the bounded artwork acceptance for this optimization. Long-session
latency, unexplained worker tails, Explorer Open With and UI-thread surface-wait
isolation remain separate open gates.
