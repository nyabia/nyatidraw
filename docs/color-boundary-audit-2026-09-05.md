# 2026-09-05 color-boundary audit

Status: reproduced software gap; correction not implemented or accepted yet.
This is part of the existing UI-color/export gates, not a new wide-gamut feature.

## Reproduced mismatch

The durable contract in `paint-cpu`, project-format.md and ADR-0003/0005 is
premultiplied **linear-light RGBA8**. The Rgba8Unorm working texture and sRGB
presentation surface honor that contract. Changing the presenter to treat those
bytes as encoded sRGB would reinterpret existing artwork and is not the fix.

`DrawingConfig::stroke_color` currently premultiplies UI color bytes without
inverse sRGB transfer; `gpu_color` divides those same bytes by 255. Selection
and gradient gestures inherit that color. PNG import also only premultiplies,
and PNG export/preview only unpremultiply. The encoder writes no color tag.
ADR-0013 already names UI sRGB conversion as a remaining gate.

An independent Python standard-library parser read the installed export at
`target/paint-redraw/export-3/performance-scratch.png`: IHDR is RGBA8; chunks are
IHDR/IDAT/IEND; the first pixel is `[32,32,32,255]`. The independent fixture's
background is linear `[32,32,32,255]`, which should encode to sRGB8
`[99,99,99,255]`. The first pixel has zero filter neighbors under every PNG row
filter, so the parser did not share production color conversion or compositing.
[Raw evidence](measurements/color-boundary-installed-export-2026-09-05.json).
This is file-byte evidence, not measured display colorimetry.

## Precision constraint

The [sRGB transfer reference](https://www.w3.org/Graphics/Color/srgb) defines
piecewise forward/inverse transfer. Alpha stays linear. Conversion must apply
inverse transfer before premultiplication and forward transfer after removing
premultiplication. An opaque linear byte 128 encodes to sRGB8 188; an sRGB8
byte 128 decodes to linear8 55.

A scalar Python experiment enumerated all 32,896 valid channel/alpha pairs
`0 <= channel <= alpha <= 255`, using round-half-up. For each pair it encoded
`srgb(channel/alpha)` to N-bit straight RGB, decoded it with the inverse
transfer, multiplied by the original alpha, and rounded to a tile byte. Zero
alpha maps to zero. [Arithmetic results](measurements/color-boundary-arithmetic-2026-09-05.json):

| Straight sRGB transport | Changed channel/alpha pairs | Maximum linear-byte error |
|---|---:|---:|
| 8-bit | 4,122 | 1 |
| 16-bit | 0 | 0 |

The first 8-bit counterexample is channel 111 / alpha 121: exported 246 returns
112. This proves that merely adding sRGB conversion to the existing RGBA8
export cannot satisfy exact tile-byte export/import for every valid tile color.
The 16-bit arithmetic result is not PNG codec or Godot acceptance. It requires
retaining decoded 16-bit precision until inverse transfer and premultiplication;
the current decoder's STRIP_16 explicitly prevents that.

Neither transport can restore information already quantized when an arbitrary
external sRGB8 image is first imported into linear8 tiles. A byte-exact original
PNG roundtrip for every possible source color cannot be claimed with this tile
contract. Existing pure-primary color fixtures did not expose this limitation.

## Concrete next implementation and acceptance

Keep existing tile bytes, content roots, stored stroke colors, schema and history
unchanged. Convert new UI colors at the semantic-to-artwork boundary; derive GPU
brush color from the same canonical premultiplied value as CPU replay to retain
existing tolerance. Core conversion belongs in an existing pixel-owning crate,
with bounded lookup storage and no per-dab allocation or transfer-function pow.

For the colocated export, first validate a tagged 16-bit sRGB PNG path and full
16-bit import precision so all existing valid tile values can survive a direct
export/import. Disposable web previews may use tagged 8-bit sRGB. Measure the
extra export memory/encode work before accepting this as the default; check
actual Godot import compatibility when its runtime is available. A fallback to
8-bit would need an explicit documented quantization contract, not a silently
weakened exact-roundtrip assertion.

Define source metadata behavior before connecting import: untagged assumption,
sRGB, gAMA/cHRM and unsupported ICC/HDR profiles must not silently take the same
byte path. The pinned png 0.17.16 exposes these metadata fields. Use
[PNG color-chunk precedence](https://www.w3.org/TR/png-3/) and constrain supported
profiles to this milestone. Do not introduce general ICC or HDR editing here.

Use a small core invariant test covering all valid channel/alpha pairs and
independent known non-primary encoded pixels. Extend the existing PNG recovery
checks rather than adding UI wiring tests. Verify a non-primary translucent
scratch through installed draw/fill/gradient, Save, normal Close, process restart,
reopen, independent PNG decode and import into a fresh sibling. Also reopen a
pre-correction project and prove all durable bytes/history stay exact. Keep
invalid-pair preservation and generation-safe temporary replacement intact.

The 4K measurements on source commit 08a9618 remain historical measurements of
the old export path; they cannot establish performance for a corrected color or
16-bit path. No application color behavior changed during this audit.
