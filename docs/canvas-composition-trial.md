# Removed native canvas composition trial

The 2026-10-28 composition presentation trial failed to resolve the delayed native
mask display symptom. Its adapter, environment switch, COM guard, and Cargo feature
additions were removed. The existing HWND swapchain remains the presentation path.

In the final run (process 33612), the reopened scratch project's mask hole appeared
correct initially and immediately after a menu interaction. After more than ten
seconds of idle and a UI tooltip, the captured canvas again showed the unmasked red
image. The Greenshot overlay had already cleared. Initial correctness therefore did
not establish a fix. Native visual acceptance remains unresolved; automated artwork
and GPU surface checks are separate evidence. Independent OS capture could not be
obtained without a system privacy prompt, and no permission was changed.

The removed experiment used WGPU 26.0.1's DX12
`SurfaceTargetUnsafe::CompositionVisual` on the existing canvas child HWND, retaining
the device, target, and visual on the render actor. It committed after configuration
and reconfiguration, with an actor-local MTA guard released after the interfaces.
It added no readback, GPU wait, browser GPU sharing, frame copy, or input change.
The trial was opt-in; the default never changed. No broader renderer rewrite or
promotion followed this failed acceptance.

The separate WebView2 `ParentWindow` correction remains: its controller names the
actual HWND hosting its visual tree. The raw canvas HINSTANCE lifetime checks and
backend diagnostics also remain. None is claimed to resolve this visual symptom.

References used to establish the trial's ownership contract:

- [Composition swapchain contract](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgifactory2-createswapchainforcomposition)
- [HWND visual target layering](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-createtargetforhwnd)
- [DirectComposition initialization sample](https://learn.microsoft.com/en-us/windows/win32/directcomp/initialize-directcomposition#complete-example)
