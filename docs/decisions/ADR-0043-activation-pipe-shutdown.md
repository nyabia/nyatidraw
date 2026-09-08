# ADR-0043: Cancelable Windows activation pipe operations

문서 기준 시각: 2026-09-08T21:24:30+09:00

Date: 2026-09-08
Status: Accepted

## Problem

The single-instance listener used synchronous `ConnectNamedPipe` and `ReadFile`.
A client that connected but retained an incomplete frame could hold the listener
inside `ReadFile` indefinitely. Primary shutdown tried to connect another client
to wake the listener, then joined it. The existing connection prevented that
wake, so normal exit and the update process waiting for exit could never finish.
The frame length cap bounded allocation, but did not bound this wait.

## Decision

Use the existing exact `windows = 0.62.2` dependency and its enabled System IO
and Threading APIs. There are no new dependencies.

- Create the server with `FILE_FLAG_OVERLAPPED` and a private manual-reset event
  per operation. Keep blocking pipe mode (`PIPE_WAIT`); do not use legacy
  `PIPE_NOWAIT` as a substitute for asynchronous I/O.
- Poll operation completion every 25 ms and observe the shutdown flag during
  both connect and read. The primary joins the listener without opening a wake
  client.
- Give the entire incoming frame one two-second deadline, including header and
  payload. Receiving another fragment does not extend it. Idle listening has no
  deadline but remains cancelable.
- On shutdown or timeout, call `CancelIoEx` for the exact pending operation and
  retire it with `GetOverlappedResult` before releasing its buffer, OVERLAPPED,
  event, or pipe handle. Cancellation request alone is not completion.
- Reject remote clients with `PIPE_REJECT_REMOTE_CLIENTS`. The existing local
  pipe security descriptor and same-machine activation protocol are otherwise
  unchanged; this is not a new authenticated per-user transport guarantee.

This removes waiting on an uncooperative client. It is not a hard real-time
shutdown guarantee: Windows still must finish/cancel its local kernel I/O.
Client delivery remains best effort, without a server acceptance acknowledgment.

## Evidence and limits

The regression test uses two isolated real Windows named pipes. Each client
sends a valid header but withholds its announced payload while holding the
connection open. The server must return an error for both the total frame
deadline and the shutdown flag before the client is released. Existing UTF-16
and oversized outbound-frame checks remain. A second real-pipe check cancels
an idle connect, reuses the listener, and delivers a Korean/space-containing path
followed by a foreground-only frame through already-connected clients.
These tests protect shutdown and
activation availability, not artwork persistence or physical input latency.

`rustfmt` and scoped `git diff --check` passed. On this Windows development
machine, `cargo test -p nyatidraw-desktop single_instance::tests -- --nocapture`
passed all four selected tests (zero failures) in 2.01 seconds. The desktop test
target compiled; the existing vendored Wry lifetime warning remained. An initial
reuse test tried opening its client before restarting `ConnectNamedPipe` and was
corrected to use a bounded client retry alongside server connection. No full
workspace build or installed GUI test was run by this focused change; Clippy
and broader integration remain the integrating task's responsibility.

## Upstream contracts

- [Named pipe type, read, and wait modes](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes)
  documents blocking calls and the legacy nonblocking wait mode.
- [ConnectNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe)
  documents overlapped connection and the already-connected race.
- [CancelIoEx](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)
  requires retaining OVERLAPPED until cancellation completes; cancellation can
  race normal completion and does not itself wait.
