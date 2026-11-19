use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui_kit::{
    AnyElement, App, AppContext, Application, ApplicationHandle, Entity, Image, ImageFormat,
    ImageSource, IntoElement, Styled, Window, WindowOptions, canvas, img,
    web::{CanvasFontFallback, WebBackendPreference, WebPlatform},
};
use nyatidraw_api::{EditorCommand, FrameId, Revision, UiProjection};
use nyatidraw_editor_gpui::{EditorHost, EditorView, FileAction};
use nyatidraw_editor_ui::{
    preview::{FrameThumbnailSnapshot, LayerThumbnailSnapshot},
    workspace_appearance::WorkspaceAppearance,
};
use nyatidraw_input::{CanvasInputMode, PressurePreset};
use nyatidraw_web::host::{
    BrowserEditor, EditorModal, can_start_standalone, download_recovery_file,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen(module = "/src/platform.js")]
extern "C" {
    fn position_canvas(x: f32, y: f32, width: f32, height: f32);
    fn show_canvas(visible: bool);
    fn canvas_dock_drag(active: bool);
    fn panel_visible(key: &str) -> bool;
    fn display_status(
        status: &str,
        notice: Option<String>,
        modal: &str,
        ready: bool,
        busy: bool,
        hosted: bool,
    );
    fn display_recovery(description: &str, available: bool);
    fn display_picker(x: f64, y: f64, image: Option<String>, color: Option<String>, visible: bool);
    fn startup_error(error: &str);
}

thread_local! {
    static APPLICATION: RefCell<Option<ApplicationHandle>> = const { RefCell::new(None) };
    static EDITOR: RefCell<Option<WebSession>> = const { RefCell::new(None) };
    static REFRESH_PENDING: Cell<bool> = const { Cell::new(false) };
}

struct WebSession {
    host: Rc<WebHost>,
    view: Entity<EditorView>,
    window: gpui_kit::AnyWindowHandle,
}

struct WebHost {
    editor: Rc<BrowserEditor>,
    navigator: RefCell<Option<(Arc<str>, ImageSource)>>,
}

impl EditorHost for WebHost {
    fn navigator_quad(&self) -> Option<[[i32; 2]; 4]> {
        self.editor
            .backend()
            .navigator_snapshot()
            .viewport
            .map(|v| v.quad_page_10k)
    }
    fn brush_preview(&self, snapshot: &UiProjection) -> Result<Arc<str>, String> {
        nyatidraw_editor_ui::brush_preview::render(
            self.editor.backend().preview_preset(snapshot),
            snapshot.drawing_tool,
        )
    }
    fn input_mode(&self) -> CanvasInputMode {
        self.editor.backend().canvas_input_mode()
    }
    fn set_input_mode(&self, mode: CanvasInputMode) -> Result<(), String> {
        self.editor.backend().set_canvas_input_mode(mode)
    }
    fn pressure_preset(&self) -> PressurePreset {
        self.editor.backend().pressure_preset()
    }
    fn set_pressure_preset(&self, preset: PressurePreset) -> Result<(), String> {
        self.editor.backend().set_pressure_preset(preset)
    }
    fn appearance(&self) -> WorkspaceAppearance {
        self.editor.backend().workspace_appearance()
    }
    fn set_appearance(&self, appearance: WorkspaceAppearance) {
        self.editor.backend().set_workspace_appearance(appearance);
    }
    fn layer_thumbnails(&self) -> LayerThumbnailSnapshot {
        self.editor.backend().layer_thumbnail_snapshot()
    }
    fn frame_thumbnails(&self) -> FrameThumbnailSnapshot {
        self.editor.backend().frame_thumbnail_snapshot()
    }
    fn request_frame_thumbnails(&self, frames: &[FrameId]) {
        self.editor.backend().request_frame_thumbnails(frames);
    }
    fn can_file(&self, _: FileAction) -> bool {
        self.editor.ready() && !self.editor.busy() && self.editor.modal().is_none()
    }
    fn project_epoch(&self) -> u64 {
        self.editor.project_epoch()
    }
    fn snapshot(&self) -> UiProjection {
        self.editor.snapshot()
    }

    fn dispatch(&self, based_on: Revision, command: EditorCommand) -> Result<(), String> {
        self.editor.dispatch(based_on, command)
    }

    fn supports_command(&self, command: &EditorCommand) -> bool {
        self.editor.supports_command(command)
    }

    fn palette_pins(&self) -> [Option<[u8; 4]>; 10] {
        self.editor.palette_pins()
    }
    fn save_palette_pins(&self, pins: [Option<[u8; 4]>; 10]) -> Result<(), String> {
        self.editor.save_palette_pins(pins);
        Ok(())
    }

    fn navigator_preview(&self, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let frame = self.editor.navigator_png()?;
        let mut cache = self.navigator.borrow_mut();
        if !cache
            .as_ref()
            .is_some_and(|(cached, _)| Arc::ptr_eq(cached, &frame))
        {
            let bytes = STANDARD
                .decode(frame.strip_prefix("data:image/png;base64,")?)
                .ok()?;
            let source = ImageSource::Image(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
            if let Some((_, old)) = cache.replace((frame, source)) {
                old.remove_asset(cx);
            }
        }
        Some(
            img(cache.as_ref()?.1.clone())
                .size_full()
                .into_any_element(),
        )
    }

    fn canvas(&self, _: &mut Window, _: &mut App) -> AnyElement {
        canvas(
            |bounds, window, _| {
                let bounds = bounds.intersect(&window.content_mask().bounds);
                position_canvas(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                );
            },
            |_, (), _, _| {},
        )
        .size_full()
        .into_any_element()
    }

    fn panel_visible(&self, key: &str) -> bool {
        panel_visible(key)
    }
    fn document_canvas_visible(&self, visible: bool) {
        show_canvas(visible);
    }
    fn document_canvas_dock_drag(&self, active: bool) {
        canvas_dock_drag(active);
    }

    fn file_action(&self, action: FileAction) -> Result<(), String> {
        if !self.editor.ready() || self.editor.busy() || self.editor.modal().is_some() {
            return Err("작업을 준비하거나 확인을 기다리는 중입니다.".to_owned());
        }
        match action {
            FileAction::New => self.editor.show_new(),
            FileAction::Open => self.editor.open(),
            FileAction::Save | FileAction::SaveAs => self.editor.save(),
            FileAction::ExportPng => self.editor.export_png(),
            FileAction::ExportSizedPng(size) => self.editor.export_sized_png(size),
            FileAction::ExportSpriteSheet => self.editor.export_sprite_sheet(),
            FileAction::CopyPng => self.editor.copy_png(),
            FileAction::ImportLayer => {
                let editor = self.editor.clone();
                spawn_local(async move { editor.import_layer().await });
            }
        }
        Ok(())
    }
}

fn schedule_refresh() {
    if REFRESH_PENDING.with(|pending| pending.replace(true)) {
        return;
    }
    spawn_local(async {
        REFRESH_PENDING.with(|pending| pending.set(false));
        EDITOR.with_borrow(|editor| {
            let Some(WebSession { host, view, .. }) = editor else {
                return;
            };
            let editor = &host.editor;
            let modal = match editor.modal() {
                None => "",
                Some(EditorModal::NewDocument) => "new",
                Some(EditorModal::ReplaceDocument) => "replace",
                Some(EditorModal::CancelIntegration) => "cancel",
                Some(EditorModal::Help) => "help",
            };
            display_status(
                &editor.status(),
                editor.notice(),
                modal,
                editor.ready(),
                editor.busy(),
                editor.hosted(),
            );
            display_recovery(editor.recovery_description(), editor.recovery_available());
            if let Some(picker) = editor.picker() {
                let [x, y] = picker.cursor_css;
                let image = picker
                    .frame
                    .as_ref()
                    .map(|frame| frame.data_uri.to_string());
                let color = picker
                    .frame
                    .as_ref()
                    .and_then(|frame| frame.candidate)
                    .map(|[r, g, b, _]| format!("#{r:02x}{g:02x}{b:02x}"));
                display_picker(x, y, image, color, true);
            } else {
                display_picker(0., 0., None, None, false);
            }
            APPLICATION.with_borrow(|app| {
                if let Some(app) = app {
                    app.update(|cx| view.update(cx, |_, cx| cx.notify()));
                }
            });
        });
    });
}

#[wasm_bindgen]
pub fn canvas_key(key: String, modifiers: u8, repeat: bool) -> bool {
    use gpui_kit::{Focusable, KeyDownEvent, Keystroke, Modifiers, PlatformInput};
    EDITOR.with_borrow(|editor| {
        let Some(WebSession {
            host,
            view,
            window: handle,
        }) = editor
        else {
            return false;
        };
        if host.editor.busy() || host.editor.modal().is_some() {
            return false;
        }
        APPLICATION.with_borrow(|app| {
            app.as_ref().is_some_and(|app| {
                app.update(|cx| {
                    handle
                        .update(cx, |_, window, cx| {
                            window.focus(&view.read(cx).focus_handle(cx), cx);
                            let key = Keystroke {
                                key,
                                modifiers: Modifiers {
                                    control: modifiers & 1 != 0,
                                    shift: modifiers & 2 != 0,
                                    alt: modifiers & 4 != 0,
                                    platform: modifiers & 8 != 0,
                                    ..Default::default()
                                },
                                ..Default::default()
                            };
                            let result = window.dispatch_event(
                                PlatformInput::KeyDown(KeyDownEvent {
                                    keystroke: key,
                                    is_held: repeat,
                                    prefer_character_input: false,
                                }),
                                cx,
                            );
                            !result.propagate || result.default_prevented
                        })
                        .unwrap_or(false)
                })
            })
        })
    })
}

#[wasm_bindgen]
pub fn browser_action(action: &str, width: u32, height: u32) {
    EDITOR.with_borrow(|state| {
        let Some(WebSession { host, .. }) = state else {
            if action == "recovery" && can_start_standalone() {
                spawn_local(async {
                    if let Err(error) = download_recovery_file().await {
                        startup_error(&error);
                    }
                });
            }
            return;
        };
        let editor = &host.editor;
        if editor.busy() {
            return;
        }
        match (action, editor.modal()) {
            ("dismiss-notice", _) => editor.dismiss_notice(),
            ("recovery", None) if editor.recovery_available() => editor.download_recovery(),
            ("save", None) if editor.hosted() && editor.ready() => editor.save(),
            ("png", None) if editor.hosted() && editor.ready() => editor.export_png(),
            ("copy", None) if editor.hosted() && editor.ready() => editor.copy_png(),
            ("close", Some(_)) => editor.close_modal(),
            ("create", Some(EditorModal::NewDocument)) => editor.new_document(width, height),
            ("replace", Some(EditorModal::ReplaceDocument)) => editor.accept_replacement(),
            ("cancel", Some(EditorModal::CancelIntegration)) => editor.cancel(),
            ("request-cancel", None) if editor.hosted() && editor.ready() => editor.show_cancel(),
            ("complete", None) if editor.hosted() && editor.ready() => editor.complete(),
            _ => {}
        }
    });
}

#[wasm_bindgen]
pub fn run() -> Result<(), JsValue> {
    if APPLICATION.with_borrow(Option::is_some) {
        return Err("이미 열린 편집기입니다.".into());
    }
    display_recovery("", can_start_standalone());
    gpui_kit::platform::web_init();
    let platform = Rc::new(WebPlatform::new_with_backend_and_font_fallback(
        false,
        WebBackendPreference::Auto,
        CanvasFontFallback::EmojiAndCjk,
    ));
    let app = Application::with_platform(platform);
    let handle = app.run_embedded(|cx| {
        gpui_kit::init(cx);
        if let Err(error) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/IBMPlexSans-Regular.ttf").as_slice(),
        )]) {
            startup_error(&error.to_string());
            return;
        }
        let editor = Rc::new(BrowserEditor::new(Rc::new(schedule_refresh)));
        let host = Rc::new(WebHost {
            editor: editor.clone(),
            navigator: RefCell::new(None),
        });
        match gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| EditorView::new(host.clone(), window, cx))
        }) {
            Ok((window_handle, view)) => {
                EDITOR.with_borrow_mut(|state| {
                    *state = Some(WebSession {
                        host,
                        view,
                        window: window_handle,
                    });
                });
                schedule_refresh();
                spawn_local(async move { editor.initialize().await });
            }
            Err(error) => startup_error(&error.to_string()),
        }
    });
    APPLICATION.with_borrow_mut(|app| *app = Some(handle));
    Ok(())
}
