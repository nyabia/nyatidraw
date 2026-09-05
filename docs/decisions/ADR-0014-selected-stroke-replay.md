# ADR-0014: Selection coverage belongs to the sealed stroke

Status: accepted for durable CPU replay; GPU/desktop connection pending.

## Decision

A brush/eraser stroke captures an immutable binary selection at Begin. Its
coverage must survive storage and affect deterministic replay, even after the
session selection changes or disappears. `StrokeCommit::seal_with_selection`
and the matching headless preparation method retain the mask with the samples.
The existing entry points continue sealing unrestricted strokes.

CPU materialization skips tiles that cannot intersect the mask, replays dabs
with the existing brush/eraser equations, and retains exact before bytes outside
coverage. Selection never extends beyond its finite page; signed off-page tiles
and boundary padding remain untouched. Brush and eraser share this rule.

Coverage is encoded as row-major, least-significant-bit-first packed bits with
explicit u32 width/height. Maximum coverage is 16,777,216 pixels (2 MiB packed).
Decoding validates dimensions, exact length and zero unused tail bits before
allocating the decoded page, and recomputes the selected count. Stroke identity
uses a separate selected-stroke hash domain containing dimensions and bits.
Unrestricted strokes retain their previous hash domain, byte encoding and IDs.

The stroke payload appends `NYSEL001`, dimensions and canonical bits only when
coverage exists. Decoding old payloads yields no mask. Unknown extensions,
truncation, noncanonical bits and a mask that disagrees with the stored ID fail.
No external dependency or record envelope version changed.

The first selected-stroke transaction upgrades the project marker to 3 and
initializes immutable layer history if the project still uses marker 1. Marker
3 includes marker 2's layer-history contract. The format change, pixels, stroke
record, history and cursor commit together. Aborting leaves the old format and
head intact. Undo and later unselected edits must not downgrade the marker:
selected strokes can remain on redo branches. Readers supporting only marker
1/2 reject the project before modifying it.

Coverage is currently inline in each selected stroke record; repeated large
selections increase project size. Packed masks are bounded but not yet interned
as shared content objects. CPU unpacking/hash work belongs on the writer, never
per raw sample. Selection session state itself is not restored by reopening.

## Evidence

Two core risk tests were added (workspace total 68):

- Table-driven selected brush/eraser replay preserves unselected pixels,
  another layer, negative tiles and boundary padding, including empty masks.
- Storage/wire checks retain legacy identities, reject malformed/altered masks,
  roll back a failed first upgrade, reopen exact coverage and retain marker 3
  after Undo.

`selected_stroke_reopen_probe --release -- --legacy-reader <old-reader.exe>`
uses ten separate child processes: edit then independent verification for
baseline, selected brush, selected eraser, Undo and Redo. A 17x5 page has an
8-column brush mask, a shifted 8-column eraser mask, opaque padding and a signed
negative tile. Full-strength dabs cover the page; expected RGBA pixels are
constructed directly from the column ranges, without using replay to compute
the expected image. Exact tile/tree/history/stored coverage and PNG comparisons
passed on Windows with the CPU backend in release profile.

The preserved pre-change `desktop_edit_fixture.exe` rejected the selected file
as `InvalidNonEmpty` after brush, eraser, Undo and Redo. Entire file bytes before
and after each rejected open matched. This directly checks a real old reader.
Logs: `target/selected-stroke-release.log`, `selected-stroke-focused.log`,
`selected-stroke-tests.log`, `selected-stroke-clippy.log`.

## Next gate

Upload the same immutable selection into the GPU brush/eraser path, including
sparse boundary tiles and Cancel restoration. Compare GPU pixels with selected
CPU replay, then connect native selection gestures/overlay and enable desktop
tools. Desktop still declines brush input while a semantic selection exists;
this commit does not claim finished selected painting or physical-pen evidence.
