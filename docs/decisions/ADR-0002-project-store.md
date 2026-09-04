# ADR-0002: Provisional redb project store

- Status: **Conditional — Sprint 2 headless durable slice accepted**
- Date: 2026-09-01

## Decision

Use a small redb-backed project-store spike behind `nyatidraw-project`'s
backend-neutral open policy. The proposed exact dependency is `redb = 2.6.3`.
This is not yet a production-format commitment; the decision remains reversible
until the Sprint 0 storage and recovery checks are complete.

The repository policy is explicit:

- A missing `.ntdr` path or an existing exactly zero-byte regular file may be
  initialized by the backend.
- An existing non-empty file is opened and validated, but is never replaced by
  initialization. Any redb-open failure or missing/incorrect marker is reported
  as `InvalidNonEmpty`, preserving the original bytes.
- A newly initialized database writes `meta/schema_version = 1` as an explicit
  schema marker. The marker is intentionally not Rust-memory-layout serialization.

`nyatidraw-project::ProjectRepository` is the backend-neutral commit/load seam.
It accepts only one immutable `ProjectCommitBatch` at a time and reconstructs
only the current durable head. The redb backend writes that batch in one
`Durability::Immediate` transaction; no UI, CLI, or history implementation
depends on redb types.

## Evidence from the spike

Scratch-file tests cover initialization and reopen of an empty file, creation
and initialization of a missing file, and rejection of non-empty invalid bytes
without changing those bytes. Tests use process/atomic-sequence-qualified paths
and do not touch user artwork.

The crate is now a workspace member and `redb 2.6.3` remains exact-pinned.
ADR-0005 extends this spike with one immediate-durability transaction for a
sealed stroke, exact before/after tile objects and roots, history/snapshot
records, close/reopen validation, and corruption rejection. Focused compile,
Clippy, and recovery tests pass.

The focused scratch-only recovery matrix uses an actual populated redb write
transaction at two explicit boundaries. Dropping it immediately before
`transaction.commit()` reopens the prior durable head. Returning an injected
error only after redb confirms the immediate commit reopens the new head, which
models an ambiguous caller outcome without claiming the data was lost. The same
table removes and corrupts the tile, root, stroke, history, and snapshot-head
records independently; each reopen returns a record-scoped `Corrupt` error.
The pre-existing non-empty-invalid-file test continues to assert byte-for-byte
preservation.

`crash_recovery_probe` adds actual subprocess evidence over the same real
closed-stroke fixture. Its feature-gated diagnostic seam creates and syncs a
stage file only after the child has reached a populated pre-commit transaction
or after `Durability::Immediate` commit returns. The parent verifies the staged
PID exactly matches its spawned child, kills only that child, then reopens the
scratch project. A pre-commit kill reopens the baseline snapshot; an
after-durable-commit kill reopens the newly committed snapshot; both roots and
flattened pixel hashes are checked. The seam is unavailable without the
`nyatidraw-project-redb/diagnostic` feature, so normal/release production builds
do not expose a transaction pause control.

## Not yet verified

- Large database recovery and redb `quick_repair` behavior.
- Concurrent writer/process behavior and lock/error policy.
- Windows file replacement semantics and directory/file durability guarantees.
- Power-loss interruption, drive/controller write-cache behavior, and any
  broader file/directory fsync protocol. The subprocess probe is only observed
  process-kill recovery for the two stated redb transaction boundaries; it does
  not establish a sudden-power-loss guarantee.
- Release-build performance, startup cost, and binary-size impact.
- Compatibility across future redb versions and schema migrations.

## Reversal conditions

Replace or revise this backend decision if workspace integration exposes an API
or durability incompatibility, if crash/recovery tests cannot preserve the last
durable snapshot, if concurrent-writer behavior cannot be made explicit and
safe, or if Windows durability/replacement requirements are not satisfiable.
The backend-neutral project policy and explicit schema envelope requirements
remain independent of that reversal.
