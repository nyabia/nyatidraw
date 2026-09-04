# ADR-0005: Deterministic CPU materialization for sealed round strokes

- Status: **Conditional — desktop CPU replay/redb slice accepted; strategy comparison open**
- Date: 2026-09-01

## Decision

Seal a closed round-brush stroke into a storage-neutral `StrokeCommit` containing
the exact before root, versioned brush snapshot, recorded evaluator metadata and
seed, premultiplied linear-light colour, affected signed tile bounds, and raw
normalized samples. Retain a hard limit of 262,144 samples and 65,536 affected
tiles per stroke.

Materialize the current round brush with deterministic CPU replay into immutable
128×128 RGBA8 premultiplied linear-light tiles. Hash tile bytes and the canonical
sorted tile manifest with tagged BLAKE3. Transparent tiles are omitted. Store
both semantic stroke and exact before/after tile roots: exact tile bytes are the
authority for reopen and history roots, while the semantic record is checked by
re-sealing and replay.

`MaterializationStrategy` names `CpuReplay`, `GpuReadback`, and
`HybridCheckpoint`, but only CPU replay is implemented. The other strategies
return `UnsupportedStrategy`; no fallback is reported as the selected strategy.

The editor uses a two-step boundary. `prepare_round_stroke` is non-mutating and
returns one immutable `ProjectCommitBatch`. The redb writer commits tile objects,
roots, stroke, history node, snapshot head, and current pointer in one
`Durability::Immediate` transaction. Only after it succeeds may
`accept_committed` advance live editor state.

The Windows desktop accepts an explicit project through the
`NAYATI_PROJECT_PATH` environment seam. Without it, the root canvas creates one
process-lifetime `Untitled (Recovery)` redb path and reuses it across GPU/custom-
paint remounts. The background closed-stroke worker owns the `ProjectDb`, resumes
`HeadlessStrokeSession::from_reopened`, commits with the existing immediate
transaction, and accepts the new session state only after commit success.
Invalid non-empty explicit input is a fatal canvas-startup error and never falls
back to another project.

The live producer uses non-blocking `try_send` plus a bounded backlog sized to
the two bounded native-input lanes. Teardown is the only blocking boundary: it
flushes that backlog, closes the sender, drains the writer, and joins it before
the repository is dropped. Renderer startup uploads reopened authoritative CPU
tiles before the first viewport render; unsupported fixed-scene tiles fail
startup rather than disappearing.

## Format and dependency record

Records use explicit little-endian encodings inside a `NYREC001` envelope with
kind, schema version 1, raw codec, uncompressed length, and tagged BLAKE3 payload
checksum. Decoder collection counts are bounded by remaining payload size before
allocation; stroke decode reuses the seal sample limit.

- `blake3 = 1.8.7` is exact-pinned with its default `std` feature. Cargo resolves
  `constant_time_eq 0.4.2` and `cpufeatures 0.3.1` for it. The upstream license is
  CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception.
- `redb = 2.6.3` remains exact-pinned in the native backend.
- Core `brush`, `tiles`, `history`, and `stroke` crates remain free of redb,
  Dioxus, wgpu, Vello, and platform-native types.

## Evidence

Focused core tests prove:

- identical sealed input CPU-materializes to exact equal tile bytes/root;
- linear history undo/redo returns the exact prior/next content roots;
- one redb immediate commit survives close/reopen with equal tile hashes and
  pixel bytes;
- a checksum-valid root envelope containing a `u64::MAX` entry count is rejected
  as project corruption before capacity allocation;
- a non-empty invalid file is rejected without overwriting its bytes.

Focused `cargo check`, `cargo test`, rustfmt, and Clippy with warnings denied pass.
These are headless correctness results, not physical-pen, input latency, GPU
readback, first-present, or crash-interruption evidence.

## Open boundaries and reversal conditions

- The GPU working texture is viewport-sized while durable storage is signed
  128×128 tiles. End-of-stroke GPU readback and hybrid checkpoint implementations
  are absent and must be benchmarked against CPU replay without silent fallback.
- Reopen validates and reconstructs the complete bounded history DAG. Migration,
  compaction, and last-valid-head recovery beyond redb's committed transaction
  remain open.
- The desktop scene is still a fixed 1024×768, two-raster-layer spike. Reopened
  tiles outside that scene fail startup; document metadata-driven scene creation
  remains open.
- The 128×128 tile choice remains provisional pending the 64/128/256 benchmark.

Reverse CPU replay as the default only if measured End-to-durable cost misses the
Sprint budget or GPU/CPU semantics cannot remain within the accepted artwork
contract. Keep the immutable commit, explicit strategy, exact tile authority,
bounded decode, and post-durability editor-advance boundaries regardless of the
chosen materializer.
