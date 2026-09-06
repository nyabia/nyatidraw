# ADR-0037: Share one history tile comparison between CPU and GPU adoption

- Date: 2026-09-06
- Status: Core invariant and compilation accepted; installed acceptance and timing pending

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

Next: installed 4K UI Undo/Redo, Save/normal Close/ordinary restart and independent
full artwork/PNG comparison; page/tree rebuild acceptance and whole history
queue-to-present timing. No performance improvement or completed artwork gate is
claimed until that evidence is recorded. No dependencies or file formats changed.
