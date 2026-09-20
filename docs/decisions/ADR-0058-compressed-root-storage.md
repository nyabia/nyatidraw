# ADR-0058: Lossless root-index compression and one-time alpha repack

## Decision

Keep redb and all 128 retained history operations. Compress immutable root
manifests as well as tiles; do not substitute a flattened image, delete branches,
change pixel precision, or add a second authoritative archive.

An inspected drawing had 986 tile objects, all already Zstd-compressed to
2,725,794 bytes. Its 113 full root-index records occupied about 7.55 MB before
database overhead. The 39,456,768-byte container shrank to 23,334,912 bytes by
compacting a private copy alone. This pointed to repeated indexes plus database
allocation, not missing pixel compression.

## Physical codec and compatibility

`project-redb/root_codec.rs` stores a beneficial compression as `NYROOT01`, an
eight-byte little-endian decoded length, and one Zstd level-1 frame. It compresses
the complete existing `ContentRoot` envelope. Uncompressed legacy roots remain
readable; new values fall back to raw when compression fails or is not smaller.
Canonical envelope bytes, payload checksums, content-root identity, signed tile
coordinates, layer IDs and referenced tile hashes are unchanged.

Compressed decoding permits 112 bytes through 64 MiB, checks the exact frame
boundary, rejects trailing/truncated data, and verifies the exact decoded length
and existing envelope checksum. Normal manifest decoding still validates the
canonical root hash. Larger legitimate legacy raw manifests remain raw; the
64 MiB rule is a decompression bound, not a new global canvas-size limit.

The redb adapter owns storage capability `0x4000`. New initialization and root
commits publish it atomically; layer/page/history metadata transitions preserve
it. Older applications do not recognize this bit and reject the marker before
writable open. The backend-neutral root wire format does not change.

Direct backend dependency: `zstd = 0.14.0`, default features disabled. This is the
same exact dependency already used by the tile wire codec, not another codec or
database runtime.

## Alpha conversion

`meta/tile_storage_revision=2` marks the current physical layout. Missing/revision-1
files use the existing ADR-0049 staged-copy conversion once. Revision 2 needs no
repack; unknown revisions fail before writable open, including builds without
automatic legacy migration.

The copy builder losslessly re-encodes tile and root values and keeps every other
record byte-exact, including editor tool preferences. Its typed-table fingerprint
normalizes root envelopes in addition to tile envelopes and excludes only known
storage markers/capability bits. Before publication it validates current artwork,
history and layer/page metadata, and requires the full normalized fingerprint to
match. Conversion does not run history retention or regenerate PNG.

Only the new private database is compacted. Automatic replacement retains the
existing no-clobber exact original `.bak`; explicit `migrate-copy` leaves its source
unchanged. There is no full repack on every Save, Close, or later ordinary Open.
The existing alpha migration module remains removable as described in ADR-0049.

## Evidence

The coordinated all-features workspace test suite passed. Focused core cases
cover raw/compressed identity, corrupt and oversized frames, every retained cursor
after conversion/reopen, exact source backup, optional editor-state preservation
and future marker refusal. Existing 128-step process-restart and
migration-interruption cases also passed with the new codec.

Actual-drawing acceptance used only a workspace `.nyatidraw` copy through the
root-repack test's explicit scratch environment hook:

| Measurement | Bytes |
|---|---:|
| Original container, read-only diagnosis | 39,456,768 |
| Previously compacted private copy | 23,334,912 |
| Root-compressed rebuilt copy | 8,712,192 |
| Canonical root value bytes | 7,545,605 |
| Stored compressed root value bytes | 2,840,009 |

The rebuilt copy is 8.31 MiB: 77.9% smaller than the original container and 62.7%
smaller than the compacted copy. All normalized table records matched, all 128
history operations and 129 retained tile/layer/page cursors validated, and the
second reopen produced the same records and cursors. The source copy stayed
byte-exact; a separate final read-only hash of the original artwork matched the
pre-diagnosis hash. No original artwork was migrated or compacted.

The exact acceptance test passed in the debug test binary; that duration is not
an application startup, ordinary save, or performance measurement. These results
demonstrate lossless storage reduction for this drawing, not a universal ratio.
