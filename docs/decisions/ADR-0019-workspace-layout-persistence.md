# ADR-0019: Process-wide workspace layout persistence

Status: accepted for layout restore and recovery; remaining docking interactions
and toolbar placement gates stay open.

## Decision

The primary desktop shell loads the user's dock tree before constructing either
the UI projection or native canvas. Accepted dock commands, including tab changes
and the menu's safe-default reset, submit the authoritative tree to a dedicated
layout worker. It keeps one latest pending payload and performs filesystem I/O
outside the renderer and UI threads. Normal background close flushes that work.
Project activation retains the process-wide layout.

Preferences live in `%LOCALAPPDATA%/NyatiDraw/workspace.layout`. The explicit
`NAYATI_LAYOUT_PATH` override isolates scratch acceptance from user preferences.
The versioned `NYDOCK01` binary stores panel IDs, tab membership/active index,
split axes and ratios. Reads are capped at 4096 bytes, recursion at 16 levels;
unknown tags, trailing data and invalid trees are rejected. Existing `DockTree`
validation requires every panel exactly once and valid tab/ratio constraints.

Missing preferences use the safe default. Invalid or unreadable preferences show
a small status row and use the complete safe default without changing the source
file at startup. A subsequently accepted dock change or explicit reset may replace
that file with the new valid layout; this is not a backup or salvage mechanism.
Writes use a unique sibling temporary file, `sync_all`, and Windows
`MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`. Failed replacement preserves the
prior file and reports a retry action. Settings and artwork saving remain
independent failure domains. No project format or dependency changes were needed.

## Evidence

Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, installed DX12 release
build. Computer-use automation operated the actual desktop window. All scenarios
used `target/layout-ui/`, its explicit settings override and a 129×65 scratch
project copied from the source/tolerance fixture.

- Dragging Color to the left above Brush and selecting History saved a 43-byte
  layout. Normal close and a fresh process restored both the split placement and
  active History tab, as observed in the installed UI.
- Making only the scratch settings file read-only and selecting Layers displayed
  the save failure while leaving the canvas and panels usable. Hash comparison
  proved the previous file was unchanged. Removing the read-only attribute and
  choosing the menu's default layout restored all panels, dismissed the notice,
  and produced bytes identical to the recorded default layout.
- A valid header followed by a Canvas-only tree recovered with
  `MissingPanel(Tools)`. All default panels and the canvas remained visible. Hash
  comparison before reset proved the invalid file was preserved at startup. The
  explicit reset cleared the notice and wrote exactly the default layout bytes.
- The observed sessions each logged one native-canvas creation and normal
  `close-ready writer=joined`. Separate fixture processes after normal close
  verified snapshot 2/history 2, all 8385 page pixels, other layers, negative tiles,
  boundary padding and independent PNG expectations without artwork changes.

Logs and byte/hash artifacts are in `target/layout-ui/`, including
`first-artwork-verify.log`, `readonly-final-{out,err}.log`,
`readonly-final-verify.log`, `readonly-artwork-verify.log`, `corrupt-{out,err}.log`,
`corrupt-verify.log` and `corrupt-artwork-verify.log`. During acceptance an initial
notice row displaced the canvas; the final installed build fixes its grid sizing
and passed both read-only and corrupt-settings scenarios.

Workspace/all-target/all-feature Clippy and the two existing API invariant tests
passed (`target/layout-clippy-final.log`, `target/layout-core-check.log`). The
release install passed (`target/layout-install-final.log`). No new automated
tests were added for this preferences/UI integration. This does not establish
physical pen continuity, latency percentiles, crash durability under power loss,
or non-Windows behavior. Toolbar entry docking, side stack/fill behavior and
pointer capture still need their own implementation and acceptance.
