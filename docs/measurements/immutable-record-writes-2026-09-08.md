# Identical immutable record writes

The project writer now skips a tile/content-root insert only when the entire
stored envelope equals the canonical bytes it was about to write. This removes
redundant database replacement calls without changing the record format,
content hashes, retained history, or immediate transaction durability. Encoding
and comparison still cost CPU time. Key presence alone is deliberately not
enough: a damaged existing value is replaced by canonical bytes, preserving the
previous writer's behavior.

## Bounded evidence

Windows 11 Pro 10.0.26200, AMD Ryzen 7 5800X3D, redb 2.6.3, Cargo test/dev
profile (not release), CPU/storage only; GPU/backend presentation not involved.
The same two-stroke scratch regression workload ran with unconditional insert
and with the final shortcut. Each variant covered a damaged tile and a damaged
root, committed the next stroke, dropped the database, then reopened and checked
exact artwork and both history nodes. Final file lengths were identical:

| Scratch case | Unconditional insert | Identical-envelope shortcut |
| --- | ---: | ---: |
| Damaged tile repaired by next commit | 4,747,264 bytes | 4,747,264 bytes |
| Damaged root repaired by next commit | 4,747,264 bytes | 4,747,264 bytes |

One run per case/variant; this is a small recovery fixture, not representative
artwork or a file-size distribution. No latency p50/p95/p99 was measured and no
speedup or file shrinkage is claimed. The tests remove only their generated
scratch files. No existing artwork was used or rewritten.

Final validation: project-redb all-feature tests **15 passed**, package
all-target/all-feature Clippy with warnings denied passed. The existing
`crash_recovery_probe` also passed its subprocess-kill/reopen checks before and
after durable stroke and layer commits, comparing artwork and metadata. These
are storage recovery checks, not installed GUI, physical pen, or cold OS-cache
measurements.

## Oversized NTDR investigation remains open

Every unique historical tile currently stores 65,536 raw pixel bytes plus a
52-byte envelope. Roots retain complete key/hash manifests; semantic strokes
also retain samples and optional selection coverage. Same hashes already share
one table key. Actual per-table sizes and database allocation slack must be
measured on a representative scratch copy before attributing an oversized file
to any one source. The earlier per-load shared-object optimization affected
read/RAM work, not this on-disk encoding.

A subsequent candidate is tile-only LZ4 block compression when smaller than
raw. The envelope already has a codec byte and uncompressed length, but existing
readers accept only codec 0. A new reader must keep raw compatibility and use a
fixed 65,536-byte output buffer, exact length, checksum and content-hash checks.
New compressed writes need an explicit project schema gate; older readers
cannot read them. No codec, dependency, or migration is added in this change.
The [LZ4 implementation documentation](https://github.com/PSeitz/lz4_flex)
documents decompression into caller-provided buffers.

Compression trades additional encode/decode CPU work against fewer disk bytes.
Reopen also loads roots and replays the current semantic stroke, so its latency
must be measured separately. Compressing newly written objects will not shrink
old raw history objects automatically. History deletion, close-time compaction,
and in-place migration are not part of this change.
