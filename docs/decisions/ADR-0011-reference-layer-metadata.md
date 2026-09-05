# ADR-0011: Durable raster Reference membership

- Status: Accepted for metadata; selection/fill semantics are subsequent work
- Date: 2026-09-05

Reference marks raster layers that later selection/fill tools can use as a source
set. It is a boolean on `LayerNode`, distinct from visibility, opacity, lock,
active selection and Solo. Multiple rasters may be marked. Groups are not marked;
their descendant rasters can be marked individually. Ordinary rendering, brush
destination and PNG export ignore this flag. The toolbar acts on the active
raster, and each marked raster row shows its membership.

`SetReference` uses the existing immediate tree/history transaction. Undo/Redo
restores membership, and subsequent stroke snapshots inherit it. Rejected unknown
IDs leave the tree unchanged. This does not enable Wand/Fill or decide their
tolerance, visibility filtering or empty-source fallback; those contracts must be
specified with the tool implementation before activation.

Layer payload version 2 adds one canonical boolean byte after each raster's
locked byte. Group records and the outer record envelope are unchanged. The
decoder accepts versions 1 and 2; version 1 assigns `reference=false` without
consuming a byte. Unsupported versions and noncanonical booleans are rejected.
New tree writes use version 2; opening alone does not rewrite the project.
The project history marker remains 2. Earlier readers reject the unknown layer
payload version during validation, including records on noncurrent branches,
instead of opening and silently dropping Reference data. No dependency changed.

One core invariant test uses an independently constructed version-1 byte fixture,
checks its default membership, version-2 round-trip, unknown-ID preservation,
invalid boolean values and unsupported versions. The existing atomic layer
history test also checks Reference inheritance and branch restoration. Workspace
tests total 61 and all-feature/all-target Clippy passes with `-D warnings`.

The installed DX 0.7.9 release extends the 20-raster/two-level group fixture with
Reference commit, Undo and explicit Redo across process restarts. Reopened tree,
tiles and projected Reference count are checked; PNG pixels are compared with the
unmarked tree. Logs: `target/reference-tests.log`, `target/reference-inheritance-test.log`,
`target/reference-clippy.log`, `target/installed-reference.log`. Host: Windows 11
Home 10.0.26200 / Core Ultra 7 155H / Intel Arc integrated / DX12 release desktop.
This is semantic command/restart evidence, not direct toolbar interaction or
physical-pen/latency evidence.

If tool-source semantics later need group-level membership, extend the tagged
payload contract explicitly; do not reinterpret this raster flag or visibility.
