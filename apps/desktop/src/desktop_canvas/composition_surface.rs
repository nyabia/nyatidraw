use std::{marker::PhantomData, rc::Rc};

use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::DirectComposition::{
            DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionRectangleClip,
            IDCompositionTarget, IDCompositionVisual,
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    },
    core::{IUnknown, Interface},
};

use crate::render_host::Geometry;

pub(super) struct CompositionSurface {
    visual: IDCompositionVisual,
    clip: IDCompositionRectangleClip,
    target: IDCompositionTarget,
    device: IDCompositionDesktopDevice,
    _apartment: ActorMta,
    geometry: Option<Geometry>,
}

impl CompositionSurface {
    pub(super) fn new(parent: HWND) -> Result<Self, String> {
        let apartment = ActorMta::new()?;
        // SAFETY: the actor retains the parent HWND until its surface and all
        // composition objects drop. The WebView owns the separate topmost tree.
        unsafe {
            let device: IDCompositionDesktopDevice =
                DCompositionCreateDevice2(None::<&IUnknown>)
                    .map_err(|error| format!("create canvas composition device: {error}"))?;
            let target = device
                .CreateTargetForHwnd(parent, false)
                .map_err(|error| format!("create parent canvas target: {error}"))?;
            let visual = device
                .CreateVisual()
                .and_then(|visual| visual.cast::<IDCompositionVisual>())
                .map_err(|error| format!("create canvas composition visual: {error}"))?;
            let clip = device
                .CreateRectangleClip()
                .map_err(|error| format!("create canvas composition clip: {error}"))?;
            clip.SetLeft2(0.0)
                .and_then(|()| clip.SetTop2(0.0))
                .and_then(|()| visual.SetClip(&clip))
                .map_err(|error| format!("attach canvas composition clip: {error}"))?;
            Ok(Self {
                visual,
                clip,
                target,
                device,
                _apartment: apartment,
                geometry: None,
            })
        }
    }

    pub(super) fn create_surface(
        &self,
        instance: &wgpu::Instance,
    ) -> Result<wgpu::Surface<'static>, String> {
        // SAFETY: this valid IDCompositionVisual pointer receives a WGPU-owned
        // reference; the renderer drops its surface before this owner.
        unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                self.visual.as_raw(),
            ))
        }
        .map_err(|error| format!("create WGPU composition surface: {error}"))
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "Win32 physical bounds feed DirectComposition's f32 geometry API"
    )]
    pub(super) fn geometry(&mut self, geometry: Geometry) -> Result<bool, String> {
        if self.geometry == Some(geometry) {
            return Ok(false);
        }
        // SAFETY: every interface belongs to this actor's live device. The
        // UI publishes physical parent-client coordinates, not CSS coordinates.
        unsafe {
            self.visual
                .SetOffsetX2(geometry.origin[0] as f32)
                .and_then(|()| self.visual.SetOffsetY2(geometry.origin[1] as f32))
                .and_then(|()| self.clip.SetRight2(geometry.width as f32))
                .and_then(|()| self.clip.SetBottom2(geometry.height as f32))
                .and_then(|()| {
                    if geometry.visible && geometry.width > 0 && geometry.height > 0 {
                        self.target.SetRoot(&self.visual)
                    } else {
                        self.target.SetRoot(None::<&IDCompositionVisual>)
                    }
                })
                .map_err(|error| format!("update canvas composition geometry: {error}"))?;
        }
        self.geometry = Some(geometry);
        Ok(true)
    }

    pub(super) fn commit(&self) -> Result<(), String> {
        // SAFETY: the actor owns this live device and serializes its changes.
        unsafe { self.device.Commit() }
            .map_err(|error| format!("commit canvas composition: {error}"))
    }
}

impl Drop for CompositionSurface {
    fn drop(&mut self) {
        // SAFETY: the surface has dropped and the parent HWND is still retained.
        unsafe {
            let _ = self.visual.SetContent(None::<&IUnknown>);
            let _ = self.target.SetRoot(None::<&IDCompositionVisual>);
            let _ = self.device.Commit();
        }
    }
}

struct ActorMta(PhantomData<Rc<()>>);

impl ActorMta {
    fn new() -> Result<Self, String> {
        // SAFETY: S_OK and S_FALSE both acquire a count balanced on this thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| format!("initialize canvas composition COM: {error}"))?;
        Ok(Self(PhantomData))
    }
}

impl Drop for ActorMta {
    fn drop(&mut self) {
        // SAFETY: this thread-bound guard drops after every composition interface.
        unsafe { CoUninitialize() };
    }
}
