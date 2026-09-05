# ADR-0031: Retain unchanged GPU tiles during history adoption

Status: implemented; installed before/after acceptance in progress.

Undo/Redo and asynchronous edits previously uploaded the union of every old
and new CPU tile, including byte-identical tiles. History cursor moves also
reconciled an identical tree, invalidating the complete composite cache. This
work runs on the Windows UI thread and can delay subsequent input dispatch.

When the page dimensions and full layer tree are unchanged, history adoption
now retains existing raster surfaces and composite caches. Only byte-different
tiles are uploaded. A removed tile still receives a zero upload, because the
union includes old keys. CPU authority is still replaced by the complete
validated snapshot; durable history transactions and input pause/adoption
barriers are unchanged.

Any page-size or tree difference uses the existing conservative full upload
path. It must rebuild restored layers and pixels revealed by page growth, even
when their CPU bytes compare equal. Selection clearing, active-layer fallback,
and fatal adoption failure behavior remain unchanged. There is no render-thread
move or change to surface presentation in this decision.

The before measurement used the installed sRGB release, SHA256
`ac6bed6e68c59d82e9006516fcf8d459294bf4dd76932ab1e7780167cb2f40c6`,
on Windows 11 Home 10.0.26200 / Core Ultra 7 155H / Intel Arc DX12. The maximized
app rendered a 2096×1458 surface at scale 2 on the configured 2880×1800/120Hz
display. A copy of the historical 3840×2160, two-raster, 32-stroke campaign
project retained its original stored colors, 902 tiles and 33 history nodes.
Ten UI Undo/Redo pairs completed, followed by Save and normal Close.

The existing `history_adoption` CPU span measured 20 restorations, including
the first one: p50/p95/p99 histogram upper bounds 53.247/55.295/56.138ms,
minimum 45.434ms, maximum 56.138ms. This excludes command queue/worker time
before adoption and composition/presentation afterward; it is not end-to-end
Undo latency. No warmup was excluded. The user-owned background download was
uncontrolled and was neither inspected nor altered.

`desktop_performance_fixture verify-reference` separately compared the copied
current snapshot, node count, every tile, tree/page and every decoded export
pixel against the original scratch. This avoids applying today's UI color
transfer to the historical workload. Before logs are under
`target/history-upload/before/`.

All-feature desktop tests (15 existing tests) and all-target/all-feature
workspace Clippy passed. No tests or dependencies were added for simple byte
equality or framework wiring. Installed after acceptance and measurements must
still verify the optimization and the page/tree fallback paths.
