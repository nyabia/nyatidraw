# ADR-0027: Scratch-only paced desktop performance workload

Status: accepted for reproducible software measurements; no physical-input claim.

## Decision

The installed release can run an opt-in 3840×2160 workload through the existing
bounded `LiveInkBridge` admission path. A dedicated producer sends 32 horizontal
64 px round-brush strokes, each with 121 samples at nominal 240 Hz, waiting for
each durable history adoption before the next stroke. This deliberately does
not measure sustained overlapping stroke-commit backlog. Samples bypass Win32
and WebView and use constant pressure. This is not a physical pen, mouse, OS
event-to-present, or first-visible-pixel benchmark.

`baseline` saves only after drawing. `export` requests a real 4K PNG export before
every stroke and begins input after its Running notification. Export is neither
paused nor artificially prolonged. The last Save must reach a strictly newer
Current generation. The writer's existing export interval tags measured spans;
both active and inactive portions of each export scenario are reported.

Admission requires both performance environment flags, an exact
`performance-scratch.ntdr` filename, its sibling
`.nyatidraw-performance-scratch` marker, the exact canvas, and fresh initial
history. The producer additionally waits up to two minutes for the project's
`.performance-start` sibling so the operator can first activate and observe the
window. Close or project activation cancels the producer. It runs once per
process and stops on rejection, workspace failure or a bounded wait timeout.
There is no global process cleanup or access to Downloads.

The fixture has two visible raster layers: locked opaque background and ink.
The separate-process verifier checks all tiles, dimensions/PPI, unchanged tree,
snapshot 33/history 33, and PNG bytes against offline deterministic replay of
the same workload. The replay and compositor are shared production algorithms;
this checks paced-input/persistence/reopen/export agreement, not an independent
rasterization oracle. The log collector also requires all 3,872 admitted samples
in the expected 32 closed streams and normal writer join.

## Reproduction

```powershell
cargo build --locked --release -p nyatidraw-desktop --example desktop_performance_fixture
& tools/install-dev.ps1
& tools/start-desktop-performance.ps1 -Mode baseline -Run 1
# Activate/observe NyatiDraw with Computer Use before creating this marker:
Set-Content target/performance-foreground/baseline-1/performance-scratch.performance-start 'start'
# Wait for event=complete, close NyatiDraw normally, then:
& target/release/examples/desktop_performance_fixture.exe verify target/performance-foreground/baseline-1/performance-scratch.ntdr *> target/performance-foreground/baseline-1/verify.log
```

Repeat with fresh run numbers and `-Mode export`. Existing run folders are
refused. Supply actual hardware/OS/backend/build/background-load context in
`target/performance-foreground/metadata.json`, then collect per-run rows:

```powershell
python tools/summarize-desktop-performance.py target/performance-foreground target/performance-foreground/report.json
```

No warmup is silently removed. Whole-session frame rows include startup and idle
redraws; matching input rows are used for interference comparisons. Histogram
definitions and phase-boundary limitations remain those of ADR-0026. Percentiles
are never averaged across runs. Scheduler lateness is recorded independently of
the nominal input interval. User background downloads remain untouched and are
an uncontrolled workload, so this is not an isolated CI baseline.

## Initial acceptance

Two exploratory installed runs in `target/performance-paced/` completed both
modes, each with all 32×121 samples and exact separate-process tile/history/PNG
agreement. They started input before foreground activation and therefore are
excluded from matched foreground comparisons. All-target/all-feature Clippy
with warnings denied and pinned-DX release installation passed. No new unit
tests or dependencies were introduced for this integration driver.
