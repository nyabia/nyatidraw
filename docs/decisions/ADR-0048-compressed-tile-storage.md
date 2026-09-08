# ADR-0048: Lossless tile storage and the compact-container gate

## Status

The candidate-selection and explicit-only migration policy below is historical.
[ADR-0049](ADR-0049-redb4-alpha-upgrade.md) adopts redb 4.2.0 and the user's later
authorization for automatic, temporary alpha conversion. The wire codec and
reference-retention decisions in this ADR remain in effect.

Tile compression and indexed object retention are implemented on the existing
redb backend. The original SQLite-only next-candidate decision is superseded by
the current redb/SQLite comparison below. Current redb 4.2.0 and SQLite have passed
a differential record-transaction/reopen/commit-boundary kill probe, not full
production-adapter acceptance. Persy remains research-only at the user's request.
At this comparison checkpoint the application still used redb 2.6.3. See ADR-0049
for the implemented upgrade, actual typed-table measurements and remaining gates.

## Decisions

- Keep the `.ntdr` extension, backend-neutral document/root/history model,
  signed off-page pixels, 128 retained operations and separate PNG export.
- Compress individual 128x128 RGBA tile byte arrays with Zstd level 1. Preserve
  internal linear/premultiplied bytes exactly; do not round-trip through a display
  color conversion. This is not AVIF or a flattened-image project format.
- Use raw storage if compression fails or is not smaller. Existing immutable
  tiles are validated and reused without recompressing on every commit.
- Do not ZIP/repack the whole project on every Save or Close. Do not add a second
  authoritative working database and synchronize it with an archive.
- Keep compression and collection in the existing single project writer, outside
  raw input and render/UI threads. Collection is not a background thread racing
  a mutable document; it is part of the same durable history transaction.
- No automatic full-file compaction on opening or closing a project.

Zstd is a general lossless codec, not an image container. PNG additionally uses
image-oriented prediction/filtering before compression; Zstd on raw tiles is not
guaranteed to beat PNG for gradients, photographs or every brush texture. The
current choice prioritizes exact internal bytes and independently decoded tiles.
Image predictors and tile-version delta chains are deliberately deferred until
representative measurements justify their complexity.

## Wire compatibility

`NYREC001` retains its envelope schema, content checksum and original byte length.
Codec 0 is raw. Codec 1 is one Zstd frame, accepted only for a Tile record with
exactly 65,536 decoded bytes. Decode uses a fixed-size destination, rejects
oversized declared lengths, truncated/trailing frames, unknown codecs, checksum
failures and wrong decoded sizes. Canonical root manifests now verify their
own content hash before object collection trusts their references.

The project marker adds capability bit `0x100`; its low metadata-history level
remains 1 through 4. New initializations and artwork commits advertise the bit.
Existing readers only accept markers 1 through 4 and therefore reject new-format
projects before writing them. Metadata-history transitions preserve the capability
at the transaction boundary. The record codec does not determine content identity.

Old raw projects can be read without a codec migration on open. A new commit
can introduce compressed tiles and sets the capability marker atomically. It does
not recompress all old blobs. The pre-existing >128 history trimming on open is
a separate policy and still applies; this ADR does not promise read-only opens
for those older over-limit projects.

## Reclamation

New projects maintain `tile_root_refs`, counting each distinct tile hash once per
immutable root (not once per coordinate). When retention discards history nodes,
their before/after roots become candidates. Current, initial-baseline and every
retained branch's before/after roots remain pinned. Only candidate roots outside
that live set are removed. Their tile references are decremented and zero-reference
tile objects are deleted within the same transaction.

This avoids a whole-database tile scan per stroke. Work still scales with the
manifests of new/evicted roots; large-document writer latency remains a measurement
gate. Full root manifests are not yet a persistent incremental tree.

Legacy files without the reference index skip object collection rather than
guessing reference counts or rebuilding all history during startup. Legacy bulk
conversion/space reclamation must be performed on a validated copy in the next
backend/migration stage. Database freed pages may be reused without immediately
shrinking the physical file.

Current export runs from the writer's materialized CPU snapshot in its FIFO;
it does not defer a bare historical root ID past collection. Any future concurrent
lazy exporter must explicitly pin roots or hold a storage snapshot for its lifetime.

## Dependencies

- Production wire codec: `zstd 0.14.0`, default features disabled; locked
  `zstd-safe 8.0.0`, `zstd-sys 2.1.0+zstd.1.5.7`.
- Existing production database: `redb 2.6.3`.
- Development-only current-redb comparison: `redb 4.2.0`, aliased `redb-current`.
- Development-only comparison: `rusqlite 0.40.2` with bundled SQLite,
  `libsqlite3-sys 0.38.2`. This does not ship a second production database.

## Evidence and next gate

See [storage measurements](../measurements/compressed-tile-storage.md). The
same compressed records originally occupied 118,784 bytes in the SQLite scratch
candidate versus 3,002,368 bytes in compacted **redb 2.6.3**. This did not establish
a limitation of current redb. Its 3.0 release substantially reduced minimum file
overhead. Do not use the old comparison to select SQLite by default.

See [current candidate measurements](../measurements/storage-candidates.md) for
redb 4.2.0 versus SQLite DELETE/FULL and WAL/FULL under identical record changes.
Current redb is sufficiently competitive to make a bounded upgrade spike the
next recommendation, before undertaking a full SQLite adapter migration. This
is not approval to replace or migrate user artwork automatically.

Next bounded implementation:

1. Evaluate redb 4.2.0 with the application's existing typed tables and full
   `ProjectDb` transaction path. The comparison uses a normalized one-table record
   layout, not a drop-in measurement of the existing adapter after a version bump.
2. Validate the v2-to-v3 format upgrade on an owned sibling copy, preserving the
   source. Check all pixels including off-page, layer/page metadata and every
   retained undo/redo state after process restart. Test interrupted conversion,
   invalid files and publication collisions. Never silently convert on open.
3. Verify PNG pairing/export, writer saturation and process-kill recovery. Measure
   normal un-compacted file sizes, startup/save/close percentiles and realistic
   small-sprite, gradient and 4K multi-layer workloads. Do not compact on every
   Save or Close merely to meet a file-size number.
4. If that gate is insufficient, implement the SQLite adapter and run the same
   acceptance. Select DELETE/FULL versus WAL/FULL explicitly: WAL needs sidecar
   lifecycle, checkpoint and live-file-copy rules, not just a faster commit number.
5. Switch production only after the selected adapter and migration pass. Until
   then, do not tag a release claiming the small-NTDR problem is solved. Do not
   build a Persy adapter unless the remaining candidates expose a concrete need.
