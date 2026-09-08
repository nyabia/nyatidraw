//! Windows render actor. The HWND thread never borrows GPU state or waits for
//! a live actor. Each actor retains the Tao parent until its surface is gone.
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    desktop_canvas::CanvasSurfaceRenderer,
    live_ink::{CloseStatus, LiveInkBridge},
};

const ACTIVATION_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub epoch: u64,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            width: 1,
            height: 1,
            scale: 1.0,
            epoch: 0,
        }
    }
}

#[derive(Default)]
struct Pending {
    wake: bool,
    geometry: Geometry,
    controls: Controls,
    retire: bool,
    activations: VecDeque<PathBuf>,
}

#[derive(Default)]
struct Controls {
    close: bool,
    reopen: bool,
    save_as: bool,
}

impl Pending {
    fn take_batch(&mut self) -> Self {
        std::mem::replace(
            self,
            Self {
                geometry: self.geometry,
                retire: self.retire,
                ..Self::default()
            },
        )
    }

    fn activate(&mut self, path: PathBuf) -> Result<(), String> {
        if self.retire || self.controls.close || self.activations.len() >= ACTIVATION_CAPACITY {
            return Err("파일 열기 요청이 가득 찼거나 종료 중입니다".into());
        }
        self.activations.push_back(path);
        self.wake = true;
        Ok(())
    }
}

struct Inner {
    pending: Mutex<Pending>,
    changed: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
    started: AtomicBool,
    finished: AtomicBool,
    surface_retired: AtomicBool,
    parent: Weak<dioxus_desktop::tao::window::Window>,
    child: isize,
    live_ink: LiveInkBridge,
}

#[derive(Clone)]
pub(crate) struct RenderHost(Arc<Inner>);

/// Only VDOM-owned UI handles share this lease. HWND state and the actor do
/// not own it, so dropping the VDOM cannot create a parent/worker ownership cycle.
pub(crate) struct RenderLifetime(pub RenderHost);

impl Drop for RenderLifetime {
    fn drop(&mut self) {
        self.0.retire();
    }
}

impl RenderHost {
    pub(crate) fn new(
        parent: &Arc<dioxus_desktop::tao::window::Window>,
        child: isize,
        live_ink: LiveInkBridge,
    ) -> Self {
        Self(Arc::new(Inner {
            pending: Mutex::new(Pending::default()),
            changed: Condvar::new(),
            worker: Mutex::new(None),
            started: AtomicBool::new(false),
            finished: AtomicBool::new(true),
            surface_retired: AtomicBool::new(true),
            parent: Arc::downgrade(parent),
            child,
            live_ink,
        }))
    }

    /// Called during the root's initial render after `WebView` construction.
    /// Browser DOM mount and mutation acknowledgement have not yet been proven.
    pub(crate) fn start(&self) {
        if self.0.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(parent) = self.0.parent.upgrade() else {
            return;
        };
        if self
            .0
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retire
        {
            return;
        }
        self.0.finished.store(false, Ordering::Release);
        self.0.surface_retired.store(false, Ordering::Release);
        let inner = self.0.clone();
        let result = thread::Builder::new()
            .name("nyatidraw-render".into())
            .spawn(move || {
                // Keep this anchor outside the catch boundary. Every surface,
                // including one unwound by startup/render failure, drops first.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&inner)));
                if result.is_err() {
                    inner.live_ink.publish_workspace_error(
                        "Render worker panicked; the project must be reopened".into(),
                    );
                    inner.live_ink.publish_close_status(CloseStatus::Failed {
                        project_saved: false,
                        project_path: String::new(),
                    });
                }
                crate::performance::flush("render");
                println!(
                    "desktop-render event=surface-retired thread={:?}",
                    thread::current().id()
                );
                inner.surface_retired.store(true, Ordering::Release);
                drop(parent);
                inner.finished.store(true, Ordering::Release);
                inner.live_ink.request_redraw();
            });
        match result {
            Ok(worker) => {
                *self
                    .0
                    .worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(worker);
            }
            Err(error) => {
                self.0.finished.store(true, Ordering::Release);
                self.0.surface_retired.store(true, Ordering::Release);
                self.0
                    .live_ink
                    .publish_workspace_error(format!("Render worker could not start: {error}"));
            }
        }
    }

    fn update(&self, apply: impl FnOnce(&mut Pending)) {
        let mut pending = self
            .0
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply(&mut pending);
        pending.wake = true;
        drop(pending);
        self.0.changed.notify_one();
    }

    pub(crate) fn wake(&self) {
        self.update(|_| {});
    }
    pub(crate) fn geometry(&self, geometry: Geometry) {
        self.update(|pending| pending.geometry = geometry);
    }
    pub(crate) fn close(&self) {
        self.update(|pending| pending.controls.close = true);
    }
    pub(crate) fn reopen(&self) {
        self.update(|pending| pending.controls.reopen = true);
    }
    pub(crate) fn save_as(&self) {
        self.update(|pending| pending.controls.save_as = true);
    }
    pub(crate) fn retire(&self) {
        self.0.live_ink.begin_retirement();
        self.update(|pending| pending.retire = true);
    }

    pub(crate) fn activate(&self, path: PathBuf) -> Result<(), String> {
        let mut pending = self
            .0
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.activate(path)?;
        drop(pending);
        self.0.changed.notify_one();
        Ok(())
    }

    /// Never join until the OS thread has actually finished. The atomic also
    /// covers the not-yet-started and failed-spawn cases without a worker handle.
    pub(crate) fn finished(&self) -> bool {
        if !self.0.finished.load(Ordering::Acquire) {
            return false;
        }
        let mut worker = self
            .0
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return false;
        }
        if let Some(worker) = worker.take() {
            let _ = worker.join();
        }
        true
    }

    pub(crate) fn exit_ready(&self) -> bool {
        self.retire();
        self.finished()
    }

    pub(crate) fn surface_retired(&self) -> bool {
        self.0.surface_retired.load(Ordering::Acquire)
    }

    pub(crate) fn has_started(&self) -> bool {
        self.0.started.load(Ordering::Acquire)
    }
}

fn run(inner: &Inner) {
    use windows::Win32::Foundation::HWND;
    println!(
        "desktop-render event=worker-started thread={:?} hwnd-owner=different-thread",
        thread::current().id()
    );
    let hwnd = HWND(inner.child as *mut std::ffi::c_void);
    let mut renderer = match CanvasSurfaceRenderer::new(hwnd, inner.live_ink.clone()) {
        Ok(renderer) => renderer,
        Err(error) => {
            inner.live_ink.publish_workspace_error(error);
            inner.live_ink.publish_close_status(CloseStatus::Failed {
                project_saved: false,
                project_path: String::new(),
            });
            return;
        }
    };
    let mut closing = false;
    loop {
        let mut pending = inner
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !pending.wake && !closing && !inner.live_ink.is_saving_as() {
            pending = inner
                .changed
                .wait(pending)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if !pending.wake {
            pending = inner
                .changed
                .wait_timeout(pending, Duration::from_millis(20))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        let work = pending.take_batch();
        drop(pending); // GPU, project and HWND operations never hold this lock.
        let _frame_batch = crate::performance::FrameBatch::begin(
            work.retire
                || work.controls.close
                || work.controls.reopen
                || work.controls.save_as
                || !work.activations.is_empty(),
        );
        renderer.resize_geometry(work.geometry);
        if work.controls.save_as
            && let Some(path) = inner.live_ink.take_save_as()
            && let Err(error) = renderer.save_as(path)
        {
            inner.live_ink.finish_save_as();
            inner.live_ink.publish_activation_notice(error);
        }
        for path in work.activations {
            if let Err(error) = renderer.activate_project(path) {
                inner.live_ink.publish_activation_notice(error);
            }
        }
        if work.retire {
            // Apply all semantic requests accepted before admission stopped;
            // suspend then joins any close/save-as/artwork workers on this
            // actor thread while the UI continues pumping DXGI messages.
            renderer.canvas.begin_close(
                renderer.config.width,
                renderer.config.height,
                renderer.scale,
            );
            renderer.canvas.suspend();
            break;
        }
        if work.controls.reopen {
            renderer
                .canvas
                .reopen_after_close_failure(&renderer.device, &renderer.queue);
            closing = inner.live_ink.is_closing();
            if !closing {
                println!("native-canvas event=close-reopened authority=durable-project");
            }
        }
        if work.controls.close {
            renderer.canvas.begin_close(
                renderer.config.width,
                renderer.config.height,
                renderer.scale,
            );
            closing = true;
        }
        if closing {
            renderer.canvas.poll_close();
        } else if let Err(error) = renderer.render() {
            inner.live_ink.publish_workspace_error(error);
        }
    }
    // Explicit retirement ordering is part of the raw surface safety contract.
    drop(renderer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_actor_batches_preserve_activation_order_and_retirement() {
        // Product risk: coalesced frame wakes must not discard accepted file
        // transitions or reopen a retired actor for a later document.
        let mut pending = Pending::default();
        for index in 0..ACTIVATION_CAPACITY {
            pending
                .activate(PathBuf::from(format!("{index}.ntdr")))
                .unwrap();
        }
        assert!(pending.activate("overflow.ntdr".into()).is_err());
        let batch = pending.take_batch();
        assert!(batch.wake);
        assert!(!pending.wake);
        for (index, path) in batch.activations.iter().enumerate() {
            assert_eq!(path, &PathBuf::from(format!("{index}.ntdr")));
        }
        // A wake arriving while the prior batch executes remains a new batch.
        pending.wake = true;
        pending.controls.close = true;
        pending.retire = true;
        let closing = pending.take_batch();
        assert!(closing.wake && closing.controls.close && closing.retire);
        assert!(pending.retire);
        assert!(pending.activate("late.ntdr".into()).is_err());
    }
}
