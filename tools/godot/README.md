# Godot PNG compatibility acceptance

`verify_png_colors.gd` is a scratch command-line acceptance script, not an editor
addon. It checks decoded PNG and normal imported Texture2D pixels independently
of NyatiDraw's PNG decoder. It does not drive either application's UI.

The recorded run uses official Godot **4.6.3.stable.official.7d41c59c4** for Windows
x64, unpacked under `.nyatidraw/toolchains/godot-4.6.3/`. Download from the
[official release](https://godotengine.org/download/archive/4.6.3-stable/).
The `Godot_v4.6.3-stable_win64.exe.zip` SHA256 from the official release API is
`e39986a178d585ce7ac198fb8de6ea436366dc0cc00e594810c2e3e104c04b90`.
The archive is 79,844,616 bytes. Verify the checksum before extracting/running;
no user-wide installation or file-association change is required.

Prepare a fresh scratch Godot project with `project.godot` and an `art/` folder.
Use `color_export_probe` from the PNG crate to generate the fixed 4K PNG and
1px preview, then copy them as `art/color.png` and `art/preview.png`. Copy this
script into the scratch project as `verify.gd`. Run the pinned Godot console exe:

```powershell
& $godot --headless --path $scratch --import
& $godot --headless --path $scratch --script verify.gd
```

The optional `-- --native-gradient` checks a saved 129x65 native-edit fixture at
`art/async-edit-scratch.png`. That fixture must first undergo the actual UI
sequence in ADR-0030 and be imported by Godot. This mode intentionally does not
invoke `--import`: it checks the cache already produced by the editor.

A live editor focus-triggered refresh is a separate Computer Use acceptance,
not something this script proves. See [ADR-0030](../../docs/decisions/ADR-0030-godot-png-acceptance.md)
for the recorded UI sequence, hashes, outcomes and limitations.
