# ADR-0038: Correlate bounded history worker and presentation samples

- Date: 2026-09-06
- Status: Accepted for bounded installed correlation; earlier intermittent tail remains unexplained

ADR-0037 improved adoption p95, but one whole-Undo sample reached 157.958 ms.
Independent stage histograms cannot identify what delayed that same request.
Preserve existing queue/adoption/present timing and add per-request boundaries
only when `NAYATI_PERFORMANCE=1` is enabled.

The history worker stamps entry, cursor preparation, tile read/validation,
tree/page read, immediate cursor persistence, session adoption and reply readiness.
The successful response carries these stamps to the same UI history job. The UI
adds receipt, adoption/publication and that adoption frame's present-return time.
Layer edits and branch pagination do not become changed-history samples. Failed,
no-op or never-presented jobs do not borrow another job's frame.

The canvas recorder holds at most 64 complete samples in a preallocated vector.
Further samples increment an explicit omitted count; existing histograms and
admission/adoption/present counters continue. Raw samples print only on recorder
flush. Histograms remain 20 stages × 2 export classifications × 1024 u64 bins
(320 KiB per participating thread). Sample allocation bytes are printed with the
capacity, rather than assuming a platform's `Instant` layout. No worker logging,
new queue, lock, dependency or persistent artwork format is introduced.

Definitions and limitations:

- Admission retains its existing timestamp after `try_send` succeeds. A worker
  may begin earlier; `worker_before_admission=true` marks that overlap and its
  reported queue wait is clamped to zero, not a claim of exact queue latency.
- `preview_reply_us` includes state bookkeeping/history projection and navigator/
  thumbnail publication after session acceptance until immediately before send.
- `reply_wait_us` includes reply delivery, redraw scheduling and UI polling until
  the result is received. It is not purely an OS scheduler measurement.
- `adoption_publish_us` includes GPU/CPU snapshot adoption and UI projection work;
  the existing nested adoption histogram remains a narrower interval.
- `after_adoption_us` includes subsequent frame work through present API return.
  GPU completion and visible pixels are not measured.
- Raw durations round up to microseconds independently; sums can differ by a few
  microseconds, and the queue-overlap case is not an additive partition. Raw
  total and histogram samples are stamped adjacent to one another, not at exactly
  the same instruction. Export classification is sampled at admission.

No tests were added for timing plumbing. All 16 existing desktop tests and
workspace all-target/all-feature Clippy with warnings denied passed. The unchanged
upstream vendored Wry lifetime warning remains outside workspace diagnostics.
Logs: `target/history-worker-timing/{tests,clippy,install}.log`.

## Installed correlation acceptance

Source `c5bc99c`, installed executable SHA-256
`456D0238B1C1B94BD2A133EC2F017A9857FF9EC6E9B163D5DBEF40102D2461B9`.
Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc DX12, pinned Dioxus CLI
0.7.9 release. Native surface 2096×1458, scale 2; 3840×2160 historical fixture,
two raster layers, 32 strokes, 902 tiles and 33 history nodes. Configured display
fields in the JSON are inherited host records, not new physical refresh evidence.

One actual toolbar Undo followed by 19 alternating keyboard Redo/Undo operations
completed ten pairs. First included, no warmup omitted. Default WebView profile,
missing scratch layout override during measurement, no override on restart.
No concurrent build/test; other host activity uncontrolled and protected downloads
not inspected or altered. Each operation retained 846 GPU tiles and uploaded 56.

Queue/adoption/present counters and complete correlated samples are each 20;
sequence 1–20 occurs once each, omitted=0. Reserved sample storage was 12,288 bytes
on this build. Every record's non-overlapping components sum to total within
independent microsecond ceiling rounding. No worker-before-admission overlap
occurred. This campaign does not exercise capacity overflow, failed/no-op jobs or
the queue-overlap branch; their contracts are enforced by code and not presented
as runtime coverage here.

| Raw interval, nearest-rank ms (20 samples) | p50 | p95 | p99 |
|---|---:|---:|---:|
| Admission → worker start | 0.027 | 0.100 | 0.116 |
| Tile read/validation | 12.672 | 14.118 | 14.128 |
| Immediate cursor persistence | 1.275 | 1.737 | 3.303 |
| Worker total | 17.002 | 18.925 | 21.525 |
| Reply ready → UI receipt | 0.152 | 0.324 | 0.603 |
| Adoption/publication | 10.470 | 11.499 | 13.243 |
| After adoption → present API | 14.631 | 17.078 | 17.095 |
| Admission → present API | 42.681 | 46.861 | 47.752 |

Existing whole-history histogram upper bounds are 43.007/47.103/47.752 ms.
These bounds differ from exact raw quantiles by design. The earlier 157.958 ms
tail did not recur; it has not been fixed or explained by this campaign.

The slowest request here, sequence 20, took 47.752 ms: queue 0.027, worker 17.103,
reply wait 0.304, adoption/publication 13.243, remaining frame 17.078 ms. Its worker
included tile read 13.007 and persistence 1.275 ms. Thus that particular request's
delay is mostly useful restoration/frame work, not queue wait or a long persistence
call. This does not explain the earlier uncorrelated outlier or prove that storage
and scheduling cannot produce tails. The remaining frame interval is not itself
a direct GPU execution measurement.

Save and normal Close completed PNG generation and joined the writer. Independent
verification matched all 902 tiles, tree/page, snapshot 33/history 33 and decoded
PNG. Ordinary installed restart displayed the artwork, then normal Close and a
second full comparison passed. The measured launch had empty stderr; restart
logged the handled initial-focus error and continued. Timing-disabled restart
emitted no performance records. Raw logs and scratch files:
`target/history-worker-timing/ui/`.
[All histogram rows, individual samples, quantiles and verification paths](../measurements/history-worker-timing-4k-2026-09-06.json).

The 16 ms Undo target remains open. This change enables diagnosis; it is not a
speed optimization. Next work should address renderer ownership/surface-wait
isolation and remaining restoration/composite cost, while retaining these stamps
to diagnose an intermittent tail when it recurs. No physical-pen, visible-pixel,
paced drawing/export, or long-session latency claim follows from this campaign.
