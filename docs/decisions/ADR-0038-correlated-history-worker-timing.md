# ADR-0038: Correlate bounded history worker and presentation samples

- Date: 2026-09-06
- Status: Instrumentation implemented; installed measurement pending

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

No tests were added for timing plumbing. Compilation/Clippy and an installed
campaign with sample/counter reconciliation, normal save/close, full artwork/PNG
comparison and ordinary restart will validate the integration.
