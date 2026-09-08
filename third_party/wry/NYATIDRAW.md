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

The startup backport in `src/webview2/mod.rs` ensures a failed initial
`ICoreWebView2Controller::MoveFocus` no longer rejects an otherwise completed
WebView. Unlike the upstream patch's discarded result, we print one
`initial-focus-deferred` diagnostic on failure. Environment, controller,
settings, script, navigation, and visibility failures still propagate.
Later focus behavior is unchanged. No retry loop, profile reset, forced
foreground activation, or runtime replacement is introduced.

Remove the startup backport when the chosen Dioxus release
supports a Wry version containing #1799, after Windows launch, focus,
Save/Close/reopen, and artwork acceptance. Do not silently upgrade this copy
or apply unrelated fixes here.

## NyatiDraw parent composition adapter

An additional explicit Windows-only `with_parent_composition(true)` opt-in
creates a WebView2 composition controller. `src/webview2/composition.rs`
attaches its visual to the parent HWND's topmost DirectComposition layer,
above child HWNDs. The existing WRY HWND remains the focus/input sink;
`composition_input_hwnd()` exposes it so NyatiDraw can shape its input region
without clipping the independently hosted visual. The host must use a
transparent HTML canvas area and transparent WebView background.
The composition-only input sink uses `WS_EX_NOREDIRECTIONBITMAP`, because
all visual content is supplied by the parent's composition target. The input
HWND intentionally owns no visual pixels. This flag expresses that ownership;
it is not by itself a guarantee of translucent-overlay correctness.

This path forwards mouse and native pen/touch UI input, with mouse capture,
leave tracking, and a bounded 32-contact cancellation cache. Canvas samples
must reach the separate native canvas HWND and never this UI adapter.
Full-parent construction only is supported; child construction and reparenting
return errors. The default windowed path is unchanged. No browser pixel
readback or canvas swapchain replacement is involved.

The implementation adds Windows API feature flags to the existing windows
dependency, not a new dependency version. Build/runtime, physical pen, IME,
capture cancellation, and translucent-overlay acceptance remain gates; source
implementation alone does not establish them. Removing the entire vendor
patch now also requires an equivalent upstream composition-host adapter.

See `docs/decisions/ADR-0035-webview-startup-focus.md` for the startup backport,
and `docs/decisions/ADR-0046-web-ui-over-native-canvas.md` for the composition
contract, input routing, failed probes and runtime acceptance boundaries.
