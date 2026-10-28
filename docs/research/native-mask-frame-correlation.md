# Scratch-only native frame correlation

Set `NAYATI_MASK_PRESENT_TRACE=1` together with the existing authorized mask scratch
probe. The exact scratch filename, marker file, and probe mode guards still apply.
The surface must support a four-byte format and COPY_SRC. Ordinary projects create
no barcode pipeline, draw no marker, and perform no readback.

After the normal presenter, a 60-by-12 physical-pixel marker is drawn at child-local
`[4,4]`. Its ten six-pixel cells are: magenta prefix, eight binary cells most
significant bit first (white=1, black=0), cyan suffix. The value identifies the
actual present serial, including frames while prior readbacks are pending and
close-overlay retained presentations. For example, `00000101` identifies serial 5.
The marker is omitted if its rectangle could overlap the transformed page bounds;
it never changes document pixels or the retained scene texture.

The asynchronous one-pixel copy samples document point `[8.5,8.5]` after the marker
draw in the same diagnostic submission. Logs carry serial, Unix milliseconds,
monotonic elapsed microseconds, geometry epoch, child-local sample position, marker
presence, sample admission, format, and surface dimensions. The later readback line
repeats that frame's context. Up to four pending reads allow consecutive frames to
be sampled without waiting. Match the screenshot marker to `sampled=true` and its
readback serial; a newer `sampled=false` marker (pool full or sample budget exhausted)
must not be attributed to an older pending sample. Screenshot coordinates
additionally require the canvas client origin within the captured window.

Bounds are 32 sampled pixels, at most four pending 256-byte staging buffers, at most
255 marked serials, and at most 128 wake requests only while readback is pending.
There is no blocking poll, full readback, timer, or redraw request after the pending
readback completes. Project activation and Save As disable the probe. The marker
adds a tiny GPU pass and thus may perturb presentation; a passing marked run does
not establish that uninstrumented presentation is fixed.

This diagnostic correlates submitted contents with capture output. Neither the
pre-present timestamp nor asynchronous completion is physical-display timing.

## First marked observation

The root session captured serial 7 with a solid red page. A menu click produced a
capture with serial 9 and the gray mask hole, then an Escape-following capture
still showed serial 9 and the menu but had a solid red page. The corresponding log
sampled serial 8 gray; serial 9 had `sampled=false` under the former one-pending-read
limit. Thus two captures bearing the same marker showed different hole states,
but neither has a matching serial-9 sample. Possible capture latency and partial
composition remain unresolved. The four-slot pool was added to obtain direct
matching-frame evidence, not to treat this observation as proof of a root cause.

## Matching serial-6 evidence with the four-slot pool

In scratch process PID35108, the root session's initial capture showed the correct
hole with marker `00000101` (serial 5). After opening the mask menu, the capture
showed marker `00000110` (serial 6) and solid red at window pixel `[751,346]`.
`barcode-pool.stdout.log`, under the directory named in
`target/native-mask-current.txt`, has matching sampled records:

| Serial | Elapsed microseconds | Epoch | Surface size | Sample RGBA |
| --- | --- | --- | --- | --- |
| 5 | 110935 | 3 | 1238×857 | `[221,221,221,255]` |
| 6 | 56974230 | 3 | 1238×857 | `[221,221,221,255]` |

Both records have `marker=true` and `sampled=true`. The sampled document point is
`[8.5,8.5]`; its child-local surface position is approximately
`[438.2265625,247.7265625]`. The logged copy floors this to `[438,247]`; adding the
observed canvas origin `[313,99]` gives exactly `[751,346]`. The adapter was the
RTX 3080, DX12, Bgra8UnormSrgb, Mailbox. This was a menu-triggered sample roughly
56.9 seconds after serial 5, not an exhausted startup-only probe.

The visible serial and matching sample now establish a discrepancy between the
submitted surface contents and the captured composed window at that point.
The surface pixel was opaque gray, so transparent canvas alpha does not explain
this sample. This does not identify which post-surface composition or capture
stage supplied the red pixels, prove a driver defect, or establish physical
scanout. A marker and another region can still be composed from different retained
states; the marker intentionally makes that possibility observable.
