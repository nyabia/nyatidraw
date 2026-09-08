# Dioxus Desktop native retirement gate

This is the published **dioxus-desktop 0.7.9** crate. It remains pinned at
0.7.9 and is patched through the workspace `[patch.crates-io]`, not added as
a workspace member. No dependency, feature, or toolchain version is upgraded.

- Registry archive: <https://crates.io/crates/dioxus-desktop/0.7.9>
- Archive SHA-256: `662cd78c73ca3f17346adbf2d64757df40dd0ce20536c05123097fd31828d2bd`.
- Packaged VCS metadata: `bfcc111817b03c5f737374037eabef8eb6795e46`, `dirty: true`,
  path `packages/desktop`. The published archive, not an assumed clean git
  checkout, is the baseline.
- The copied `.cargo-ok`, independent `Cargo.lock`, and `.vscode` directory
  are omitted. Package-declared `headless_tests` and their Cargo entries are
  retained to keep the upstream manifest valid; they are not product tests.
- License metadata is `MIT OR Apache-2.0`. The published archive does not carry
  root license files, so the upstream MIT text from the packaged revision and
  the standard Apache-2.0 license are supplied here. `NOTICE` identifies this
  modification. `tools/package-release.ps1` collects these root license/notice
  files through Cargo metadata, including this path dependency.

## Narrow implementation change

`src/config.rs` adds optional `with_exit_guard(FnMut() -> bool)`. The callback
must be nonblocking and may join only an already-finished worker.

`src/launch.rs` latches a requested exit (including its exit code), or captures
an application-dispatch panic. While the callback says a native worker is
still retiring, it keeps the native event loop at `WaitUntil(now + 10 ms)` and
does not reenter the exiting/panicked App state. When retirement completes,
the original exit or panic resumes. A panic is neither silently swallowed nor
converted into success. Without a guard, ordinary behavior is unchanged.

Optional startup triage instrumentation in `src/app.rs`, `src/edits.rs`,
`src/protocol.rs` and the IPC dispatch in `src/launch.rs` is enabled
only by `NAYATI_STARTUP_DIAGNOSTICS`. It records first initialization, edit queue,
socket send/acknowledgement, and UI poll completion without payloads, URLs or
authentication keys. It does not change acknowledgement or retry behavior and
is removable independently of the exit gate. The first interpreter `rafEdits`
entry/return/throw wrapper is injected only when that flag is enabled, preserves
the original receiver, arguments, return and thrown value, and reports only a
fixed stage, byte length and headless boolean. With the flag disabled it adds
no interpreter script or IPC. The intermittent pre-observer
startup stall is not diagnosed or claimed fixed by this instrumentation.

The public custom-event callback cannot implement this: it receives no mutable
ControlFlow, and the launcher overwrites ControlFlow after that callback.
Tao's normal `run` exits the process after its event loop finishes, regardless
of retained `Arc<Window>` objects. Both surface lifetime and process-exit
retirement therefore need explicit ownership, not merely an HWND integer.

The application starts its render worker from the root's initial-render hook after
WebView construction. Earlier WebView construction failure has no live worker
to retire. Hard process termination and aborting panics remain outside orderly
shutdown guarantees.

Remove this patch when upstream exposes an equivalent nonblocking exit/panic
retirement hook and the full native lifecycle acceptance passes against it.
