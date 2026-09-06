# ADR-0032: Reuse mutable CPU tile buffers during history adoption

Status: implemented; installed comparison and restart acceptance pending.

After ADR-0031, unchanged GPU tiles survived, but every history adoption still
allocated and copied the complete CPU tile cache. In the 902-tile 4K fixture
that recreates 59,113,472 bytes of pixel buffers, even when only 56 GPU tile
uploads are required. Changed GPU uploads also made temporary per-tile copies.

The renderer now reconciles its mutable CPU cache with the validated immutable
snapshot: remove missing keys, copy changed pixels into surviving allocations,
retain equal bytes, and allocate only newly appearing keys. The resulting map
must exactly equal the whole snapshot, including signed tiles and restored
layers. It does not decide durable history, modify stored objects, or bypass
the existing input pause/adoption barrier. GPU uploads borrow immutable tile
bytes directly; deleted tiles use one static 64 KiB transparent tile.

One table-driven artwork invariant covers add, changed/unchanged tiles, removed
keys, restored layers, signed coordinates and undo to an empty map. All 16
desktop tests and all-target/all-feature workspace Clippy passed. No dependency
or UI tests were added. The existing opt-in performance recorder gained a
nested `history_cpu_snapshot` span; its 18 stages use 288 KiB of fixed histogram
bins per participating thread when enabled. Nested spans must not be added to
their parent to claim end-to-end latency.

A fresh pre-change installed session on 2026-09-06 repeated the ten UI Undo/Redo
pairs on a separate copy of the historical 4K scratch. All 20 adopted, followed
by normal Save/Close and exact reference artwork/history/tree/page/PNG checks.
The `history_adoption` CPU p50/p95/p99 upper bounds were
45.055/48.369/48.369ms, min 38.692ms and max 48.369ms. All samples include the
first restoration; no warmup was excluded. Logs are under
`target/history-cpu-reuse/before/`. This excludes command/worker delay and
subsequent composition/present, and cannot establish the Undo latency goal.

Host: Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc DX12, pinned-DX
release; 2096×1458 native surface, scale 2, configured 2880×1800/120Hz display.
The original 3840×2160 two-raster, 32-stroke, 33-history fixture retains its
recorded historical colors. Other host activity is uncontrolled; protected
download processes/files were not inspected or modified. Build/test work was
finished before collecting the UI samples. Repeat after installation and
verify actual reopen before accepting this optimization.
