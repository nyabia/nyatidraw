# ADR-0049: redb 4 and temporary alpha conversion

## Status

Implemented in the actual `ProjectDb` adapter and compiled through the desktop
and CLI. The user explicitly authorized automatic alpha conversion and wants
legacy support removed later, not maintained as a permanent compatibility stack.
No installed binary, user artwork, association, release tag or push was changed
while implementing this gate. SQLite remains a fallback; Persy is research-only.

## Dependencies and storage

- Production database: `redb = 4.2.0`, existing typed tables and immediate durable
  transactions, not the normalized one-table comparison adapter.
- Temporary default feature `legacy-migration`: `redb-legacy = 2.6.3` and direct
  `blake3 = 1.8.7` for streaming record fingerprints. Domain crates remain neutral.
- Tile wire codec/reference retention remain unchanged. Conversion now decodes
  and losslessly re-encodes every tile into a fresh redb database, including
  files already converted to v3 by alpha.6. Non-tile records remain byte-exact,
  except the compression capability bit and `meta/tile_storage_revision=1`.
  It does not prune history, change colors or regenerate sibling PNGs.
- No full compaction on ordinary save, existing-file open or close. A brand-new
  metadata-only DB is compacted once at initialization to remove redb's initial
  allocation. A one-time alpha storage rebuild compacts its private new DB once.

## Automatic alpha conversion

`ProjectDb::open` detects redb's `UpgradeRequired` result or the absence of
`meta/tile_storage_revision=1`, then invokes the isolated `migration.rs` adapter.
New files start with the marker; unknown marker versions fail closed. The
writable crash-recovery path applies the same check after validation:

1. Open the original regular file for read and lock it. On Windows, deny writers
   while permitting the final replacement; take an exclusive byte-range lock to
   serialize cooperating DB openers/converters. Reject symbolic-link sources.
2. Exclusively create a sibling staging directory, copy the source and sync the
   copied file. Never open the original with redb 2.6's writable database API.
3. Fingerprint all typed tables and rows in the copy. Unknown tables or multimaps
   fail closed. Upgrade v2 copies to v3 using the legacy library when necessary.
   Fingerprints canonicalize tile envelopes to decoded raw pixels and ignore only
   the compression capability bit and storage-revision marker.
4. Open with redb 4 and compare the content fingerprint. Stream all rows into a
   fresh sibling DB, re-encoding tile values with the existing lossless codec.
   Set the two storage markers, durably commit, compact and close the rebuilt DB.
   Validate the application marker, current artwork, history and layer/page
   metadata. Do not run open-time retention during conversion. Compare records
   again before publication and sync the completed copy.
5. Create a no-clobber hard-link backup of the original at
   `<project>.pre-redb4-<nonce>.bak`, then atomically rename the completed copy
   over the original project path. Hold the source lock through publication.
   Keep the backup on success and on publication failure. Never replace a backup.
6. Open the new container through the normal adapter. Future opens see the
   storage marker, need no conversion and create no additional backup.

Hard links and atomic same-filesystem rename are required; unsupported filesystems
fail rather than falling back to truncating/overwriting the destination. Verified
on Windows/local NTFS only. This is not proof against hostile external renames,
power loss, cloud synchronization or network-filesystem behavior.

A killed pre-publication conversion may leave its private staging directory.
That directory is never authoritative. A subsequent open can retry conversion
from the intact original. We deliberately do not add a broad recursive orphan
cleanup job during app startup. Backups are user-recoverable and not auto-deleted.

The CLI also has `migrate-copy <source.ntdr> <new.ntdr>` for explicit conversion
without replacing the source. It rejects early/late destination collisions and
does not generate PNG. This command is alpha tooling, not a long-term promise.

## Validation without modifying rejected artwork

redb 4 writable close may update allocator metadata even when no application
records were changed. Existing clean files therefore receive a read-only
application validation before opening the writer. Invalid application records
are rejected without changing any original bytes; the existing corruption tests
continue to enforce byte-for-byte preservation.

Valid opens may change database bookkeeping. The raw-codec test verifies exact
decoded artwork, a smaller encoding and the original byte-exact backup. Without
the temporary migration feature, it checks unchanged wire records. An unclean database can require redb's
writable crash-recovery path; full byte preservation during failed allocator
repair is not established. The read-only preflight currently adds a second
application validation/materialization pass; large-document startup is a remaining
measurement/optimization gate, not hidden behind the small-fixture results.

## Evidence

The storage-rebuild follow-up passes the existing core suite with genuine raw
v2 fixtures, 128 undo/redo steps after child-process restart, and a raw v3 fixture.
A private 183×205 sprite copy falls from 1,978,368 to 253,952 bytes after automatic
open and a second-process reopen. All nine tiles and fifteen history nodes remain.
See [repack acceptance](../measurements/alpha-storage-repack.md). The original
artwork is untouched by acceptance; only its scratch copy is converted.

- 19 project-redb core tests pass, including two new migration safety tests.
- Actual v2-container fixture passes automatic conversion in a fresh process,
  followed by all 128 undo/redo steps, signed off-page pixels, shared tiles,
  per-snapshot layer names/page sizes and continued branch edits.
- Locked source, invalid application marker, injected pre-publication failure,
  late destination collision, and exact original-backup checks pass.
- Real child-process kills before/after conversion publication leave an intact
  original or a complete replacement. The next open succeeds and exact backup
  bytes match. Process termination is not a power-loss simulation.
- Existing release stroke/layer commit-boundary kill probe passes with redb 4.
- Workspace tests pass, including PNG pairing/export/reopen and the existing
  child-process reopen gates. The explicit scratch-registry test remains ignored.
- Desktop compiled with the upgraded adapter; installed GUI/manual acceptance
  has not been performed. See [measurements](../measurements/redb4-native-adapter.md).

## Removal plan

When the alpha compatibility window closes, delete rather than extend:

1. `crates/project-redb/src/migration.rs`, its module/export, its auto-open branch
   and the migration-only hooks in the retained-history test.
2. The `legacy-migration` default/feature entries and `redb-legacy` / migration-only
   direct `blake3` dependencies; regenerate Cargo.lock normally.
3. The CLI `migrate-copy` command/help entry and its explicit migration feature.
4. The obsolete migration-specific error/help wording and alpha instructions.

Keep normal current-format corruption/recovery tests, typed-table validation,
compression and history retention. `--no-default-features --lib` currently builds
without the old engine, proving this compatibility code is separable. Removal
does not authorize deleting users' `.bak` files or old artwork.
