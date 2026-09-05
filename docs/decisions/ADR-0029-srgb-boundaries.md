# ADR-0029: Explicit sRGB boundaries with precise PNG transport

Status: implementation checkpoint; installed acceptance and export cost pending.

The [color-boundary audit](../color-boundary-audit-2026-09-05.md) reproduced
linear tile bytes being emitted directly as untagged PNG and UI sRGB bytes
being interpreted as linear brush color. Existing stored tile bytes, stroke
colors, roots, history and schema remain unchanged.

New UI colors decode sRGB before premultiplication and one RGBA8 quantization.
GPU straight brush uniforms derive from that same canonical premultiplied
color, including translucent rounding. Selection/fill/gradient gestures receive
the same canonical color. The shared transfer functions live in the existing
tiles crate; its 256-entry UI lookup is bounded and no pow runs per dab.

File export writes straight **16-bit sRGB PNG**, with a relative-colorimetric
sRGB tag. This is transport precision for existing 8-bit linear tiles, not a
16-bit painting engine. A fixed 128 KiB lookup converts channel/alpha pairs;
encoding streams one row to avoid a second full-size 16-bit image. Both stream
completion and final IEND/flush must succeed before the unchanged sibling-temp
and generation-guarded export replacement can proceed. Disposable Navigator
and thumbnail PNGs use tagged 8-bit sRGB and may quantize display values.

Import retains 16-bit decoded precision through inverse transfer and
premultiplication. Untagged images explicitly assume sRGB; sRGB tags override
lower-priority gamma/chromaticity hints. Gamma-only data uses its power-law
transfer and assumes sRGB primaries unless matching cHRM is present. Non-sRGB
primaries, ICC profiles and cICP metadata fail explicitly before artwork import.
There is no general profile converter or HDR support in this milestone.
Sources: [sRGB transfer](https://www.w3.org/Graphics/Color/srgb),
[PNG color chunk rules](https://www.w3.org/TR/png-3/).

All 32,896 valid channel/alpha pairs now pass the production file encoder and
16-bit decoder with exact premultiplied-byte recovery in the existing PNG
roundtrip test. Independent raw encoded midpoint (linear 0.5 -> sRGB16 48192),
alpha and color-tag assertions prevent equally wrong forward/inverse paths
from passing. One small metadata invariant test and one UI-color invariant test
were added; existing flush-failure preservation remains covered. Workspace
tests and all-target/all-feature Clippy with warnings denied passed in
`target/color-workspace-tests.log` and `target/color-clippy.log`.

Arbitrary external sRGB8 pixels still quantize on first import into the existing
linear8 tile format. This cannot promise byte-exact original-source PNG
roundtrips, display calibration, wide-gamut accuracy or Godot acceptance.
Installed non-primary translucent artwork, legacy project preservation,
independent PNG verification, actual restart and export cost remain required.
Earlier 4K measurements describe the old color/export path, not this change.

No dependency versions changed; PNG remains pinned at 0.17.16.
