# ADR-0030: Godot PNG compatibility and focus-triggered refresh evidence

Status: accepted for the recorded Windows PNG integration case.

## Tool and scene

Use official Godot **4.6.3.stable.official.7d41c59c4**, Windows x64 portable,
only as a local acceptance tool. The archive SHA256 is
`e39986a178d585ce7ac198fb8de6ea436366dc0cc00e594810c2e3e104c04b90` (79,844,616 bytes),
verified against the official GitHub release asset digest before extraction.
The GUI executable SHA256 is
`ef90e929ba1a6a4322860285d97f40f4aa349c90329a91b0e8b55b8df0f4cb00`.
No Rust dependency, Godot addon, system installation or default handler was added.
[Release](https://godotengine.org/download/archive/4.6.3-stable/),
[command-line reference](https://docs.godotengine.org/en/4.6/tutorials/editor/command_line_tutorial.html),
[Image API](https://docs.godotengine.org/en/4.6/classes/class_image.html).

Host: Windows 11 Home 10.0.26200 / Core Ultra 7 155H / Intel Arc.
NyatiDraw uses pinned DX 0.7.9 release / DX12, source 047cf6f, installed SHA256
`ac6bed6e68c59d82e9006516fcf8d459294bf4dd76932ab1e7780167cb2f40c6`.
Godot's live editor used Compatibility with reported native OpenGL 3.3.
The user's download remained untouched; desktop/background load was uncontrolled.

## Actual acceptance

1. After Windows unlocked, the installed NyatiDraw opened the legacy page
   scratch, saved, closed normally, restarted and closed again. Separate-process
   checks retained snapshot 1/history 2, all original signed/locked/hidden tiles,
   page/PPI and independent PNG pixels. `target/color-ui/legacy/` has both sessions
   and `verify.log` / `reopen-verify.log`.
2. Godot headless import and the committed `tools/godot/verify_png_colors.gd`
   script loaded the 4K 16-bit file and 1px 8-bit preview as raw PNG and normal
   imported textures. Both produced RGBA8 [255,188,137,128] at the independently
   specified non-primary translucent pixel. The script allows one encoded-byte
   tolerance but this run matched exactly. This is not a full-image Godot oracle.
3. In `target/godot-color-acceptance/art/async-edit-scratch.ntdr`, the installed
   app filled a blank 129x65 page with UI #1AC7E8. Save/normal Close and the
   separate native-edit verifier confirmed every page pixel [3,146,206,255],
   unchanged signed/outside artwork and snapshot 2/history 2. A normal app
   restart visibly restored the fill.
4. A live Godot editor opened that colocated PNG as CompressedTexture2D, showing
   the solid fill and 129x65 RGBA8 in the Inspector. In NyatiDraw, actual UI Undo
   restored the empty page; Wand selected all 8,385 pixels; Gradient dragged
   document x=10 to x=110, current color to transparent, then Save completed.
5. On returning focus to the same Godot editor, the Inspector and file preview
   changed to the gradient **without a manual reimport command or addon**.
   `editor-out.log` recorded `EditorFileSystem: Importing file:` for the exact
   PNG and a 25ms import observation. This is one focus-triggered refresh, not
   continuous background watcher latency or a percentile result.
6. Both apps closed normally. The native verifier checked snapshot 3/history 3,
   every gradient pixel via independent rational interpolation, all outside
   artwork and PNG. Another ordinary NyatiDraw restart restored the gradient;
   closing it and re-running verification passed again.
7. A separate Godot headless process loaded the existing editor import cache,
   without a new `--import`. At (0,32) it returned [28,199,232,255], at (60,32)
   [22,199,232,126], and the far endpoint retained zero alpha. This agrees with
   the independent canonical linear-pixel expectations and proves that the
   editor cache, not merely a displayed filename, contains the updated artwork.

The final colocated PNG SHA256 is
`be5b112d4d4ceaf1200e84ec9e4e9cbfddb16cb421d3260d334151d553163dde`.
Root log files are `target/godot-color-{import,verify}.log`,
`godot-fill-import.log`, `godot-native-gradient-verify.log`. Under the scratch
project, preserve `nyati-*.log`, `editor-{out,err}.log`, `fill-verify.log`,
`gradient-verify.log` and `reopen-verify.log`. The final verification script run
has no decode/import errors; the verbose GUI log also retains engine startup
controller-mapping and shutdown StringName diagnostics rather than filtering them.

## Limits

Computer Use injects mouse/keyboard actions. This does not prove physical pen,
first-visible-pixel timing, calibrated display color, other Godot versions or
render backends, exported-game packaging, or long-session watcher reliability.
The linear8 first-import quantization constraint in ADR-0029 still applies.
This run adds no automated UI tests. Separate installed new-color native brush
and fresh 16-bit PNG bootstrap acceptance subsequently passed; see ADR-0029.
