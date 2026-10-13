# Layer color effect acceptance

Local source only at verification time; no commit, push, release, installation, or user
artwork mutation. These are correctness probes, not performance measurements.

## Core and persistence

- API/document/editor/project/native-storage suites: 52 tests passed.
- Project-web suite: 12 passed, including a scratch file read in a separate native
  process. Configured-but-disabled colors survived save/reopen and Undo/Redo.
- Web-core suite: 10 passed, including effect-only single-history commits, no-op
  rejection, original pixel/alpha preservation, and portable codec recovery.
- Tiles and CPU-paint suites: 6 and 35 passed. Checks cover black/white endpoints,
  independent red-channel mapping, all alpha values, disabled byte identity,
  clipping, opacity, Multiply, nested groups, reference/Solo, and signed pixels.
- Version 1–3 layer records remain readable with no effect. Version 4 metadata
  requires the new capability flag; removing either required compositing flag
  rejects the scratch project. The existing capability check was retained.
- Internal portable browser records read NYWEB001 and NYWEB002. User-facing Save
  and automatic recovery still use the shared `.ntdr` container.

## GPU

Windows, NVIDIA GeForce RTX 3080, Vulkan, debug profile. Run:

```text
cargo run -p nyatidraw-paint-gpu --example gpu_layer_color --locked
cargo run -p nyatidraw-paint-gpu --example gpu_shading --locked
```

The color probe compared 4,915,200 composite pixels with CPU output: maximum byte
difference 0. It changed the same scene through absent/default/custom/disabled/
removed effects, covering cache invalidation, 32px page-boundary and 128px page
products, negative/off-page sparse tiles, opacity, nested Multiply clipping, and
Solo. Read-back raw raster tiles remained byte-identical to the input snapshot.

The existing shading probe passed all 40 cases: raw composition exact, presentation
read-back difference at most 1 byte. This is GPU offscreen read-back evidence, not
an HWND present, first-visible-pixel, physical-pen, or another backend claim.

## Integrated acceptance

Shared editor UI and Worker were built with `tools/build-web.ps1 -DebugBuild
-BasePath /` and served only on loopback. An isolated Edge profile imported the
existing layered scratch fixture, selected its reference line layer, enabled the
effect, changed the endpoints to `#e05030` and `#f0e0c0`, and exercised disable,
enable, Undo, and Redo through the actual controls. Canvas and navigator showed
the effect; the source raster thumbnail remained unchanged as specified.

Saving produced `output/playwright/layer-color-final.ntdr` (282,624 bytes).
The browser was closed and a different fresh profile opened that file. Both
endpoint colors and the enabled flag survived, alongside reference, clipping,
Multiply, and alpha-lock metadata. PNG exports confirmed:

- Original, disabled, and Redo-after-disable: SHA-256
  `626C4BE0CA542810F822EC88B3FFF0E92EDF8D6C9FCACF58F0907EB55F3E725D`.
- Custom effect, Undo-after-disable, and fresh-process reopen: SHA-256
  `F286D7D37848E02D00BE5E1B2102B84BEF332CC331311A3115A0EA5BA7A1E722`.

The newly built native CLI also validated that browser-written file in a separate
process. Screenshots and fixture exports are local ignored acceptance artifacts
under `output/playwright/layer-color-*`.

Final integrated checks passed:

```text
cargo test --workspace --all-features --locked --quiet
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy -p nyatidraw-web -p nyatidraw-web-worker --target wasm32-unknown-unknown --locked -- -D warnings
cargo fmt --all --check
```

The workspace test run retained five ignored desktop hardware/runtime probes;
they are not counted as passed. The explicit offscreen GPU probes above are
separate evidence. Physical pen, desktop window presentation, other OSes, and
Safari/iPad remain unverified. Existing dependency lifetime warnings and the
browser's favicon/power-preference messages do not constitute new app failures.
