# W1/W3 paint correctness acceptance

Local working tree at verification time; no commit, push, tag, release, or
installation. All fixtures below are synthetic scratch artwork.

## Alpha-locked fill and line preservation

Before the fix, alpha-locked bucket fill applied source-atop to both the
connected fill region and the AA/expansion backing fringe. With current-layer
sampling, filling a translucent green interior red also turned its opaque black
boundary from `[0, 0, 0, 255]` into `[255, 0, 0, 255]`. The new regression failed
on that exact comparison before the production change.

Alpha lock now disables backing expansion: only the connected region is
recolored, retaining its original alpha. Backing is a destination-over operation
whose purpose is to add coverage beneath boundary pixels; it cannot add coverage
when alpha is locked. Gap closing and tolerance still determine the connected
region independently.

The table-driven core regression covers active, reference, and visible sampling,
with AA-only, one-pixel expansion, and four-pixel expansion plus AA. It checks
that `[0, 128, 0, 128]` becomes `[128, 0, 0, 128]`, black target boundaries and
reference pixels remain exact, and transparent exterior stays empty.

`native_canvas::flat_fill_acceptance::alpha_locked_fill_fresh_reopen_preserves_barriers_and_translucent_alpha`
extends the existing native acceptance module. It executes the existing fill
gesture and edit worker, persists a scratch NTDR, closes the session/database,
and starts a fresh test process to compare every target pixel, other layers,
content root, layer tree, and canvas. Both parent and fresh child passed.

Verified on Windows with the debug/unoptimized test profile:

- `cargo test --locked -p nyatidraw-brush -p nyatidraw-stroke -p nyatidraw-paint-cpu --lib`:
  3 brush, 3 stroke, and 37 paint CPU tests passed.
- `cargo test --locked -p nyatidraw-desktop flat_fill_acceptance -- --nocapture`:
  both native fill tests passed, each including its own fresh child process.
  The existing reference-fill test also checks selection clipping, Undo/Redo,
  and decoded PNG equivalence after reopen.
- `cargo clippy --locked -p nyatidraw-brush -p nyatidraw-stroke -p nyatidraw-paint-cpu --all-targets -- -D warnings`:
  passed. Sandbox configuration lookup initially returned OS error 5; the same
  scoped command passed with approved escalation.
- Scoped rustfmt passed with approved escalation after the sandboxed attempt
  returned OS error 5. No new dependency was added.

These are correctness results, not performance measurements; no p50/p95/p99 or
first-visible-pixel claim is made. Native test compilation still reports the
existing vendored Wry lifetime warning and sandbox hard-link fallback warnings.

## Review boundaries

The existing short-stroke tests cover pressure arriving before spacing, zero
pressure, invisible presets, pencil taps, and legacy replay. No brush engine
version or recorded replay semantics were changed in this pass.

## Native live-input dab bounds

Native Move, End, and synthetic shutdown End now use an explicit live evaluator
entry point with 65,536 dabs per segment and 1,048,576 per stroke. Preflight uses
the brush's existing spacing and retained arc-length carry. Nonfinite distance
and oversized segments latch discontinuity; exact emission accounting includes
the first-contact fallback. A rejected segment publishes no dabs. Native input
retains its existing whole-stroke cancellation and clean-Begin recovery path.

At minimum spacing of 0.25 pixels, an 8192-by-8192 diagonal needs approximately
46,341 dabs and fits the segment ceiling. These ceilings bound evaluation work
and allocations; they are not a latency or physical-pen acceptance claim.
Web's existing stricter 2,048 dabs per update and 262,144 per stroke remain
unchanged, as do pencil's existing engine limits.

The legacy evaluator/replay entry point imposes no new round limits. Applying
the pencil cap of 4,096 dabs to saved round streams would reject segments longer
than 1,024 pixels at minimum spacing; that approach was rejected. Legacy replay
of unusually large round streams therefore retains its existing resource risk.

After the live-input change, `cargo test --locked -p nyatidraw-brush --lib`
passed all six tests, including minimum-spacing 4K/8K diagonals and exact
live/replay equality for round engines 1, 2, and 4; over-budget Move/End,
nonfinite input, fallback accounting, latched tail rejection, and fresh-token
admission; and exact replay of a 65,538-dab recording above the live segment
limit. `cargo clippy --locked -p nyatidraw-brush --all-targets -- -D warnings`
also passed. Native compilation/full integration after this follow-up is
reported by the main verification lane.

These are deterministic core and native persistence checks. They do not prove
physical pen behavior, display latency, native window interaction, or completion
of the W0 reference illustration. Advanced enclose fill and line-aware overflow
prevention remain deferred.
