# NyatiDraw implementation rules

## Current milestone

Implement the Windows-first vertical slice through Sprint 3. Close the remaining
Sprint 1 software gates, then finish Sprint 2 durability/reopen/export and
Sprint 3 viewport, layer/composite, docking, UI-projection, and hot-path
isolation gates. macOS, vector/Vello, animation, and advanced brushes remain
out of scope until Sprint 4 or later.

Physical-pen, high-refresh display, and backend-specific hardware evidence may
remain explicitly unverified when the required hardware interaction is not
available. Never convert synthetic input, GPU submission, or a narrow probe
into physical-pen, first-visible-pixel, or cross-platform proof. Do not add
later features merely to make an earlier spike look complete.

## Architectural boundaries

- `crates/document`, `crates/input`, `crates/brush`, `crates/tiles`, and `crates/history` must not depend on Dioxus, wgpu, Vello, redb, Win32, AppKit, or Wayland types.
- UI commands never carry raw stylus samples. The native input path bypasses Dioxus state updates.
- Hot-path queues are bounded. Within the configured primary and retry
  capacities, stroke phase transitions are preserved. Exhausting both lanes
  must latch one explicit discontinuity, cancel/quarantine the partial stroke,
  and resume only from a clean `Begin`; transition loss must never be silent.
- GPU state is not the only durable representation of a closed stroke.
- Project saving and image export are separate failure domains.
- Avoid catch-all `common`, `shared`, or `utils` crates.

## Evidence

- Target numbers in `docs/performance.md` are not verified results.
- Record benchmark hardware, OS, backend, release profile, p50, p95, and p99.
- A feature that changes artwork is complete only after save, process restart, and reopen verification.
- Add dependencies only for the current spike and record the chosen exact versions in an ADR after the gate passes.

## Test policy

- Write automated tests only for core logic whose regression can corrupt artwork, lose input transitions, break deterministic brush/history behavior, or violate project recovery guarantees.
- Good test targets: signed tile addressing, pressure/sample normalization,
  bounded transition preservation plus fail-closed discontinuity recovery,
  brush determinism, content-root/history changes, invalid-file preservation,
  and migration invariants.
- Do not add tests for simple getters, data-only constructors, UI layout, framework wiring, OS wrapper pass-through, or behavior already enforced by Rust's type and ownership system.
- Validate shell/framework integration with compilation, Clippy, focused runtime acceptance, and measurements rather than a large mock-heavy test suite.
- Prefer a small table-driven invariant test over many one-case tests. Every test must name the product risk it prevents.

## Safety

- Never overwrite a non-empty invalid project file during initialization.
- Crash tests operate on scratch projects, not user artwork.
- Generated exports use a sibling temporary file and generation check before replacement.
