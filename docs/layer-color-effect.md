# Rough-layer color effect

## Purpose and scope

The raster layer color effect makes a dark rough drawing easier to distinguish
from new line art without repainting its pixels. It is not a layer-row color tag,
alpha-lock recoloring, a destructive correction, or a claim of Clip Studio Paint
algorithm parity. Groups do not have this effect in this slice.

The initial dark color is blue (`#6090e0`), and the light color is white. Both can
be changed in compact swatches below the layer controls. The layer color button
toggles the effect; disabling it retains the chosen colors in the project.
New layers and old documents have no effect until the user enables it.

## Pixel contract

- Stored premultiplied linear RGBA8 pixels are never rewritten by this effect.
- For composition, recover straight sRGB8 and calculate a Rec.709-weighted luma
  in that sRGB domain. Interpolate from the dark endpoint to the light endpoint,
  quantize to sRGB8, then convert back to premultiplied linear RGBA8.
- Preserve the original alpha exactly. A disabled or absent effect bypasses the
  operation without any color round trip.
- Apply it once to each raster source before its opacity, clipping, and blend
  mode. Do not apply it again to accumulated clipping stacks or group output.
- CPU and GPU use the same bounded transfer tables and integer rounding rules.

The canvas, navigator, PNG export, visible-layer eyedropper, and reference-layer
composition include the effect. Explicit raw active-layer sampling and raster
thumbnails remain raw, following the existing clipping convention. The pressed
color button indicates that the selected raster has an enabled display effect.
Selection from raster alpha, transformations, brush strokes, and destructive
color correction continue to operate on the original pixels.

## Persistence and history

The effect is document metadata, not a global preference. Each actual change is
one structural Undo step and participates in normal save/export scheduling.
Undo/Redo restores the full configuration, including its enabled flag; toggling
does not change the raw tile content root.

Unconfigured trees retain the existing version-3 layer encoding. Configured trees
use version 4 with a capability flag, including disabled retained configurations.
Older layer encodings decode with no effect. Readers that do not understand the
new capability must reject it instead of silently dropping artwork appearance.

## Acceptance

Implementation acceptance must cover independent pixel cases, CPU/GPU agreement,
original-pixel preservation, clipping/opacity/blend interaction, metadata Undo,
and a saved file opened in a fresh process. Compile success alone is not evidence
of those properties. [Local acceptance results](measurements/layer-color-effect.md)
record core/persistence checks, CPU/GPU comparison, actual browser controls, and
save followed by a fresh-process reopen and identical exported pixels.

This is not a physical-pen, high-refresh, or cross-platform acceptance claim.
