# ADR-0035: Keep a completed WebView alive when initial focus fails

- Date: 2026-09-06
- Status: Accepted for the identified startup focus failure

## Failure and diagnosis

Ordinary installed launches repeatedly exited at Dioxus Desktop 0.7.9's
`webview.rs:503` unwrap with WebView2 HRESULT `0x80070057`. CPU project reopen
and native GPU canvas creation had already succeeded. Moving the profile did
not resolve it; that attempted change was reverted in 912cb76. No user profile,
runtime installation, download, or Windows security setting was changed.

A separate ignored copy of Wry 0.53.5, used only by a diagnostic Cargo command,
identified the exact failure in `init_webview`: the final
`ICoreWebView2Controller::MoveFocus(PROGRAMMATIC)`. Environment/controller
creation, settings, handlers, navigation and visibility had succeeded. In the
third recorded failure, both parent and WebView were visible, the parent was
enabled, and it was not minimized. Thus the observed failure must not be
attributed specifically to a minimized window or corrupt profile.

Raw logs: `target/history-present/diagnostic-{2,3}-{out,err}.log`, with build
logs and PIDs alongside. The global Cargo registry source was not modified.

The same fatal treatment of a focus failure is addressed by upstream
[Wry #1799](https://github.com/tauri-apps/wry/pull/1799), following a minimized
window reproduction in [#1798](https://github.com/tauri-apps/wry/issues/1798).
It was released in 0.56.1. Dioxus Desktop 0.7.9 requires the incompatible
0.53.x line; the latest published 0.53 release checked on this date is 0.53.5.

## Decision

Backport only #1799 to the published Wry **0.53.5** source under
`third_party/wry`. The root Cargo patch makes regular Cargo and DX builds use
the same source. Keep Dioxus and all resolved dependency versions unchanged.
Keep upstream licenses and archive provenance in `third_party/wry/NYATIDRAW.md`.
The directory is excluded from workspace membership.

Failure of this one advisory focus request emits one
`desktop-webview event=initial-focus-deferred` diagnostic and continues with
the fully constructed WebView. This logging is the only adaptation of the
upstream discarded result. Other initialization errors still propagate;
later activation and user focus handling remain unchanged. There is no forced
foreground retry, profile reset, or general error swallowing.

The maintenance cost is a vendored upstream crate for a small fix. Remove the
patch when the selected Dioxus supports a Wry containing #1799, after the same
Windows acceptance. Source comparison against the published crate found only
this implementation change. No artwork/input/project schema changes or
automated UI tests were added.

## Installed acceptance

Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc DX12, DX 0.7.9 release.
Installed executable SHA-256:
`AA3959C637B7D59BC4BDB84E11BEF36CA0C6F4A20506E38ED0BF42D8C78AF7EE`.

No WebView profile, performance, or layout environment override was supplied.
The historical 3840×2160 scratch contains 902 tiles and 33 history nodes.

- First launch (PID 28068) displayed the reopened artwork in the normal editor.
  Initial Z before a toolbar click admitted no command. A coordinate Undo click
  and subsequent Shift+Z were admitted and restored the expected history.
  S saved/exported snapshot 33. Alt+F4 drained/joined the writer and exited.
  The independent reference verifier reopened the database and matched every
  tile, tree/page, history count and decoded PNG exactly.
- Second launch (PID 47884) reached native present and remained running.
  A second process opening the same file routed to this primary; the primary
  recorded `activation-reused-current-project`. Further UI observation was
  interrupted by the computer-use helper reporting
  `foreground window did not report a process id` twice after fresh selection.
  This is not evidence that startup failed, nor a completed second UI round trip.
- Neither of these two launches logged a deferred focus error. They verify the
  installed application path, not exercise of the nonfatal error branch.

Release build, DX build/install, and workspace all-target/all-feature Clippy
with warnings denied passed. The path dependency emits its existing upstream
`mismatched_lifetime_syntaxes` warning; workspace compilation remained clean.
Raw evidence is `target/history-present/focus-fix-*`; `wry-backport.diff`
records the exact upstream source difference.

## Controlled focus-failure reproduction

A scratch standalone program reproduces the upstream condition: create a Tao
window, minimize it, assert `is_minimized()`, then build Wry with default focused
attributes and `about:blank`. It owns its window and touches no artwork.
Identical program, Tao 0.34.8 and release profile; the second build only patches
Wry to the committed local source. Both runs use the same existing probe profile.

| Wry source | Observed result | Exit |
|---|---|---:|
| Published 0.53.5 | Minimized=true; build fails with 0x80070057 | 2 |
| 0.53.5 with this backport | Same focus error logged; WebView created and dropped; teardown complete | 0 |

Source: `target/webview-focus-probe/{Cargo.toml,src/main.rs}`. Build/run records:
`target/history-present/focus-probe-{baseline,patched}-{build,out,err}.log`.
This is a focused runtime acceptance probe, not an added automated framework
test or a claim that the earlier visible/non-minimized host had the same trigger.

This closes the identified fatal focus handling defect. It does not prove all
WebView initialization failures are
recoverable, initial keyboard focus is always assigned, Explorer Open With is
fixed, or hardware input/display performance gates pass.

## Subsequent installed error-path acceptance

The computer-use helper recovered on a later observation. The second instance
was normally closed and its full independent comparison passed
(`target/history-present/focus-fix-2-verify.log`); no forced termination was used.
The helper failure's cause was not established.

The subsequent storage-optimized installed application, PID 35024 and ordinary
restart PID 42084, both logged `initial-focus-deferred` with 0x80070057 and
continued. The first completed 20 actual UI Undo/Redo operations, Save and normal
Close; the second displayed the saved 4K artwork and normally closed. Both full
independent artwork/PNG comparisons passed. Unlike the earlier two launches,
these exercise the nonfatal error branch in the real installed editor.
See [ADR-0036](ADR-0036-deduplicate-root-object-loads.md) for source, hash and logs.
