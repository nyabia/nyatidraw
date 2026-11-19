use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, Bounds, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div, px, size,
};
use nyatidraw_api::{
    EditCommand, EditorCommand, LayerId, ProjectCommand, Revision, TransformCommand, UiProjection,
};
use nyatidraw_editor_gpui::{EditorHost, EditorView, FileAction};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;

use crate::{
    desktop_canvas::DesktopCanvasHandle,
    live_ink::{CloseStatus, LiveInkBridge},
    palette_preferences::{PalettePreferences, Pins},
    single_instance::PrimaryInstance,
};

pub(crate) fn launch(instance: PrimaryInstance) -> Result<(), String> {
    let error = Rc::new(RefCell::new(None));
    let launch_error = error.clone();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let live_ink = LiveInkBridge::with_capacity(512, LayerId(1));
        crate::updates::initialize(live_ink.clone());
        live_ink.enable_layout_persistence();
        let display_id = pointer_display(cx);
        let options = WindowOptions {
            display_id,
            window_bounds: Some(WindowBounds::Maximized(Bounds::centered(
                display_id,
                size(px(1280.), px(800.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("NyatiDraw".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut startup_error = None;
        let opened = cx.open_window(options, |window, cx| {
            let content: gpui_kit::AnyView = match WindowsHost::new(window, cx, live_ink, instance)
            {
                Ok(host) => {
                    let host = Rc::new(host);
                    let (sender, receiver) = async_channel::bounded(1);
                    host.ink.set_ui_notifier(Arc::new(move || {
                        let _ = sender.try_send(());
                    }));
                    let editor = cx.new(|cx| EditorView::new(host.clone(), window, cx));
                    let weak_editor = editor.downgrade();
                    cx.spawn(async move |cx| {
                        while receiver.recv().await.is_ok() {
                            if weak_editor.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    host.canvas.start_render_worker();
                    println!("desktop-shell event=gpui-editor-ready");
                    editor.into()
                }
                Err(reason) => {
                    startup_error = Some(reason);
                    cx.new(|_| gpui_kit::Empty).into()
                }
            };
            cx.new(|cx| gpui_kit::base::Root::new(content, window, cx))
        });
        if let Some(reason) = startup_error.or_else(|| opened.err().map(|error| error.to_string()))
        {
            *launch_error.borrow_mut() = Some(reason);
            cx.quit();
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
    error.take().map_or(Ok(()), Err)
}

fn pointer_display(cx: &App) -> Option<gpui_kit::DisplayId> {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint},
        UI::WindowsAndMessaging::GetCursorPos,
    };
    let mut cursor = POINT::default();
    // SAFETY: cursor is writable storage and MonitorFromPoint accepts any screen point.
    unsafe {
        GetCursorPos(&raw mut cursor).ok()?;
    }
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }
    let id = gpui_kit::DisplayId::new(monitor.0 as u64);
    cx.displays()
        .iter()
        .any(|display| display.id() == id)
        .then_some(id)
}

struct WindowsHost {
    ink: LiveInkBridge,
    canvas: DesktopCanvasHandle,
    _raw_mouse: Option<nyatidraw_input_platform::WindowsRawMouseRegistration>,
    geometry: Rc<Cell<Option<[f64; 5]>>>,
    visible: Rc<Cell<bool>>,
    dialog: rfd::AsyncFileDialog,
    dialog_open: Rc<Cell<bool>>,
    app: AsyncApp,
    navigator: RefCell<NavigatorImages>,
    palette: PalettePreferences,
    pins: Cell<Pins>,
}

#[derive(Default)]
struct NavigatorImages {
    epoch: u64,
    pending: Option<(Arc<str>, Arc<gpui_kit::Image>)>,
    displayed: Option<(Arc<str>, Arc<gpui_kit::RenderImage>)>,
}

impl NavigatorImages {
    fn clear_pending(&mut self, cx: &mut App) {
        if let Some((_, source)) = self.pending.take() {
            source.remove_asset(cx);
        }
    }

    fn clear_displayed(&mut self, window: &mut Window, cx: &mut App) {
        if let Some((_, image)) = self.displayed.take() {
            cx.drop_image(image, Some(window));
        }
    }

    fn image(
        &mut self,
        uri: Arc<str>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<gpui_kit::RenderImage>> {
        use base64::Engine as _;
        if let Some((current, image)) = &self.displayed
            && current == &uri
        {
            let image = image.clone();
            self.clear_pending(cx);
            return Some(image);
        }
        if self
            .pending
            .as_ref()
            .is_none_or(|(current, _)| current != &uri)
        {
            self.clear_pending(cx);
            let encoded = uri.strip_prefix("data:image/png;base64,")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()?;
            self.pending = Some((
                uri.clone(),
                Arc::new(gpui_kit::Image::from_bytes(
                    gpui_kit::ImageFormat::Png,
                    bytes,
                )),
            ));
        }
        if let Some(image) = self
            .pending
            .as_ref()?
            .1
            .clone()
            .use_render_image(window, cx)
        {
            self.clear_displayed(window, cx);
            self.displayed = Some((uri, image));
            self.clear_pending(cx);
        }
        self.displayed.as_ref().map(|(_, image)| image.clone())
    }
}

impl WindowsHost {
    fn update_control(&self) -> Option<AnyElement> {
        use crate::updates::Status;
        let status = crate::updates::status();
        let label = match &status {
            Status::Development => return None,
            Status::Checking => "업데이트 확인 중".into(),
            Status::Downloading => "업데이트 받는 중".into(),
            Status::Current(version) => format!("업데이트 확인 · {version}"),
            Status::Ready(version) => format!("저장 후 업데이트 · {version}"),
            Status::Failed(error) => format!("업데이트 재시도 · {error}"),
        };
        let mut button = div().id("desktop-updates").child(label);
        if !matches!(status, Status::Checking | Status::Downloading) {
            let control = self.canvas.clone();
            button = button.cursor_pointer().on_click(move |_, _, _| {
                if matches!(crate::updates::status(), Status::Ready(_)) {
                    control.request_close();
                } else {
                    crate::updates::check();
                }
            });
        }
        Some(button.into_any_element())
    }
    fn new(
        window: &Window,
        cx: &App,
        ink: LiveInkBridge,
        instance: PrimaryInstance,
    ) -> Result<Self, String> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        else {
            return Err("GPUI did not supply a Win32 window".into());
        };
        let parent = HWND(handle.hwnd.get() as *mut _);
        let raw_mouse = nyatidraw_input_platform::WindowsRawMouseRegistration::install_if_absent(
            handle.hwnd.get().cast_unsigned(),
        )
        .unwrap_or_else(|reason| {
            eprintln!("desktop-input event=raw-mouse-registration-unavailable reason={reason}");
            None
        });
        let canvas = DesktopCanvasHandle::create(parent, None, &ink, instance, None)?;
        let notify_ink = ink.clone();
        let (palette, pins) = PalettePreferences::open(Arc::new(move || notify_ink.notify_ui()));
        Ok(Self {
            ink,
            canvas,
            _raw_mouse: raw_mouse,
            geometry: Rc::default(),
            visible: Rc::new(Cell::new(true)),
            dialog: rfd::AsyncFileDialog::new().set_parent(window),
            dialog_open: Rc::default(),
            app: cx.to_async(),
            navigator: RefCell::default(),
            palette,
            pins: Cell::new(pins),
        })
    }

    fn select_file(&self, action: FileAction) -> Result<(), String> {
        if self.dialog_open.replace(true) {
            return Err("파일 선택 창이 열려 있습니다.".into());
        }
        let busy = self.dialog_open.clone();
        let dialog = self.dialog.clone();
        let canvas = self.canvas.clone();
        let ink = self.ink.clone();
        let epoch = ink.project_epoch();
        self.app
            .spawn(async move |cx| {
                let file = select_path(dialog, action).await;
                let result = async {
                    let Some(file) = file else {
                        return Ok(());
                    };
                    let mut path = file.path().to_owned();
                    if epoch != ink.project_epoch() || ink.is_closing() {
                        return Err("그림이 바뀌어 파일 작업을 취소했습니다.".to_owned());
                    }
                    match action {
                        FileAction::Open => canvas.open_path(path),
                        FileAction::SaveAs => {
                            if path.extension().is_none() {
                                path.set_extension("ntdr");
                            }
                            if !path
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("ntdr"))
                                || path.exists()
                                || path.with_extension("png").exists()
                            {
                                return Err("새 .ntdr 파일 이름을 선택해주세요.".into());
                            }
                            canvas.save_as(path)
                        }
                        FileAction::ExportPng | FileAction::ExportSizedPng(_) => {
                            if path.extension().is_none() {
                                path.set_extension("png");
                            }
                            crate::scaled_export::validate_target(&path)?;
                            let dimensions = ink.protocol_snapshot().0.canvas;
                            let size = if let FileAction::ExportSizedPng(size) = action {
                                size
                            } else {
                                [dimensions.width_px, dimensions.height_px]
                            };
                            send(
                                &ink,
                                EditorCommand::Project(ProjectCommand::ExportScaledPng {
                                    path,
                                    size,
                                    project_epoch: epoch,
                                }),
                            )
                        }
                        FileAction::ExportSpriteSheet => {
                            let directory = path.with_extension("frames");
                            crate::sprite_export::validate_target(&directory)?;
                            send(
                                &ink,
                                EditorCommand::Project(ProjectCommand::ExportSpriteSheet {
                                    directory,
                                    columns: None,
                                    project_epoch: epoch,
                                }),
                            )
                        }
                        FileAction::ImportLayer => {
                            let import = cx
                                .background_executor()
                                .spawn(
                                    async move { crate::image_import::decode_import(&path, epoch) },
                                )
                                .await?;
                            if epoch != ink.project_epoch() || ink.is_closing() {
                                return Err("그림이 바뀌어 이미지 가져오기를 취소했습니다.".into());
                            }
                            send(
                                &ink,
                                EditorCommand::Edit(EditCommand::FreeTransform(
                                    TransformCommand::ImportRaster(import),
                                )),
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                .await;
                busy.set(false);
                if let Err(reason) = result {
                    ink.publish_activation_notice(reason);
                }
            })
            .detach();
        Ok(())
    }
}

async fn select_path(dialog: rfd::AsyncFileDialog, action: FileAction) -> Option<rfd::FileHandle> {
    match action {
        FileAction::Open => {
            dialog
                .set_title("그림 열기")
                .add_filter("그림 / 프로젝트", &["png", "ntdr"])
                .pick_file()
                .await
        }
        FileAction::SaveAs => {
            dialog
                .set_title("다른 이름으로 저장")
                .add_filter("NyatiDraw 프로젝트", &["ntdr"])
                .set_file_name("새 그림.ntdr")
                .save_file()
                .await
        }
        FileAction::ExportPng | FileAction::ExportSizedPng(_) => {
            dialog
                .set_title("PNG 내보내기")
                .add_filter("PNG 이미지", &["png"])
                .set_file_name("그림.png")
                .save_file()
                .await
        }
        FileAction::ExportSpriteSheet => {
            dialog
                .set_title("스프라이트 시트 내보내기")
                .set_file_name("그림.frames")
                .save_file()
                .await
        }
        FileAction::ImportLayer => {
            dialog
                .set_title("PNG를 레이어로 가져오기")
                .add_filter("PNG 이미지", &["png"])
                .pick_file()
                .await
        }
        _ => unreachable!("only dialog actions are scheduled"),
    }
}

fn send(ink: &LiveInkBridge, command: EditorCommand) -> Result<(), String> {
    ink.push_ui_editor_command(command)
        .map(|_| ())
        .map_err(|_| "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요.".into())
}

impl EditorHost for WindowsHost {
    fn brush_preview(&self, projection: &UiProjection) -> Result<Arc<str>, String> {
        nyatidraw_editor_ui::brush_preview::render(
            crate::native_canvas::preset_for_ui_preview(projection),
            projection.drawing_tool,
        )
    }
    fn recent_projects(&self) -> Vec<String> {
        crate::recent_projects::list()
            .unwrap_or_default()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    fn open_recent(&self, label: &str) -> Result<(), String> {
        let mut matches = crate::recent_projects::list()?
            .into_iter()
            .filter(|path| path.to_string_lossy() == label);
        let path = matches.next().ok_or("최근 파일 목록이 바뀌었습니다.")?;
        if matches.next().is_some() {
            return Err("이름이 같은 경로가 있습니다. 파일 열기를 사용해주세요.".into());
        }
        self.canvas.open_path(path)
    }

    fn input_mode(&self) -> nyatidraw_input::CanvasInputMode {
        self.ink.canvas_input_mode()
    }
    fn set_input_mode(&self, mode: nyatidraw_input::CanvasInputMode) -> Result<(), String> {
        self.ink.set_canvas_input_mode(mode)
    }
    fn pressure_preset(&self) -> nyatidraw_input::PressurePreset {
        self.ink.pressure_preset()
    }
    fn set_pressure_preset(&self, preset: nyatidraw_input::PressurePreset) -> Result<(), String> {
        self.ink.set_pressure_preset(preset)
    }
    fn appearance(&self) -> nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance {
        let current = self.ink.workspace_appearance();
        nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance {
            checkerboard: current.checkerboard,
            solid_rgb: current.solid_rgb,
        }
    }
    fn set_appearance(
        &self,
        appearance: nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance,
    ) {
        let mut native = self.ink.workspace_appearance();
        native.checkerboard = appearance.checkerboard;
        native.solid_rgb = appearance.solid_rgb;
        self.ink.set_workspace_appearance(native);
    }
    fn snapshot(&self) -> UiProjection {
        self.ink.protocol_snapshot().0
    }
    fn project_epoch(&self) -> u64 {
        self.ink.project_epoch()
    }
    fn layer_thumbnails(&self) -> nyatidraw_editor_ui::preview::LayerThumbnailSnapshot {
        self.ink.layer_thumbnail_snapshot()
    }

    fn frame_thumbnails(&self) -> nyatidraw_editor_ui::preview::FrameThumbnailSnapshot {
        self.ink.frame_thumbnail_snapshot()
    }

    fn request_frame_thumbnails(&self, frames: &[nyatidraw_api::FrameId]) {
        self.ink.request_frame_thumbnails(frames);
    }

    fn dispatch(&self, based_on: Revision, command: EditorCommand) -> Result<(), String> {
        self.ink
            .push_editor_command(based_on, command)
            .map(|_| ())
            .map_err(|_| "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요.".to_owned())
    }

    fn file_status(&self) -> Option<(String, bool)> {
        use crate::live_ink::ExportStatus;
        let (label, retry) = match self.ink.export_status_snapshot() {
            ExportStatus::Idle => ("", false),
            ExportStatus::Waiting { .. } => ("PNG 저장 대기", false),
            ExportStatus::Queued { .. } => ("PNG 대기", false),
            ExportStatus::Running { .. } => ("PNG 저장 중", false),
            ExportStatus::Current { .. } => ("PNG 저장됨", false),
            ExportStatus::Failed { .. } => ("PNG 실패", true),
        };
        Some((
            if self.ink.is_saving_as() {
                format!("다른 이름으로 저장 중… {label}")
            } else {
                label.to_owned()
            },
            retry,
        ))
    }
    fn begin_tracked_command(
        &self,
        revision: Revision,
        command: EditorCommand,
    ) -> Result<Option<u128>, String> {
        let id = self
            .ink
            .push_tracked_command(revision, command)
            .map_err(|_| "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요.".to_owned())?;
        self.canvas.wake_for_command();
        Ok(Some(id.0))
    }
    fn take_command_result(&self, id: u128) -> Option<Result<(), String>> {
        self.ink.take_command_receipt(id)
    }

    fn supports_command(&self, command: &EditorCommand) -> bool {
        use nyatidraw_api::HistoryCommand;
        let EditorCommand::History(command @ (HistoryCommand::Undo | HistoryCommand::Redo)) =
            command
        else {
            return true;
        };
        let history = self.ink.protocol_snapshot().0.history;
        if *command == HistoryCommand::Undo {
            history.entries.len() > 1 || history.has_older_entries
        } else {
            !history.redo_branches.is_empty() || history.redo_page_after.is_some()
        }
    }

    fn palette_pins(&self) -> Pins {
        self.pins.get()
    }

    fn save_palette_pins(&self, pins: Pins) -> Result<(), String> {
        self.palette.save(pins)?;
        self.pins.set(pins);
        Ok(())
    }

    fn canvas(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let control = self.canvas.clone();
        let geometry = self.geometry.clone();
        let visible = self.visible.clone();
        let status = self.ink.close_status();
        let mut area = div().size_full().flex().flex_col();
        if status != CloseStatus::Open {
            if geometry.take().is_some() {
                control.hide();
            }
            return match status {
                CloseStatus::Failed {
                    project_saved,
                    project_path,
                } => {
                    let reopen = control.clone();
                    area.child(if project_saved {
                        "프로젝트는 저장됐지만 PNG 내보내기에 실패했습니다."
                    } else {
                        "일부 변경이 저장되지 않았을 수 있습니다."
                    })
                    .child(project_path)
                    .child(
                        div()
                            .id("reopen")
                            .child("프로젝트 다시 열기")
                            .on_click(move |_, _, _| reopen.reopen_after_close_failure()),
                    )
                    .child(
                        div()
                            .id("confirm-close")
                            .child("오류를 확인하고 종료")
                            .on_click(move |_, _, _| control.confirm_failed_close()),
                    )
                }
                _ => area.child("저장 후 종료합니다…"),
            }
            .into_any_element();
        }
        if let Some(button) = self.update_control() {
            area = area.child(button);
        }
        area.children(
            self.ink
                .activation_notice_snapshot()
                .or_else(|| self.palette.notice())
                .or_else(|| self.ink.layout_notice())
                .map(|message| div().child(message)),
        )
        .child(
            canvas(
                move |bounds, window, _| {
                    let rect = [
                        f64::from(f32::from(bounds.origin.x)),
                        f64::from(f32::from(bounds.origin.y)),
                        f64::from(f32::from(bounds.size.width)),
                        f64::from(f32::from(bounds.size.height)),
                        f64::from(window.scale_factor()),
                    ];
                    if visible.get() && geometry.replace(Some(rect)) != Some(rect) {
                        control.set_geometry(rect[0], rect[1], rect[2], rect[3], rect[4]);
                    }
                },
                |_, (), _, _| {},
            )
            .w_full()
            .flex_1(),
        )
        .into_any_element()
    }

    fn document_canvas_visible(&self, visible: bool) {
        if self.visible.replace(visible) && !visible {
            self.geometry.set(None);
            self.canvas.hide();
        }
    }

    fn document_canvas_dock_drag(&self, active: bool) {
        self.canvas.set_dock_drag(active);
    }

    fn navigator_quad(&self) -> Option<[[i32; 2]; 4]> {
        self.ink
            .navigator_snapshot()
            .viewport
            .map(|viewport| viewport.quad_page_10k)
    }

    fn navigator_preview(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let mut cache = self.navigator.borrow_mut();
        let epoch = self.ink.project_epoch();
        let frame = self.ink.navigator_snapshot().frame;
        if cache.epoch != epoch || frame.is_none() {
            cache.clear_pending(cx);
            cache.clear_displayed(window, cx);
            cache.epoch = epoch;
        }
        let image = cache.image(frame?.data_uri.clone(), window, cx)?;
        Some(
            gpui_kit::img(image)
                .size_full()
                .object_fit(gpui_kit::ObjectFit::Contain)
                .into_any_element(),
        )
    }

    fn file_action(&self, action: FileAction) -> Result<(), String> {
        match action {
            FileAction::New => self.canvas.new_drawing(),
            FileAction::Save => send(&self.ink, EditorCommand::Project(ProjectCommand::Save)),
            FileAction::CopyPng => send(&self.ink, EditorCommand::Project(ProjectCommand::CopyPng)),
            other => self.select_file(other),
        }
    }
}
