use std::{cell::RefCell, io::Cursor, rc::Rc};

use dioxus::prelude::*;
use js_sys::{Array, Uint8Array};
use nyatidraw_api::DrawingTool;
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_project_web::{RecoveryCheckpoint, WebProject};
use nyatidraw_web_core::{CanvasSpec, CanvasUpdate, StrokePoint, WebDocument, WebError, WebTool};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::HtmlCanvasElement;

use crate::{browser, gpu::WebRenderer};

const RECOVERY_QUIET_MS: f64 = 600.0;
const RECOVERY_MAX_WAIT_MS: f64 = 3000.0;
const PREVIEW_DELAY_MS: f64 = 200.0;

#[allow(clippy::struct_excessive_bools)]
pub struct Runtime {
    pub document: WebProject,
    pub renderer: WebRenderer,
    pub canvas: HtmlCanvasElement,
    pub viewport: ViewportTransform,
    pub fit_to_canvas: bool,
    pub recent_colors: Vec<[u8; 4]>,
    pub recent_sizes: Vec<u16>,
    pub selected_tool: DrawingTool,
    pub project_epoch: u64,
    pub preview_generation: u64,
    preview_dirty: bool,
    preview_running: bool,
    frame_pending: bool,
    pub gpu_failed: bool,
    pan_start: Option<(Point, Point)>,
    save_generation: u64,
    save_running: bool,
    save_pending: bool,
    save_not_before: f64,
    save_due_by: f64,
    recovery_checkpoint: Option<RecoveryCheckpoint>,
    download_pending: bool,
    worker_failed: bool,
    pending_document: Option<WebProject>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EditorModal {
    Help,
    NewDocument,
    ReplaceDocument,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Editor {
    pub runtime: Signal<Rc<RefCell<Option<Runtime>>>>,
    pub revision: Signal<u64>,
    pub status: Signal<String>,
    pub ready: Signal<bool>,
    pub modal: Signal<Option<EditorModal>>,
    pub notice: Signal<Option<String>>,
}

impl Editor {
    pub fn read<T>(self, read: impl FnOnce(&Runtime) -> T) -> Option<T> {
        let cell = self.runtime.peek().clone();
        cell.borrow().as_ref().map(read)
    }

    pub fn refresh(mut self) {
        self.revision += 1;
    }

    pub fn report(mut self, text: impl Into<String>) {
        self.status.set(text.into());
    }

    pub fn warn(mut self, text: impl Into<String>) {
        self.notice.set(Some(text.into()));
    }

    pub fn dismiss_notice(mut self) {
        self.notice.set(None);
    }

    pub fn show_modal(mut self, modal: EditorModal) {
        browser::set_modal_open(true);
        self.modal.set(Some(modal));
    }

    pub fn close_modal(mut self) {
        if let Some(runtime) = self.runtime.peek().borrow_mut().as_mut() {
            runtime.pending_document = None;
        }
        self.modal.set(None);
        browser::set_modal_open(false);
    }

    pub fn confirm_replacement(self) {
        let document = self
            .runtime
            .peek()
            .borrow_mut()
            .as_mut()
            .and_then(|runtime| runtime.pending_document.take());
        self.close_modal();
        if let Some(document) = document {
            self.replace(document);
        }
    }

    pub async fn initialize(mut self) -> Result<(), String> {
        if !JsFuture::from(browser::claim_workspace())
            .await
            .map_err(|e| browser::error_text(&e))?
            .as_bool()
            .unwrap_or(false)
        {
            return Err("다른 탭에 NyatiDraw가 열려 있습니다. 그림을 보호하기 위해 그 탭을 닫은 후 새로고침해주세요.".into());
        }
        let saved = JsFuture::from(browser::load_workspace())
            .await
            .map_err(|e| browser::error_text(&e))?;
        let restored = !saved.is_null();
        let document = if restored {
            WebProject::decode_ntdr(&Uint8Array::new(&saved).to_vec()).map_err(|e| {
                format!("저장된 작업을 열지 못했습니다. 원본 복구 데이터는 유지됩니다: {e}")
            })?
        } else {
            WebProject::from_document(WebDocument::default())
        };
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("drawing-canvas"))
            .ok_or("그리기 영역을 찾지 못했습니다")?
            .dyn_into::<HtmlCanvasElement>()
            .map_err(|_| "캔버스 초기화 실패")?;
        let renderer = WebRenderer::new(canvas.clone(), &document).await?;
        let document_tool = document.tool();
        let import_notice = document.import_notice();
        let selected_tool = supported_restored_tool(&document)
            .unwrap_or_else(|| crate::adapter::drawing_tool(document_tool));
        let mut runtime = Runtime {
            document,
            renderer,
            canvas: canvas.clone(),
            viewport: ViewportTransform {
                revision: 1,
                window_origin_physical: Point::default(),
                physical_size: [1, 1],
                dpi_scale: 1.0,
                pan: Point::default(),
                zoom: 1.0,
                rotation_radians: 0.0,
                mirrored_horizontal: false,
            },
            recent_colors: Vec::new(),
            fit_to_canvas: true,
            recent_sizes: Vec::new(),
            selected_tool,
            project_epoch: 1,
            preview_generation: 1,
            preview_dirty: false,
            preview_running: false,
            frame_pending: false,
            gpu_failed: false,
            pan_start: None,
            save_generation: 0,
            save_running: false,
            save_pending: false,
            save_not_before: 0.0,
            save_due_by: 0.0,
            recovery_checkpoint: None,
            download_pending: false,
            worker_failed: false,
            pending_document: None,
        };
        runtime.resize();
        runtime.fit();
        *self.runtime.peek().borrow_mut() = Some(runtime);
        let callback = Closure::<dyn FnMut(String, f64, f64, f64, f64)>::new(
            move |phase: String, x, y, pressure, time| self.input(&phase, x, y, pressure, time),
        );
        browser::bind_canvas(&canvas, callback.as_ref().unchecked_ref());
        browser::set_canvas_tool(canvas_input_mode(selected_tool));
        callback.forget();
        self.ready.set(true);
        self.report(if restored {
            "작업 복구 완료 · 이 브라우저에 저장됨"
        } else {
            "WebGPU · 그림은 이 기기에만 저장됩니다"
        });
        self.refresh();
        self.redraw();
        if let Some(notice) = import_notice {
            self.warn(notice);
        }
        Ok(())
    }

    pub fn edit(self, operation: impl FnOnce(&mut WebDocument) -> Result<CanvasUpdate, WebError>) {
        if let Err(error) = self.try_edit(operation) {
            self.warn(error);
        }
    }

    pub fn try_edit(
        self,
        operation: impl FnOnce(&mut WebDocument) -> Result<CanvasUpdate, WebError>,
    ) -> Result<(), String> {
        if self.modal.peek().is_some() {
            return Err("열려 있는 대화상자를 먼저 닫아주세요.".into());
        }
        let cell = self.runtime.peek().clone();
        let result = {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return Err("그림을 여는 중입니다.".into());
            };
            if runtime.document.is_drawing() {
                return Err("먼저 현재 획을 마쳐주세요.".into());
            }
            if runtime.gpu_failed {
                return Err("화면이 중단되었습니다. 작업 저장 후 새로고침해주세요.".into());
            }
            operation(&mut runtime.document)
                .map_err(|e| e.to_string())
                .and_then(|update| {
                    let result = runtime.apply(&update);
                    if result.is_err() {
                        runtime.gpu_failed = true;
                    }
                    result
                })
        };
        match result {
            Ok(()) => {
                self.refresh();
                self.redraw();
                self.persist();
                Ok(())
            }
            Err(error) => {
                self.refresh();
                self.persist();
                Err(error)
            }
        }
    }

    pub fn try_preferences(
        self,
        operation: impl FnOnce(&mut WebDocument) -> Result<(), WebError>,
    ) -> Result<(), String> {
        self.try_edit(|document| {
            operation(document)?;
            Ok(CanvasUpdate::default())
        })
    }

    pub fn fit(self) {
        if self.modal.peek().is_some() {
            return;
        }
        if let Some(runtime) = self.runtime.peek().borrow_mut().as_mut() {
            if runtime.document.is_drawing() {
                return;
            }
            runtime.fit();
        }
        self.refresh();
        self.redraw();
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines
    )]
    pub fn input(self, phase: &str, x: f64, y: f64, pressure: f64, time_ms: f64) {
        if phase == "dismissModal" {
            self.close_modal();
            return;
        }
        if self.modal.peek().is_some() && !matches!(phase, "resize" | "cancel" | "overflow") {
            return;
        }
        match phase {
            "undo" => {
                self.edit(WebDocument::undo);
                return;
            }
            "redo" => {
                self.edit(WebDocument::redo);
                return;
            }
            "save" => {
                self.download_project();
                return;
            }
            _ => {}
        }
        let cell = self.runtime.peek().clone();
        let mut save = false;
        let mut refresh = false;
        let result = (|| {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return Ok(());
            };
            let point = Point { x, y };
            match phase {
                "resize" => {
                    save = runtime.document.is_drawing() || runtime.save_pending;
                    let update = runtime.document.cancel_stroke();
                    runtime.apply(&update)?;
                    runtime.resize();
                    if runtime.fit_to_canvas {
                        runtime.fit();
                    }
                    refresh = true;
                    return Ok(());
                }
                "panBegin" => {
                    runtime.fit_to_canvas = false;
                    runtime.pan_start = Some((point, runtime.viewport.pan));
                    return Ok(());
                }
                "panMove" => {
                    if let Some((start, pan)) = runtime.pan_start {
                        runtime.viewport.pan = Point {
                            x: pan.x + x - start.x,
                            y: pan.y + y - start.y,
                        };
                        runtime.viewport.revision += 1;
                        refresh = true;
                    }
                    return Ok(());
                }
                "panEnd" => {
                    runtime.pan_start = None;
                    refresh = true;
                    return Ok(());
                }
                "zoom" => {
                    runtime.zoom_at(point, (-pressure * 0.002).exp());
                    refresh = true;
                    return Ok(());
                }
                "cancel" | "overflow" => {
                    runtime.pan_start = None;
                    let update = runtime.document.cancel_stroke();
                    refresh = true;
                    save = true;
                    runtime.apply(&update)?;
                    if phase == "overflow" {
                        return Err(
                            "입력량이 한도를 넘어 이번 획을 취소했습니다. 다시 그려주세요.".into(),
                        );
                    }
                    return Ok(());
                }
                _ => {}
            }
            if runtime.gpu_failed {
                return Err("화면 표시가 중단되었습니다. 작업 저장 후 새로고침해주세요.".into());
            }
            let position = runtime
                .viewport
                .logical_to_document(point)
                .ok_or("입력 좌표 오류")?;
            let sample = StrokePoint {
                x: position.x,
                y: position.y,
                pressure: pressure as f32,
                time_ms,
            };
            if phase == "pickEnd" {
                let color = runtime
                    .document
                    .sample_rgba8(position.x.floor() as i32, position.y.floor() as i32)
                    .map_err(|error| error.to_string())?;
                if color[3] == 0 {
                    return Ok(());
                }
                runtime.document.set_foreground(color);
                save = true;
                refresh = true;
                return Ok(());
            }
            let update = match phase {
                "begin" => runtime.document.begin_stroke(sample),
                "move" if runtime.document.is_drawing() => runtime.document.move_stroke(sample),
                "end" if runtime.document.is_drawing() => runtime.document.end_stroke(sample),
                _ => return Ok(()),
            };
            let update = match update {
                Ok(update) => update,
                Err(error) => {
                    runtime.document.cancel_stroke();
                    save = true;
                    refresh = true;
                    if let Err(gpu_error) = runtime.renderer.sync_document(&runtime.document) {
                        runtime.gpu_failed = true;
                        return Err(gpu_error);
                    }
                    return Err(error.to_string());
                }
            };
            if phase == "begin" {
                let size = (runtime.document.brush_settings().size_px * 10.0).round() as u16;
                runtime.recent_sizes.retain(|previous| *previous != size);
                runtime.recent_sizes.insert(0, size);
                runtime.recent_sizes.truncate(4);
                let color = runtime.document.foreground();
                if runtime.document.tool() != WebTool::Eraser {
                    runtime.recent_colors.retain(|previous| *previous != color);
                    runtime.recent_colors.insert(0, color);
                    runtime.recent_colors.truncate(10);
                }
                browser::set_unsaved(true);
            }
            save = update.committed || phase == "end";
            refresh = phase == "begin" || phase == "end";
            if let Err(error) = runtime.apply(&update) {
                runtime.document.cancel_stroke();
                runtime.gpu_failed = true;
                save = true;
                return Err(error);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.warn(error);
        }
        if refresh {
            self.refresh();
        }
        self.redraw();
        if save {
            self.persist();
        }
    }

    pub fn redraw(self) {
        self.queue_previews();
        let cell = self.runtime.peek().clone();
        {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            if runtime.frame_pending {
                return;
            }
            runtime.frame_pending = true;
        }
        let callback = Closure::once_into_js(move || {
            let result = {
                let mut borrow = cell.borrow_mut();
                let Some(runtime) = borrow.as_mut() else {
                    return;
                };
                runtime.frame_pending = false;
                let result = runtime.renderer.render(runtime.viewport);
                if result.is_err() {
                    runtime.gpu_failed = true;
                    runtime.document.cancel_stroke();
                }
                result
            };
            if let Err(error) = result {
                self.refresh();
                self.persist();
                self.warn(error);
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window.request_animation_frame(callback.unchecked_ref());
        }
    }

    fn queue_previews(self) {
        let cell = self.runtime.peek().clone();
        {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            if !runtime.preview_dirty || runtime.preview_running || runtime.document.is_drawing() {
                return;
            }
            runtime.preview_running = true;
        }
        spawn_local(async move {
            let _ = JsFuture::from(browser::background_turn(PREVIEW_DELAY_MS)).await;
            let publish = {
                let mut borrow = cell.borrow_mut();
                let Some(runtime) = borrow.as_mut() else {
                    return;
                };
                runtime.preview_running = false;
                if runtime.document.is_drawing() || !runtime.preview_dirty {
                    false
                } else {
                    runtime.preview_dirty = false;
                    runtime.preview_generation += 1;
                    true
                }
            };
            if publish {
                self.refresh();
            }
        });
    }

    #[allow(clippy::too_many_lines)]
    pub fn persist(self) {
        let cell = self.runtime.peek().clone();
        {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            let now = browser::monotonic_now();
            if !runtime.save_pending {
                runtime.save_due_by = now + RECOVERY_MAX_WAIT_MS;
            }
            runtime.save_not_before = if runtime.download_pending {
                now
            } else {
                (now + RECOVERY_QUIET_MS).min(runtime.save_due_by)
            };
            runtime.save_generation += 1;
            runtime.save_pending = true;
            browser::set_unsaved(true);
            if runtime.save_running {
                return;
            }
            runtime.save_running = true;
        }
        self.report("변경사항 저장 대기 중…");
        spawn_local(async move {
            loop {
                let delay = cell.borrow().as_ref().map_or(0.0, |runtime| {
                    (runtime.save_not_before - browser::monotonic_now()).max(0.0)
                });
                let _ = JsFuture::from(browser::background_turn(delay)).await;
                let prepared = {
                    let mut borrow = cell.borrow_mut();
                    let Some(runtime) = borrow.as_mut() else {
                        return;
                    };
                    if runtime.document.is_drawing() {
                        runtime.save_running = false;
                        return;
                    }
                    if browser::monotonic_now() < runtime.save_not_before {
                        continue;
                    }
                    runtime.save_pending = false;
                    let started = browser::monotonic_now();
                    let download = std::mem::take(&mut runtime.download_pending);
                    let result = runtime
                        .document
                        .prepare_recovery(
                            runtime.project_epoch,
                            runtime.save_generation,
                            runtime.recovery_checkpoint.as_ref(),
                        )
                        .map(|request| (runtime.save_generation, request, download));
                    browser::record_work("recovery_capture", started);
                    result
                };
                self.report("브라우저에 저장 중…");
                let (generation, request, download) = match prepared {
                    Ok(value) => value,
                    Err(error) => {
                        if let Some(runtime) = cell.borrow_mut().as_mut() {
                            runtime.save_running = false;
                            runtime.worker_failed = true;
                        }
                        self.report("브라우저 저장 실패");
                        self.warn(format!(
                            "자동 복구 저장 실패 · 작업 저장으로 내려받으세요: {error}"
                        ));
                        return;
                    }
                };
                let result = JsFuture::from(browser::save_in_worker(
                    &Uint8Array::from(request.bytes.as_slice()),
                    download,
                ))
                .await;
                let mut borrow = cell.borrow_mut();
                let Some(runtime) = borrow.as_mut() else {
                    return;
                };
                if let Err(error) = result {
                    runtime.save_running = false;
                    runtime.worker_failed = true;
                    drop(borrow);
                    self.report("브라우저 저장 실패");
                    self.warn(format!(
                        "브라우저 저장 실패 · 작업 파일을 내려받으세요: {}",
                        browser::error_text(&error)
                    ));
                    return;
                }
                runtime.recovery_checkpoint = Some(request.checkpoint);
                runtime.worker_failed = false;
                let pending = runtime.save_pending;
                runtime.save_running = pending;
                let current = !pending
                    && generation == runtime.save_generation
                    && !runtime.document.is_drawing();
                drop(borrow);
                if download {
                    let bytes = Uint8Array::new(&result.unwrap_or(JsValue::NULL));
                    browser::download_bytes(&bytes, "drawing.ntdr", "application/octet-stream");
                }
                if pending {
                    continue;
                }
                if current {
                    browser::set_unsaved(false);
                    self.report(if self.read(|r| r.gpu_failed).unwrap_or(false) {
                        "화면 표시 중단 · 복구 저장 완료 · 작업 파일을 내려받은 후 새로고침해주세요"
                    } else {
                        "이 브라우저에 저장됨 · 중요한 그림은 작업 파일로 보관하세요"
                    });
                }
                return;
            }
        });
    }

    pub fn download_project(self) {
        if !self.read(|runtime| runtime.worker_failed).unwrap_or(true) {
            if let Some(runtime) = self.runtime.peek().borrow_mut().as_mut() {
                runtime.download_pending = true;
            }
            self.persist();
            return;
        }
        let result = {
            let cell = self.runtime.peek().clone();
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            runtime.document.encode_ntdr()
        };
        match result {
            Ok(bytes) => browser::download_bytes(
                &Uint8Array::from(bytes.as_slice()),
                "drawing.ntdr",
                "application/octet-stream",
            ),
            Err(error) => self.warn(error),
        }
    }

    pub fn download_recovery(self) {
        spawn_local(async move {
            match JsFuture::from(browser::load_workspace()).await {
                Ok(saved) if saved.is_null() => self.warn("이 브라우저에 저장된 작업이 없습니다."),
                Ok(saved) => {
                    let bytes = Uint8Array::new(&saved);
                    let legacy = bytes.subarray(0, 8).to_vec() == b"NYWEB001";
                    browser::download_bytes(
                        &bytes,
                        if legacy {
                            "recovered.nyatidraw-web"
                        } else {
                            "recovered.ntdr"
                        },
                        "application/octet-stream",
                    );
                }
                Err(error) => self.warn(format!(
                    "복구 데이터를 읽지 못했습니다. 기존 데이터는 유지됩니다: {}",
                    browser::error_text(&error)
                )),
            }
        });
    }

    pub fn download_png(self) {
        let Some(result) = self.read(|runtime| encode_png(&runtime.document)) else {
            return;
        };
        match result {
            Ok(bytes) => browser::download_bytes(
                &Uint8Array::from(bytes.as_slice()),
                "drawing.png",
                "image/png",
            ),
            Err(error) => self.warn(error),
        }
    }

    pub fn new_document(self, width: u32, height: u32) {
        match WebProject::new(CanvasSpec {
            width_px: width,
            height_px: height,
            pixels_per_inch: 96,
        }) {
            Ok(document) => self.stage_replacement(document),
            Err(error) => self.warn(error),
        }
    }

    pub fn open(self) {
        if self.modal.peek().is_some() {
            return;
        }
        spawn_local(async move {
            let file = match JsFuture::from(browser::pick_file()).await {
                Ok(file) if file.is_null() => return,
                Ok(file) => Array::from(&file),
                Err(error) => {
                    self.warn(browser::error_text(&error));
                    return;
                }
            };
            let name = file.get(0).as_string().unwrap_or_default();
            let bytes = Uint8Array::new(&file.get(1)).to_vec();
            let document = if name.to_lowercase().ends_with(".png") {
                decode_png(&bytes).map(WebProject::from_document)
            } else {
                WebProject::decode_ntdr(&bytes)
            };
            match document {
                Ok(document) => self.stage_replacement(document),
                Err(error) => {
                    self.warn(format!("열지 못했습니다. 현재 그림은 유지됩니다: {error}"));
                }
            }
        });
    }

    fn stage_replacement(self, document: WebProject) {
        {
            let cell = self.runtime.peek().clone();
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            if runtime.pending_document.is_some() {
                self.warn("먼저 열려 있는 작업 교체 확인 창을 닫아주세요.");
                return;
            }
            runtime.pending_document = Some(document);
        }
        self.show_modal(EditorModal::ReplaceDocument);
    }

    fn replace(self, document: WebProject) {
        let import_notice = document.import_notice();
        let cell = self.runtime.peek().clone();
        let result = {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            if runtime.document.is_drawing() {
                return;
            }
            match runtime.renderer.sync_document(&document) {
                Ok(()) => {
                    runtime.selected_tool = supported_restored_tool(&document)
                        .unwrap_or_else(|| crate::adapter::drawing_tool(document.tool()));
                    runtime.document = document;
                    runtime.project_epoch += 1;
                    runtime.preview_generation += 1;
                    browser::set_canvas_tool(canvas_input_mode(runtime.selected_tool));
                    runtime.gpu_failed = false;
                    runtime.fit();
                    Ok(())
                }
                Err(error) => {
                    if let Err(restore_error) = runtime.renderer.sync_document(&runtime.document) {
                        runtime.gpu_failed = true;
                        return self.warn(format!(
                            "새 그림을 표시하지 못했습니다: {error}. 현재 그림 데이터는 유지되지만 화면 복구도 실패했습니다: {restore_error}. 작업 저장 후 새로고침해주세요."
                        ));
                    }
                    Err(error)
                }
            }
        };
        match result {
            Ok(()) => {
                self.refresh();
                self.redraw();
                self.persist();
                if let Some(notice) = import_notice {
                    self.warn(notice);
                }
            }
            Err(error) => self.warn(error),
        }
    }
}

impl Runtime {
    pub fn apply(&mut self, update: &CanvasUpdate) -> Result<(), String> {
        if update.committed && update.changed {
            self.preview_dirty = true;
        }
        if update.structure_changed {
            self.renderer.sync_document(&self.document)
        } else {
            self.renderer
                .upload_dirty(&self.document, &update.dirty_tiles)
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn resize(&mut self) {
        let rect = self.canvas.get_bounding_client_rect();
        let dpi = web_sys::window()
            .map_or(1.0, |w| w.device_pixel_ratio())
            .clamp(1.0, 2.0);
        self.viewport.physical_size = [
            (rect.width() * dpi).round().max(1.0) as u32,
            (rect.height() * dpi).round().max(1.0) as u32,
        ];
        self.viewport.dpi_scale = dpi;
        self.viewport.revision += 1;
    }

    fn fit(&mut self) {
        self.fit_to_canvas = true;
        let rect = self.canvas.get_bounding_client_rect();
        let page = self.document.canvas();
        let zoom = ((rect.width() - 64.0) / f64::from(page.width_px))
            .min((rect.height() - 64.0) / f64::from(page.height_px))
            .clamp(0.05, 8.0);
        self.viewport.zoom = zoom;
        self.viewport.rotation_radians = 0.0;
        self.viewport.mirrored_horizontal = false;
        self.viewport.pan = Point {
            x: (rect.width() - f64::from(page.width_px) * zoom) / 2.0,
            y: (rect.height() - f64::from(page.height_px) * zoom) / 2.0,
        };
        self.viewport.revision += 1;
    }

    pub fn zoom_at(&mut self, focus: Point, factor: f64) {
        self.fit_to_canvas = false;
        if let Some(view) = self.viewport.with_view_at(
            focus,
            (self.viewport.zoom * factor).clamp(0.1, 16.0),
            self.viewport.rotation_radians,
            self.viewport.mirrored_horizontal,
        ) {
            self.viewport = view;
            self.viewport.revision += 1;
        }
    }
}

fn supported_restored_tool(document: &WebProject) -> Option<DrawingTool> {
    document.restored_tool().filter(|tool| {
        matches!(
            tool,
            DrawingTool::Move
                | DrawingTool::Eyedropper
                | DrawingTool::Pencil
                | DrawingTool::Pen
                | DrawingTool::Brush
                | DrawingTool::Eraser
        )
    })
}

pub const fn canvas_input_mode(tool: DrawingTool) -> &'static str {
    match tool {
        DrawingTool::Move => "pan",
        DrawingTool::Eyedropper => "pick",
        _ => "stroke",
    }
}

fn encode_png(document: &WebDocument) -> Result<Vec<u8>, String> {
    if document.is_drawing() {
        return Err("획을 마친 후 내보내주세요.".into());
    }
    let pixels = document.export_rgba8().map_err(|e| e.to_string())?;
    let canvas = document.canvas();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, canvas.width_px, canvas.height_px);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

fn decode_png(bytes: &[u8]) -> Result<WebDocument, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 72 * 1024 * 1024,
    });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    if info.width > 4096 || info.height > 4096 {
        return Err("웹판 PNG는 최대 4096 × 4096입니다.".into());
    }
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
    buffer.truncate(info.buffer_size());
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba = buffer,
        png::ColorType::Rgb => {
            for pixel in buffer.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for value in buffer {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in buffer.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => return Err("PNG 색상 변환에 실패했습니다.".into()),
    }
    WebDocument::import_rgba8(info.width, info.height, &rgba).map_err(|e| e.to_string())
}
