# Product

<!-- impeccable:product-schema 1 -->

## Platform

Windows-first desktop, with a separately bounded browser edition using the same
Dioxus editor UI, editing core and `.ntdr` format. Native macOS and Linux remain
deferred. See [current priorities](docs/sprints/current-priorities.md) and
[web support boundaries](docs/web-support.md).

## Stack

Rust workspace. Dioxus Desktop owns the WebView editor chrome while a native
Windows child HWND owns raw pointer input and the wgpu canvas. The core document,
brush, history, and project layers do not depend on the UI framework.

## Users

Primary user: a digital artist who wants to open a PNG beside a Godot project through the Windows image-opening workflow, draw immediately with a pressure-sensitive pen, and save both the paired editable source and a game-ready export without manual synchronization.

The active workflow also covers illustration practice: rough sketch, shape
correction, linework, flat color and shading. A public Windows alpha and a web
experimental edition exist; publication is not a claim of production readiness.
Vector layers, animation and timelapse remain later work.

## Product Purpose

NyatiDraw is a cross-platform raster-first drawing application built for low startup latency, low ink latency, and dependable save/export behavior. Opening an empty project file initializes it; saving durably records the editable project and schedules its colocated export; closing waits only for the latest required durable work.

## Positioning

The differentiator is one continuous native workflow from pen sample to GPU-visible ink to durable project snapshot and colocated engine-ready export. The UI framework is replaceable; the drawing engine and project format are the product.

## Operating Context

- Desktop-first use with pen tablets and high-frequency stylus input.
- Editable project files live beside exported PNG or other game assets.
- NyatiDraw is registered as a Windows PNG `Open with` application. Opening a PNG resolves an exact sibling `.ntdr` before creating an imported project.
- Godot may watch and reload exported files while the editor remains open.
- A dedicated Godot editor addon is backlog outside the active sprint plan.
- Projects can become large and should load visible content lazily.
- Users expect Clip Studio-like brush dynamics and docking behavior, without requiring feature parity at launch.

## Capabilities and Constraints

- Raster-first; vector, animation, advanced image editing, embedded-web integrations, and canonical Git export are later layers.
- Starting a desktop drawing does not require choosing a save location. Local
  Sketchbook storage supplies recovery; Save As assigns a user-chosen destination.
- Pressure support is mandatory. Tilt, twist, eraser, and barrel buttons are capability-detected.
- Native desktop on Windows, macOS, and Linux is the product target; backend coverage may ship in stages.
- Fast launch, responsive drawing, and bounded shutdown outrank Git-friendly storage.
- The project file may be binary and single-file. A zero-byte project file must initialize safely; an invalid non-empty file must never be overwritten automatically.
- Save and export are linked in the user experience but remain separate failure domains internally.
- Dioxus Desktop is a replaceable WebView shell; the Windows child HWND and WGPU canvas retain
  the hot input/render loop so UI framework churn cannot own artwork or latency authority.

## Evidence on Hand

The Windows alpha includes bounded native input, WGPU live ink and compositing,
immutable signed tiles, redb reopen/history, paired PNG export, file associations,
and an installer/update feed. Basic selection, affine transform, fill, gradient,
layer groups, alpha lock and clipping are implemented. Desktop and web share
editor commands; browser clipboard, history navigation and file access retain
explicit host limitations. [Release evidence](docs/releasing.md) and
[workflow gates](docs/illustration-workflow.md) distinguish implementation,
local acceptance, public deployment and unverified hardware behavior.

## Product Principles

1. Ink reaches the screen before application chrome gets a vote.
2. The latest durable artwork is never coupled to export success.
3. Hot paths are bounded, allocation-aware, and observable.
4. Replaceable shells surround stable document and drawing contracts.
5. Advanced features earn their place after the first complete stroke-to-reopen loop works.

## Open Decisions

- The product name is `NyatiDraw` and its editable project extension is `.ntdr`.
- Dependency versions are locked by `Cargo.lock`; Vello remains a later adapter decision.
- Linux/macOS input and window adapters remain deferred, not implicitly supported.
- Current raster storage is 128px RGBA8 tiles. Wider gamut, higher bit depth and
  advanced color management require separate format and rendering decisions.
