# ADR-0006: Branch-preserving history DAG and explicit redo selection

- Status: **Accepted for the core history boundary**
- Date: 2026-09-01

## Context

The first `History` implementation retained a linear redo stack. Appending after
undo cleared that stack, deleting the only in-memory path to the previous child
branch even though its immutable content root and operation node were still
valid. This conflicts with the project rule that semantic operations and exact
roots remain recoverable.

## Decision

Store every `HistoryNode` by ID and maintain a second parent-to-children index.
Both maps use ordered collections. The cursor consists only of the current
optional head ID and current immutable `ContentRootId`.

- `append` accepts only a unique caller-provided ID whose parent equals the
  current head and whose before root equals the current root.
- `undo` follows the current node's parent and before root without deleting or
  copying any node, child, tile, or pixel buffer.
- `redo_candidates` returns every direct child in ascending `HistoryNodeId`
  order. This order is stable regardless of insertion and traversal history.
- `redo_to` requires explicit direct-child selection and validates the stored
  parent/before-root transition before moving the cursor.
- Convenience `redo` succeeds only for exactly one child. Multiple children
  return `AmbiguousRedo`; it never guesses the last visited branch.
- Appending a new child after undo adds it to the parent child set and preserves
  all existing redo branches.
- Reopen decodes the full persisted `history` table into the same ordered maps,
  retaining all direct children at the durable cursor rather than reconstructing
  only its ancestor chain. It rejects a malformed node key/envelope, duplicate
  ID, missing parent, discontinuous parent-after/before root, cycle, or cursor/
  current-root mismatch. Enumeration is capped at 100,000 nodes.
- `ReopenedProject` is storage-neutral: it joins the validated complete
  `History` with the current materialized batch. `HeadlessStrokeSession::from_reopened`
  consumes that state without resetting its history.
- A `ProjectHistoryCursor` names an existing snapshot/head/root. Undo or
  explicit redo first prepares this immutable target, hydrates its root, then
  commits an immediate state-only cursor transaction. The editor accepts the
  move only after that transaction succeeds; no tile object, stroke, or
  history node is rewritten or deleted.
- The pre-first-commit root is a distinct durable cursor record with no
  `HistoryNodeId`, seeded from the first commit's exact before root and parent
  snapshot ID. It is not a fake stroke. Undoing the first node can therefore
  select and reopen the initial root, whose redo candidate remains that first
  persisted node.
- Before a stroke transaction begins, the backend verifies the batch parent
  snapshot, before root, and history parent against the durable cursor. A new
  parentless node is accepted only for an empty project or while the initial
  cursor is current; a fresh-session second root is rejected before writes.

IDs remain a caller policy. Core history neither uses a clock nor an internal
counter, and duplicate IDs are rejected. A durable ID allocator or content-ID
scheme can therefore be selected later without changing DAG traversal.

## Evidence

One core invariant test constructs two branches from the same parent, verifies
ID-sorted enumeration, ambiguous convenience redo, explicit traversal to both
roots, preservation of every node, and duplicate/parent/before-root rejection.
One scratch redb cursor recovery invariant commits A then B, durably undoes to A and
reopens with B as its redo candidate, then durably `redo_to`s B and reopens at
B again. It then reaches the durable initial root, reopens with no head and the
first stroke as its only redo candidate, and redoes that stroke across reopen.

The release example below keeps two separate 3840×2160 RGBA8 buffers resident
outside `History` and performs 20,000 undo/explicit-redo cycles:

```powershell
cargo run --release -p nyatidraw-history --example root_transition_4k -- target/history-root-transition-4k.json
```

Measured on Windows 10.0.26200.9168, AMD64 Family 25 Model 33 Stepping 2:

| transition | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|
| undo root cursor | 0 ns | 100 ns | 100 ns | 400 ns |
| explicit redo root cursor | 0 ns | 100 ns | 100 ns | 400 ns |

Pointer identity for both 33,177,600-byte buffers remained unchanged and the
transition copied zero pixel bytes. The 0ns p50 is below the timer's observable
granularity, not a claim that execution takes literally no time. This artifact
measures core root cursor cost only; it does not include tile hydration, GPU
cache invalidation, compositor work, UI command routing, or present.

No new dependency was added. Focused fmt, check, Clippy with warnings denied,
the history core test, and the branch-reopen recovery invariant pass.

## Consequences and open boundaries

Branch metadata remains resident rather than being reclaimed on divergence.
Future compaction must be an explicit project operation with reachability and
recovery guarantees; ordinary append must not perform hidden GC.

The redb reopen path validates and reconstructs the full bounded child index;
a cursor-only transaction can durably select any persisted history snapshot
without changing artwork. Branch naming/UI, root hydration for an actual
restored frame, and end-to-restored-frame p95 remain open.
