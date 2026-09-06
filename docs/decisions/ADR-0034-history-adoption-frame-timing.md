# ADR-0034: Time history worker admission through its adoption frame

- Date: 2026-09-06
- Status: Accepted for bounded CPU/API timing; Undo latency and normal startup gates remain open

## Decision

The existing history_adoption span omits worker preparation/wait and the later
composite/present. Carry an opt-in timestamp in the single pending ArtworkJob only
for MoveHistory requests. Capture it immediately after successful worker queue
admission, before publishing busy/accepted UI state. This is not the keyboard,
pointer, DOM event, or EditorCommand queue admission timestamp.

A changed, successfully adopted history outcome records history_queue_to_adoption
after its projection publication. The timestamp travels with only the display
texture produced by that adoption render call. The child surface host records
history_queue_to_present_request after blit/submit and frame.present() return.
GPU completion, scanout and physical visible pixels are outside the interval.

No-op/rejected/fatal requests never produce an adoption-frame timestamp. If
compositing or surface acquisition fails, the local timestamp is discarded rather
than attached to a later unrelated frame, activation or snapshot. Queue/adoption/
presented counters expose those missing outcomes. Their difference does not by
itself diagnose no-op, worker failure, close, or surface failure; inspect the
associated runtime log. The changed-and-presented distribution is conditional on
successful adoption-frame submission, and must always be read with those counts.

No raw samples, pixels or timing objects enter the UI command protocol. The
recorder remains fixed and opt-in (20 stages × 2 phases × 1024 × 8 = 320 KiB of
histogram bins per participating thread). Each interval's export classification
is captured at worker admission. Nested/component percentiles must not be added
or subtracted to manufacture a whole-path percentile.

## Installed evidence

Source 532c417, installed SHA-256
9A4C1B2A05484EBE1455A67AD3566147D1546785C5D7D6DE9BE35793808E8A64;
Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc DX12, pinned DX 0.7.9
release. Native surface 2096×1458, scale 2; host configured at 2880×1800/120 Hz.
No physical refresh or input-to-visible measurement is implied.

The historical 3840×2160, two-raster, 32-stroke, 902-tile, 33-history scratch was
copied without reinterpreting its stored colors. Ten actual UI Undo/Redo pairs
produced queued=20, changed_adopted=20, adoption_frame_presented=20, no command
rejections. The first operation is included; no warmup was discarded.

| CPU/API interval | p50 upper | p95 upper | p99 upper |
|---|---:|---:|---:|
| history_adoption | 21.503 ms | 30.719 ms | 33.280 ms |
| history_queue_to_adoption | 114.687 ms | 139.263 ms | 146.435 ms |
| history_queue_to_present_request | 131.071 ms | 155.647 ms | 164.815 ms |

These are one session's histogram bounds, not a before/after optimization claim.
The 16 ms Undo target is not met. Worker-side preparation/reopen is a priority for
additional instrumentation; do not assign exact per-stage cost by subtracting
the percentile columns. No concurrent build/test ran during the accepted inputs;
other host activity was uncontrolled and unrelated processes were untouched.

Normal UI Save completed PNG export, normal Close drained/joined the writer, and
the independent reference verifier reopened the closed database and compared all
902 tiles, complete tree/page, snapshot 33, history count 33 and decoded PNG.
All matched. This verifies stored artwork after process exit; normal-profile GUI
restart is a separate unresolved startup issue below.

Workspace all-feature/all-target Clippy passed. No automated UI/framework tests
were added. Raw records: target/history-present/{out,err,verify}.log and
[all measured rows and environment](../measurements/history-present-4k-2026-09-06.json).

## Startup and input setup limitations

The default WebView profile failed at Dioxus 0.7.9 webview.rs:503 (WebView build
unwrap, HRESULT 0x80070057). Two failures have separate logs; an additional Shell
launch exited without a window. A dedicated WEBVIEW2_USER_DATA_FOLDER under the
scratch directory allowed the measured session to run. Earlier key attempts
without WebView focus and one misplaced accessibility click admitted no commands;
they are not warmup operations. A screenshot-coordinate Undo click established
focus, and the following nineteen keyboard commands were accepted.

The crash scratch initially had no PNG because only the database was copied.
Its first reference verification passed database comparison then failed opening
the absent PNG. A previously verified reference PNG was supplied, after which the
same complete check passed. That supplied PNG is not an export from the failed
startup. The measured session subsequently exported its own PNG through Save.

An explicit AppData profile-directory change (aa61b21) did not resolve later
startup failures and was reverted (912cb76). Further attempts also failed with
an existing scratch profile and a new unique AppData profile. Therefore neither
profile corruption nor AppData location is an established root cause. No profile
was deleted and no Windows security/permission settings were changed. Preserve
the successful session's measurement, but do not claim reliable GUI restart or
the broader Sprint gates from it.
