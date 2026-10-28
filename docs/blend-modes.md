# Illustration light and contrast blend modes

Local implementation, 2026-10-28. This extends the existing W4 shading/W5 finishing
workflow with Screen, Add, and Overlay on raster layers and isolated groups.
Normal and Multiply remain available. The scope is a small useful painting set,
not a competitor menu checklist.

## Why these three

- **Screen / 스크린** adds soft light and highlights while black contributes no
  change in the overlapping area. Krita and Adobe describe its inverse-multiply
  construction. [Krita][krita] [Adobe][adobe]
- **Add / 더하기** adds light channels with saturation at white, useful for strong
  highlights. This is a clamped color blend followed by ordinary alpha
  composition, not CSP's distinct Add (Glow) treatment. [CSP][csp]
- **Overlay / 오버레이** strengthens contrast or adds a finishing color wash using
  the backdrop channel to choose Multiply versus Screen behavior. [Adobe][adobe]

Soft Light has multiple documented variants, and the remaining competitor modes
do not address a separate demonstrated workflow need in this slice. They remain
deferred. [Krita's Soft Light variants][krita]

## Pixel contract

Working pixels remain **linear-light premultiplied RGBA8**. These modes operate in
that existing space. Their names do not promise pixel equality with another
application using a different color profile, gamma, precision, or clipping model.
The general alpha-composition equations and separable Screen/Overlay definitions
follow the [W3C compositing model][w3c].

For byte source channel `s`, backdrop channel `d`, and their alpha bytes `a`, `b`,
after source opacity is rounded once, the overlap numerator is:

| Mode | Alpha-weighted overlap numerator |
|---|---|
| Normal | `s*b` |
| Multiply | `s*d` |
| Screen | `s*b + d*a - s*d` |
| Add | `min(s*b + d*a, a*b)` |
| Overlay, `2*d <= b` | `2*s*d` |
| Overlay, otherwise | `a*b - 2*(a-s)*(b-d)` |

Source-over adds `s*(255-b) + d*(255-a)` to that numerator; source-atop clipping
adds only `d*(255-a)`. Both round once with `(numerator+127)/255`. Source-atop
keeps the backdrop alpha. Source-over uses the original over-alpha equation.
No channel is divided by a small alpha, and the output is bounded by its alpha.
The original Normal/Multiply byte rounding remains algebraically identical.

CPU export, picker, preview and GPU display use these same integer equations.
Existing clipping stacks, isolated group boundaries, Solo dependencies, base
opacity application, and signed workspace rules are unchanged. Mode changes
are layer metadata edits; original raster tiles and color effects are retained.

## Save compatibility

The native layer-tree v3/v4 records and portable NYWEB001/NYWEB002 records retain
their field layout. Blend tags are explicit: Normal `0`, Multiply `1`, Screen `2`,
Add `3`, Overlay `4`. No Rust enum ordinal is serialized. Existing files remain
readable, and old Normal/Multiply records encode identically. Unknown tags fail
decoding rather than falling back to Normal. Older applications therefore cannot
open new-mode files; this is backward read compatibility, not forward support.
Web NTDR uses the same native project layer-tree codec. No dependency was added.

## Verification boundary

Added automated coverage:

- Byte oracles for partial alpha, clipping, opacity, transparent backdrops,
  both Overlay branches, and saturated Add.
- Explicit wire tags, preserved legacy fixtures, and rejected unknown tags.
- Portable save/reopen preserving raw tiles, metadata, and exported pixels.
- Native-to-web-to-native NTDR handoff with a separate process reopening each
  new mode, checking raw content hash, group/raster metadata, and RGBA export.
- The real-device `gpu_shading` probe now compares CPU/GPU exact raw bytes for
  each new mode as a raster/group, with opacity, clipping, Solo, page and signed
  workspace coordinates.

Integration commands (execution/results are recorded by the integration owner):

```powershell
cargo test -p nyatidraw-paint-cpu byte_blends_preserve_transparent_backdrops_and_atop_alpha
cargo test -p nyatidraw-project reference_metadata_preserves_legacy_layers_and_rejects_malformed_membership
cargo test -p nyatidraw-web-core portable_blend_modes_preserve_raw_artwork_and_composite_after_reopen
cargo test -p nyatidraw-project-web blend_modes_survive_native_web_native_process_restart_without_repainting
cargo run --release -p nyatidraw-paint-gpu --example gpu_shading
```

Compilation, Clippy, browser/desktop dropdown interaction, and actual execution
of these checks must be confirmed separately. Source/test additions alone are
not runtime evidence. Physical pen, first-visible-pixel latency, installed app,
high-refresh display, and other platform/backend acceptance remain unverified.

[krita]: https://docs.krita.org/en/reference_manual/blending_modes/lighten.html
[adobe]: https://helpx.adobe.com/photoshop/desktop/repair-retouch/adjust-light-tone/blending-mode-descriptions.html
[csp]: https://help.clip-studio.com/en-us/manual_en/180_layers/Blending_modes.htm
[w3c]: https://www.w3.org/TR/compositing-1/#blending
