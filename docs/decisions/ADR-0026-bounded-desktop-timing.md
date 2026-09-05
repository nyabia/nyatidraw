# ADR-0026: Opt-in bounded CPU timing in the installed desktop

Status: instrumentation accepted by the installed-app wiring check below.
Performance gates remain open; this is not a latency acceptance baseline.

## Decision

`NAYATI_PERFORMANCE=1` enables desktop-private timing. Default runs retain no
histograms and emit no performance records. A cached flag avoids environment
lookup per sample. Recorders are thread-local, use 17 stages × 2 export phases ×
1024 fixed u64 bins (272 KiB plus small metadata per recording thread), and do
not acquire new shared mutexes or write to console/files on each frame. They
allocate once on first use. Normal writer exit and canvas destruction print
aggregate records; crash termination can lose those in-memory measurements.
Input source, stored samples, brush/history contracts and dependencies do not
change. Histogram counters saturate rather than wrap.

Durations round up to integer microseconds. Values below 16 µs have exact bins;
each higher power-of-two interval has 16 subdivisions. `p50_upper_us`,
`p95_upper_us`, and `p99_upper_us` are nearest-rank bin upper bounds, capped at the
observed exact maximum. They are not interpolated exact percentiles. Bin width
is at most 1/16 of the interval's lower boundary. Counts and min/max accompany
every row. Samples are counted throughout the session, without an unbounded
sample list or silently dropping older measurements.

Stages separate brush evaluation after dequeue, GPU operation encoding/submits,
composite encoding/submits, full CPU frame, surface acquisition, blit/present
request, history GPU adoption, committed-history projection publication, stroke
CPU replay, DB commit/session acceptance, navigator, thumbnails, export composite,
PNG encoding, and export sync/generation check/replacement. Nested stages overlap
and must not be added together. The projection stage covers the committed-history
publication method, not all WebView work or every semantic command. History
adoption excludes writer queue/load/commit latency. Frame/composite rows include
non-ink redraws; brush/GPU-op rows require samples or operations respectively.

Input instrumentation retains one optional `Instant` for a nonempty admission
batch under the already-existing raw queue lock, and transfers it after releasing
that lock. It represents the first accepted push since the previous full drain,
including any coalesced/evicted Move. This is not a per-sample timestamp or the
Win32 event clock. The first corresponding present API return consumes the
thread-local pending marker; a failed surface acquisition leaves it pending.
Discontinuous batches are excluded. A pending marker at canvas destruction is
reported separately. Dedicated close-worker drains have no presentation and are
outside the displayed input histogram. Neither GPU completion nor visible pixels
are measured. Physical mouse/pen/display cadence is not inferred from tool-driven
input injection or a configured refresh rate.

One atomic flag classifies each span by whether the single document writer was
inside PNG export at span start. A span may cross an export boundary, and export
phase counts across nested stages can differ. The flag is only diagnostic; it
does not gate drawing, saving or task scheduling. Future interference comparisons
must use enough matching ink batches, not merely compare all idle frame rows.

## Installed-app wiring evidence

Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc / DX12, pinned DX release,
surface Mailbox. WMI reported 2880×1800 at 120 Hz; screenshot surface was
2096×1458 physical pixels at scale 2. A background user-owned video download was
running and was left untouched. This was not an isolated benchmark environment.

With timing enabled, Computer Use selected a 3 px brush in a 16×16, three-raster
scratch fixture, injected one mouse drag, then used actual Undo/Save/Close. The
native path received three nonempty batches. Records separated those batches
from one CPU replay/commit, navigator/thumbnail work and one export:

| Stage | Count | p50 upper | p95 upper | p99 upper |
|---|---:|---:|---:|---:|
| Batch admission → dequeue | 3 | 9 µs | 26 µs | 26 µs |
| Batch admission → present API return | 3 | 1599 µs | 11917 µs | 11917 µs |
| GPU operations encode/submit | 3 | 767 µs | 9516 µs | 9516 µs |
| Stroke CPU replay | 1 | 176 µs | 176 µs | 176 µs |
| Stroke commit/accept | 1 | 3990 µs | 3990 µs | 3990 µs |

The initial GPU-op spike is a location to investigate, not proof of a persistent
bottleneck. Three batches cannot support a tail-latency or 4K interference claim.
No warmup was excluded in this wiring check.

Separate-process `desktop_page_fixture` verification after close compared every
original tile, locked/hidden/negative artwork, dimensions/PPI, snapshot 1/history 2
and independent expected PNG exactly. A normal restart without the timing flag
visually restored the original fixture, emitted zero performance lines, and
passed the same verifier after close. Evidence is in `target/performance-ui/`:
`first-out.log`, `first-verify.log`, `reopen-out.log`, `reopen-verify.log`.

All-target/all-feature workspace Clippy with warnings denied passed in
`target/performance-clippy.log`. The existing 15 desktop invariant tests passed
in `target/performance-desktop-tests.log`; no new wiring tests were added. Release
build/install passed in `target/performance-install.log`.

Next work is repeated installed-shell startup/reopen/undo and paced 4K drawing
with export inactive/active, long-session UI changes, sufficient sample counts,
and focused optimization based on those results. OS-event-to-visible-pixel and
physical pen acceptance remain separate hardware/manual evidence.
