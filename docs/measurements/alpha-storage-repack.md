# Alpha storage repack acceptance

## Why container-only migration was insufficient

A user-provided 183×205 sprite exported to a 16,659-byte PNG while its already-v3
project occupied 1,978,368 bytes. All nine stored tile objects were still raw:
590,292 bytes of encoded values versus 11,830 with the existing lossless codec.
The project also retained fifteen history nodes and 122,745 bytes in the strokes
table (including keys). Compacting the old DB alone did not reduce file size.

## Change

Automatic alpha conversion now rebuilds all typed tables in a fresh DB, applying
lossless tile compression and recording `tile_storage_revision=1`. It preserves
all tile identities/pixels, non-tile records, history, layer/page state and
off-page artwork. A canonical full-record digest is checked before publication.
The marker prevents repeated rebuilds; it is separate from the codec capability
bit, which only establishes that a reader understands compressed values.

## Actual adapter result

- Windows 11, local NTFS, release CLI using production `ProjectDb`/redb 4.2.0.
- Source was copied to an ignored workspace acceptance directory first.
- First `validate` process automatically repacked the copy; a second process
  reopened it successfully with the same content root and no second backup.
- Project: **1,978,368 → 253,952 bytes (248 KiB), about 87.2% smaller**.
- Backup: exact original 1,978,368 bytes. Original user source hash unchanged.
- This is one artwork size/reopen acceptance, not a latency benchmark, physical
  pen test, power-loss test, or a promise that an editable project matches PNG size.

Retained semantic stroke samples and database page overhead still occupy space.
They are not removed or rewritten by this patch. A backup consumes additional
disk space intentionally. Conversion temporarily needs the source, its staging
copy and the fresh DB; disk-full publication behavior is not hardware-tested.

## Subsequent editing growth

A later read-only source inspection (on a scratch copy) found 987,136 bytes
(964 KiB), 42 history nodes, and 27 tiles. All tiles were already compressed,
occupying 18,775 encoded value bytes. The strokes table occupied 317,592 stored
bytes including keys. Scratch compaction reduced the DB to 593,920 bytes
(580 KiB). This is retained edit data plus database allocation/fragmentation,
not a return to raw tile storage. The user project was not compacted or rewritten.
Per-close full compaction and a semantic-stroke codec are not added by the layer
drag patch; those require a separate size/latency and recovery decision.
