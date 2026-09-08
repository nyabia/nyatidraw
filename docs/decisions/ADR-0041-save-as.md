# ADR-0041: Save As preserves the closed project and its complete history

문서 기준 시각: 2026-09-08T21:24:30+09:00

Status: implemented; focused tests and Windows interactive/independent-process Save As acceptance passed.

## Decision

The Windows file controls offer **다른 이름으로 저장** for a new `.ntdr` and
paired `.png`. Both names must be unused, including empty files, directories,
and dangling links. Existing artwork is never replaced by this operation.

Admission stops atomically across native samples and semantic commands. The
last canvas frame processes already admitted input and commands using their
normal revision/busy rules. A dedicated worker drops the old canvas, joining
pending artwork, the durable writer, and PNG work. A failed project drain
retains the fatal error and original identity; an independent source PNG
failure does not prevent Save As to a healthy location.

Only after the writer has closed does the worker open the source with a
Windows read handle that denies write/delete sharing. It copies all database
bytes into an exclusively created staging directory next to the destination.
This retains every immutable history branch, not just the current visible
snapshot. The staged database is reopened and its complete history, canvas,
and layer metadata validated. Missing legacy layer metadata uses the same
default tree as ordinary reopen. PNG rendering uses these durable CPU tiles.
Both files are synchronized and the staged database is closed before publish.

Publication uses no-clobber hard links, PNG first and project second. This
requires a filesystem supporting hard links, such as NTFS; unsupported
filesystems fail without modifying the source. There is no cross-file atomic
transaction: a crash or late collision can leave a complete PNG (or completed
pair) at the selected name. Those final files are retained rather than deleted
after another process might have adopted them. Only the owned staging files
are cleaned up. A process crash may retain that hidden staging directory.

The active path/title/update-restart target changes only after successful pair
publication and target reopen. A copy/export/open failure reopens the original
project. A failed durable drain does not clear its failure latch. Close and
additional activations are rejected with visible feedback while Save As runs;
the UI message loop remains alive for worker completion. Unexpected teardown
joins the worker. This is not physical-input or first-pixel evidence.

Ordinary activation now also drains admitted work and checks project/PNG
failure before clearing projection state or changing document identity.

## Validation

Focused invariants cover preservation of the original bytes and all 32
history snapshots after undo, plus non-overwrite of invalid/empty projects and
PNG collisions and Save As of an untouched project. These are storage/reopen
checks in one test process, not interactive installation or separate-process
acceptance. No new dependency was introduced.

On 2026-09-08 the DX release app's real Windows Save As dialog created a
Korean/space-named sibling pair from `desktop_save_as_fixture`. The title
switched to the copy, and ordinary Close joined the writer. A separate verifier
process confirmed source bytes unchanged, all three history snapshots including
an off-current branch, signed off-page tiles, metadata, and exact PNG bytes.
This does not prove power-loss or physical-pen behavior. See the
[integration record](../status-plan-2026-09-08.md).

Failure restoration retains the source PNG error, but rebases its status to
generation zero after the old writer has joined; otherwise the new writer's
generation one Save could be incorrectly rejected as stale. A focused invariant
covers small, large, and maximum previous generation values.
