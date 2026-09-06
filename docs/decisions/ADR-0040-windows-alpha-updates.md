# ADR-0040: Windows alpha installation and updates

- Date: 2026-09-06
- Decision: Velopack Rust SDK and packager **1.2.0**, Windows x64, full packages,
  `NyatiDraw.Alpha` package ID and `alpha` channel, public GitHub Releases source.

The installer prepares WebView2 when needed and creates a per-user Start Menu
shortcut. Artwork lives outside the replaceable application directory. An
ordinary launch reopens `%LOCALAPPDATA%/NyatiDraw/Sketchbook/작업 중.ntdr`.
Explicit PNG/ntdr arguments and the existing project environment override retain
precedence. A failed default project open must remain visible.

Update checking/downloading runs on one background worker, outside the native
input/render queues. UI status flows through the existing notification into a
dedicated Dioxus signal. The SDK's automatic apply-on-startup is disabled.
The user requests Save and ordinary close; only after writer/export join and a
successful close result may Update.exe wait for process exit and apply. The
current project path is passed to the restarted process. Save/export failure
cancels that update request. There is no force-kill path in the app updater.

The unsigned installer is an alpha delivery choice, not a claim of publisher
verification or warning-free Windows installation. Full package checksums are
separate from publisher signing. Third-party notices are collected into the
package. The repository's own license has not yet been assigned.

## Evidence so far

- Local Windows release compilation, existing desktop tests and workspace
  all-target/all-feature Clippy pass. No new framework/UI tests were added.
- Local package 0.1.0-alpha.1 installed and launched. A synthetic mouse stroke in
  the default sketchbook survived Save, normal process exit, and launcher restart.
  Reopened content root: `4ada7bc091c2d15286f9a3c9991823579e15a81e0a7c01e44a2556138a9f21b6`.
  PNG SHA256 before/after restart:
  `0338a30d3caa2e08d31159eb2c422d7af9ffa0b9eea93f976837438cfdba51f1`.
- Local package 0.1.0-alpha.2 installed successfully, retained that drawing,
  and the manual update check visibly entered its checking state.
- An isolated SDK runtime probe used a copied manifest and scratch package
  directory. Connection refusal and a same-size package with one altered byte
  were rejected; no complete package or pending restart was produced, and the
  copied installed manifest was unchanged. This is SDK failure-path evidence,
  not a whole-machine offline UI test or power-loss test.
- Raw local evidence is ignored under `target/public-alpha-acceptance/` and
  `target/public-alpha-tools/`. Cloud packaging and the actual published-feed
  A-to-B saved-close update still need the acceptance result in `docs/releasing.md`.

This evidence does not verify physical-pen latency, a clean Windows VM without
WebView2, Explorer Open With, or any non-Windows backend.
