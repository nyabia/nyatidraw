# NyatiDraw Wry backport

This is the published **wry 0.53.5** crate, required by Dioxus Desktop 0.7.9.
The workspace applies it with `[patch.crates-io]`; it is not a workspace member.
Upstream licenses and package files are retained. Cargo's local `.cargo-ok`
marker and the crate's independent `Cargo.lock` are omitted; the workspace
lockfile remains authoritative.

- Registry: <https://crates.io/crates/wry/0.53.5>
- Published archive SHA-256:
  `728b7d4c8ec8d81cab295e0b5b8a4c263c0d41a785fb8f8c4df284e5411140a2`
- The packaged `.cargo_vcs_info.json` reports revision
  `2bfe45f48a166f4c172a460de23f925ec9e534b1` and `dirty: true`; the published
  archive, not an assumed clean checkout at that revision, is the baseline.
- Backported fix: <https://github.com/tauri-apps/wry/pull/1799>
  (merged as `b95a7f8`, released upstream in 0.56.1).

The only implementation change is in `src/webview2/mod.rs`: a failed initial
`ICoreWebView2Controller::MoveFocus` no longer rejects an otherwise completed
WebView. Unlike the upstream patch's discarded result, we print one
`initial-focus-deferred` diagnostic on failure. Environment, controller,
settings, script, navigation, and visibility failures still propagate.
Later focus behavior is unchanged. No retry loop, profile reset, forced
foreground activation, or runtime replacement is introduced.

Remove this directory and the Cargo patch when the chosen Dioxus release
supports a Wry version containing #1799, after Windows launch, focus,
Save/Close/reopen, and artwork acceptance. Do not silently upgrade this copy
or apply unrelated fixes here.

See `docs/decisions/ADR-0035-webview-startup-focus.md` in the repository for
the failure evidence and runtime acceptance.
