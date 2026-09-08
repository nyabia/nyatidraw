# ADR-0042: Windows alpha file associations

문서 기준 시각: 2026-09-08T21:24:30+09:00

- Date: 2026-09-08
- Decision: register per-user alpha ProgIDs through Velopack lifecycle hooks.

The installed `NyatiDraw.Alpha` package registers `NyatiDraw.Alpha.PNG.1` and
`NyatiDraw.Alpha.Project.1` under HKCU Software/Classes. Both formats appear in
Open With through their extension's OpenWithProgids value. PNG's default and
Explorer UserChoice remain untouched. `.ntdr` gets a default only if neither the
merged machine/user Classes view supplies one. A protected UserChoice is never
written or deleted, including during uninstall. Development registrations use
different ProgIDs and are not migrated.

The open command is `"<install root>/Update.exe" start nyatidraw-desktop.exe -- "%1"`.
The root launcher resolves the current application after updates; the quoted
file argument reaches the existing single-instance activation path. The icon
references the stable `current/nyatidraw-desktop.exe` path. These paths come from
the installed package locator, not an assumed LocalAppData directory. Portable
packages and different package IDs do not register.
The ProgID's Application subkey supplies the alpha name, artwork icon, and AUMID
so Open With can identify the app independently of Update.exe, following
[Microsoft's ProgID application metadata example](https://learn.microsoft.com/en-us/deployedge/edge-ie-mode-add-guidance-filetype-associations).

After-install and after-update fast hooks register idempotently. Before-uninstall
removes only values whose type and bytes still match this installation, gated by
an installation-root ownership marker on the ProgID. An unowned ProgID or changed
value is not overwritten. Unknown values/subkeys survive; only empty keys are
pruned. A changed file-extension default survives cleanup. The shared extension
keys are never recursively deleted. Registration failure is logged but cannot
cancel the installer and does not involve project files. Hook execution does not
open the UI, start the drawing engine, or run a subprocess.

No new dependency version is introduced: the existing Windows **0.62.2** binding
adds its Registry feature; Velopack remains **1.2.0**. No packaging script changes
are needed because the hook is compiled into the packaged main executable.

## Upstream contract

The pinned [Velopack 1.2.0 Rust hook implementation](https://github.com/velopack/velopack/blob/1.2.0/src/lib-rust/src/app.rs)
dispatches install/update/uninstall callbacks before ordinary startup, then exits
unless Velopack debug mode is enabled. The
[hook documentation](https://docs.velopack.io/integrating/hooks) specifies a
30-second install/uninstall limit and a 15-second update limit, with no UI or
cancel feedback. The [Update.exe 1.2.0 CLI](https://docs.velopack.io/reference/cli/content/update-windows)
defines `start [EXE_NAME] -- [EXE_ARGS]` as launching the current installation.

## Verification boundary

An explicitly ignored Windows acceptance test injects a fresh registry subtree
under HKCU Software/NyatiDraw/Acceptance. It covers install, repeat registration,
foreign command/subkey preservation, shared Open With preservation, existing
defaults, uninstall, and unowned registration collisions. It never uses real
Classes or Explorer UserChoice. Run it explicitly with
`cargo test -p nyatidraw-desktop lifecycle_preserves_foreign_associations_and_user_choices -- --ignored`.

The scratch acceptance passed on 2026-09-08. Packaged update, uninstall and
reinstall hooks also ran successfully, preserving foreign registrations and
artwork. However, file-handle and external WMI evidence showed Codex's packaged
execution environment redirected the installed files into its LocalCache while
the ProgIDs reached real user Classes and referenced the conventional path.
Explorer did not offer the app in Open With. This is not ordinary Windows
installation acceptance. An Explorer-launched installation outside that context
and explicit shell activation remain the release gate; no Codex-specific path
workaround belongs in the product. See the [integration record](../status-plan-2026-09-08.md).
