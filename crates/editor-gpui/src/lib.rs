mod canvas_dialog;
mod canvas_workspace;
mod color_wheel;
mod controls;
mod dock;
mod panels;
mod preferences;
use controls::{action_control, control};

use gpui_kit::{
    component::{button::Button, input::InputState},
    prelude::*,
    *,
};
use nyatidraw_api::*;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
    ExportPng,
    CopyPng,
    ImportLayer,
    ExportSizedPng([u32; 2]),
    ExportSpriteSheet,
}

pub trait EditorHost {
    fn brush_preview(&self, _snapshot: &UiProjection) -> Result<std::sync::Arc<str>, String> {
        Err("획 미리보기를 사용할 수 없습니다.".into())
    }
    fn layer_thumbnails(&self) -> nyatidraw_editor_ui::preview::LayerThumbnailSnapshot {
        Default::default()
    }
    fn frame_thumbnails(&self) -> nyatidraw_editor_ui::preview::FrameThumbnailSnapshot {
        Default::default()
    }
    fn request_frame_thumbnails(&self, _frames: &[FrameId]) {}
    fn recent_projects(&self) -> Vec<String> {
        Vec::new()
    }
    fn open_recent(&self, _path: &str) -> Result<(), String> {
        Err("최근 파일을 열 수 없습니다.".into())
    }
    fn can_file(&self, _action: FileAction) -> bool {
        true
    }
    fn file_status(&self) -> Option<(String, bool)> {
        None
    }
    fn input_mode(&self) -> nyatidraw_input::CanvasInputMode;
    fn set_input_mode(&self, mode: nyatidraw_input::CanvasInputMode) -> Result<(), String>;
    fn pressure_preset(&self) -> nyatidraw_input::PressurePreset;
    fn set_pressure_preset(&self, preset: nyatidraw_input::PressurePreset) -> Result<(), String>;
    fn appearance(&self) -> nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance;
    fn set_appearance(
        &self,
        appearance: nyatidraw_editor_ui::workspace_appearance::WorkspaceAppearance,
    );
    fn project_epoch(&self) -> u64 {
        0
    }
    fn snapshot(&self) -> UiProjection;
    fn dispatch(&self, based_on: Revision, command: EditorCommand) -> Result<(), String>;
    fn begin_tracked_command(
        &self,
        revision: Revision,
        command: EditorCommand,
    ) -> Result<Option<u128>, String> {
        self.dispatch(revision, command).map(|()| None)
    }
    fn take_command_result(&self, _id: u128) -> Option<Result<(), String>> {
        None
    }
    fn supports_command(&self, _command: &EditorCommand) -> bool {
        true
    }
    fn panel_visible(&self, _key: &str) -> bool {
        true
    }
    fn palette_pins(&self) -> [Option<[u8; 4]>; 10] {
        [None; 10]
    }
    fn save_palette_pins(&self, _pins: [Option<[u8; 4]>; 10]) -> Result<(), String> {
        Err("팔레트 고정을 지원하지 않습니다.".into())
    }
    fn document_canvas_visible(&self, _visible: bool) {}
    fn document_canvas_dock_drag(&self, _active: bool) {}
    fn canvas(&self, window: &mut Window, cx: &mut App) -> AnyElement;
    fn navigator_preview(&self, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        None
    }
    fn navigator_quad(&self) -> Option<[[i32; 2]; 4]> {
        None
    }
    fn file_action(&self, action: FileAction) -> Result<(), String>;
}

pub struct EditorView {
    pub(crate) host: Rc<dyn EditorHost>,
    projection: Rc<UiProjection>,
    pub(crate) panels: panels::PanelState,
    focus: FocusHandle,
    wheel: color_wheel::Wheel,
    rotation: Option<controls::RotationField>,
    epoch: u64,
    resizing: Option<(usize, u16)>,
    pending_dock_resize: Option<dock::PendingResize>,
    dock_hint: Option<(PanelKind, DockPosition)>,
    compact_dock: bool,
    compact_panels: dock::CompactPanels,
    canvas_drawn: bool,
    dismissed: Option<String>,
    menu: bool,
    settings: Option<preferences::BackgroundPicker>,
    timeline: bool,
    notice: Option<String>,
    page: Option<canvas_dialog::PageDraft>,
}

impl EditorView {
    pub fn new(host: Rc<dyn EditorHost>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, Some(window), cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            projection: Rc::new(UiProjection::empty()),
            wheel: color_wheel::Wheel::new(cx),
            rotation: None,
            resizing: None,
            pending_dock_resize: None,
            dock_hint: None,
            compact_dock: false,
            compact_panels: dock::CompactPanels::default(),
            canvas_drawn: false,
            epoch: host.project_epoch(),
            dismissed: None,
            host,
            panels: Default::default(),
            focus,
            settings: None,
            menu: false,
            timeline: true,
            notice: None,
            page: None,
        }
    }

    fn send(&mut self, command: EditorCommand, cx: &mut Context<Self>) {
        self.send_at(self.host.snapshot().revision, command, cx);
    }

    fn send_at(&mut self, based_on: Revision, command: EditorCommand, cx: &mut Context<Self>) {
        let commits_preview = matches!(
            command,
            EditorCommand::Edit(
                EditCommand::FreeTransform(TransformCommand::Commit { .. })
                    | EditCommand::ColorAdjustment(ColorAdjustmentCommand::Commit { .. })
            )
        );
        let switches_tool = matches!(
            command,
            EditorCommand::Tool(
                ToolCommand::Select(_)
                    | ToolCommand::SelectPencilTemplate(_)
                    | ToolCommand::CycleBrushFamily
                    | ToolCommand::CycleSelectionFamily
                    | ToolCommand::CycleFillFamily
                    | ToolCommand::ApplyBrushPreset(_)
            )
        );
        if (commits_preview || switches_tool) && self.panels.has_unapplied_preview() {
            self.notice = Some("입력한 값을 먼저 미리보기에서 확인하세요.".into());
            cx.notify();
            return;
        }
        if self.host.supports_command(&command) {
            self.notice = self.host.dispatch(based_on, command).err();
        }
        cx.notify();
    }

    fn file(&mut self, action: FileAction, cx: &mut Context<Self>) {
        self.menu = false;
        if !self.host.can_file(action) {
            self.notice = Some("현재 이 작업을 실행할 수 없습니다.".into());
            cx.notify();
            return;
        }
        self.notice = self.host.file_action(action).err();
        cx.notify();
    }

    fn command_button(
        &self,
        label: &'static str,
        command: EditorCommand,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Button {
        let enabled = enabled && self.host.supports_command(&command);
        let base = self.projection.revision;
        action_control(label, enabled).when(enabled, |el| {
            el.on_click(cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus, cx);
                view.send_at(base, command.clone(), cx);
            }))
        })
    }

    fn toolbar(
        &mut self,
        kind: PanelKind,
        top_row: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snap = self.projection.clone();
        let mut bar = div()
            .flex()
            .items_center()
            .gap_1()
            .when(top_row, |d| d.flex_shrink_0().whitespace_nowrap())
            .when(!top_row, |d| d.flex_wrap());
        match kind {
            PanelKind::CanvasActions => {
                bar = bar
                    .child(self.command_button(
                        "비우기",
                        EditorCommand::Edit(EditCommand::ClearActiveLayer),
                        snap.active_layer.is_some(),
                        cx,
                    ))
                    .child(self.command_button(
                        "흰 배경",
                        EditorCommand::Layer(LayerCommand::AddWhiteBackground),
                        true,
                        cx,
                    ))
                    .child(self.command_button(
                        "변형",
                        EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::Begin)),
                        nyatidraw_editor_ui::can_begin_transform(&snap),
                        cx,
                    ))
                    .child(
                        action_control("캔버스", true).on_click(
                            cx.listener(|view, _, window, cx| view.open_page(window, cx)),
                        ),
                    )
                    .child(self.command_button(
                        "취소",
                        EditorCommand::Tool(ToolCommand::CancelGesture),
                        snap.edit.can_cancel || snap.edit.has_selection,
                        cx,
                    ));
            }
            PanelKind::Viewport => {
                for (label, command) in [
                    ("맞춤", ViewportCommand::FitDocument),
                    ("1:1", ViewportCommand::ActualPixels),
                    ("−", ViewportCommand::ZoomSteps(-1)),
                    ("+", ViewportCommand::ZoomSteps(1)),
                ] {
                    bar = bar.child(self.command_button(
                        label,
                        EditorCommand::Viewport(command),
                        true,
                        cx,
                    ));
                }
                let angle = snap.viewport.rotation_millidegrees;
                let enabled = self.host.supports_command(&EditorCommand::Viewport(
                    ViewportCommand::SetRotationMillidegrees(0),
                ));
                bar = bar
                    .child(format!("{}%", snap.viewport.zoom_ppm / 10_000))
                    .child(
                        self.rotation
                            .get_or_insert_with(|| controls::RotationField::new(angle, window, cx))
                            .render(angle, enabled, window, cx),
                    )
                    .child(self.command_button(
                        "0°",
                        EditorCommand::Viewport(ViewportCommand::ResetRotation),
                        true,
                        cx,
                    ))
                    .child(self.command_button(
                        "좌우 반전",
                        EditorCommand::Viewport(ViewportCommand::ToggleMirrorHorizontal),
                        true,
                        cx,
                    ));
            }
            _ => return panels::panel(kind, self, window, cx),
        }
        bar.into_any_element()
    }

    fn actions(&mut self, top: &[PanelKind], window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut bar = div()
            .id("actions")
            .flex()
            .items_center()
            .min_h(px(36.))
            .flex_shrink_0()
            .whitespace_nowrap()
            .gap_1()
            .bg(rgb(0x505050))
            .on_drop(cx.listener(|view, drag: &dock::PanelDrag, _, cx| {
                if drag.0.is_toolbar() {
                    view.send(
                        EditorCommand::Dock(DockCommand::MoveToolbarToTop {
                            panel: drag.0,
                            before: None,
                        }),
                        cx,
                    );
                }
            }))
            .child(
                action_control("메뉴", true).on_click(cx.listener(|view, _, window, cx| {
                    view.menu = !view.menu;
                    window.focus(&view.focus, cx);
                    cx.notify()
                })),
            );
        let files: &[(&str, FileAction)] = if self.menu {
            &[
                ("새 그림", FileAction::New),
                ("열기", FileAction::Open),
                ("다른 이름으로 저장", FileAction::SaveAs),
                ("PNG 내보내기", FileAction::ExportPng),
                ("PNG 복사", FileAction::CopyPng),
                ("레이어로 가져오기", FileAction::ImportLayer),
            ]
        } else {
            &[
                ("새 그림", FileAction::New),
                ("열기", FileAction::Open),
                ("저장", FileAction::Save),
            ]
        };
        for &(label, action) in files {
            let enabled = self.host.can_file(action);
            let button = if self.menu {
                control(label, enabled)
            } else {
                action_control(label, enabled)
            };
            bar = bar.child(button.when(enabled, |b| {
                b.on_click(cx.listener(move |v, _, _, cx| v.file(action, cx)))
            }));
        }
        if self.menu {
            bar = bar
                .child(
                    control("PNG 크기 지정", true).on_click(cx.listener(|v, _, w, cx| {
                        v.menu = false;
                        v.open_page(w, cx);
                        if let Some(draft) = v.page.as_mut() {
                            draft.export = true;
                        }
                    })),
                )
                .child(
                    control(
                        "스프라이트 시트",
                        self.host.can_file(FileAction::ExportSpriteSheet),
                    )
                    .when(
                        self.host.can_file(FileAction::ExportSpriteSheet),
                        |b| {
                            b.on_click(
                                cx.listener(|v, _, _, cx| {
                                    v.file(FileAction::ExportSpriteSheet, cx)
                                }),
                            )
                        },
                    ),
                )
                .child(
                    control("설정", true).on_click(cx.listener(|v, _, window, cx| {
                        v.menu = false;
                        v.settings = Some(preferences::BackgroundPicker::new(
                            v.host.appearance().solid_rgb,
                            window,
                            cx,
                        ));
                        cx.notify();
                    })),
                );
            for (idx, path) in self.host.recent_projects().into_iter().take(5).enumerate() {
                let label = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&path)
                    .to_owned();
                bar = bar.child(
                    div()
                        .id(("recent", idx))
                        .px_1()
                        .cursor_pointer()
                        .child(label)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.menu = false;
                            v.notice = v.host.open_recent(&path).err();
                            cx.notify();
                        })),
                );
            }
            bar = bar
                .child(
                    control("애니메이션", true).on_click(cx.listener(|v, _, _, cx| {
                        v.menu = false;
                        v.timeline = true;
                        if v.host.snapshot().animation.frames.is_empty() {
                            v.send(EditorCommand::Animation(AnimationCommand::Enable), cx);
                        }
                        cx.notify();
                    })),
                )
                .child(self.command_button(
                    "도킹 초기화",
                    EditorCommand::Dock(DockCommand::ResetToSafeDefault),
                    true,
                    cx,
                ));
        } else {
            let s = self.projection.clone();
            bar = bar
                .child(self.command_button(
                    "실행 취소",
                    EditorCommand::History(HistoryCommand::Undo),
                    s.history.entries.len() > 1 || s.history.has_older_entries,
                    cx,
                ))
                .child(self.command_button(
                    "다시 실행",
                    EditorCommand::History(HistoryCommand::Redo),
                    true,
                    cx,
                ));
            for &kind in top {
                if !self.visible(kind) {
                    continue;
                }
                bar = bar
                    .child(dock::handle(kind, "⠿", cx))
                    .child(self.toolbar(kind, true, window, cx));
            }
            if !s.animation.frames.is_empty() {
                bar = bar.child(
                    control("타임라인", true).on_click(cx.listener(|v, _, _, cx| {
                        v.toggle_timeline(cx);
                    })),
                );
            }
        }
        if let Some((status, retry)) = self.host.file_status() {
            let mut caption = self.projection.document_title.to_string();
            if self.projection.dirty {
                caption.push_str(" *");
            }
            if !status.is_empty() {
                caption.push_str(" · ");
                caption.push_str(&status);
            }
            bar = bar.child(div().px_1().text_sm().child(caption));
            if retry {
                bar = bar.child(
                    control("PNG 재시도", true)
                        .on_click(cx.listener(|view, _, _, cx| view.file(FileAction::Save, cx))),
                );
            }
        }
        div()
            .flex()
            .items_center()
            .min_w_0()
            .flex_shrink_0()
            .bg(rgb(0x505050))
            .child(
                div()
                    .id("actions-scroll")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .child(bar),
            )
            .when(self.compact_dock, |d| {
                d.child(self.compact_dock_controls(cx))
            })
    }

    fn toggle_timeline(&mut self, cx: &mut Context<Self>) {
        self.timeline = !self.timeline;
        if !self.timeline && self.host.snapshot().animation.playing {
            self.send(EditorCommand::Animation(AnimationCommand::Stop), cx);
        }
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        if self.menu {
            if key == "escape" {
                self.menu = false;
                window.focus(&self.focus, cx);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        if key == "escape" && self.panels.cancel_reference_contact() {
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.settings.is_some() {
            if key == "escape" {
                self.settings = None;
                window.focus(&self.focus, cx);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.page.is_some() {
            if key == "enter" && !e.is_held {
                self.commit_page(false, window, cx);
                cx.stop_propagation();
                return;
            }
            if key == "escape" && !self.page.as_ref().is_some_and(|page| page.is_pending()) {
                self.page = None;
                window.focus(&self.focus, cx);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        if !(self.focus.is_focused(window)
            || self.wheel.has_keyboard_focus(window)
            || self.panels.frames_have_focus(window))
            || e.keystroke.modifiers.platform
        {
            return;
        }
        if e.keystroke.modifiers.alt {
            if key == "o" && !e.is_held {
                self.file(FileAction::Open, cx);
                cx.stop_propagation();
            }
            return;
        }
        let ctrl = e.keystroke.modifiers.control;
        let shift = e.keystroke.modifiers.shift;
        let command = match (ctrl, key) {
            (false, "enter") if !e.is_held => {
                let Some(transform) = self.host.snapshot().edit.transform.filter(|t| t.can_commit)
                else {
                    return;
                };
                EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::Commit {
                    generation: transform.generation,
                }))
            }
            (false, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "f6") => {
                let kind = match key {
                    "1" | "f6" => PanelKind::Canvas,
                    "2" => PanelKind::Tools,
                    "3" => PanelKind::Brush,
                    "4" => PanelKind::Navigator,
                    "5" => PanelKind::Color,
                    "6" => PanelKind::Layers,
                    _ => PanelKind::History,
                };
                EditorCommand::Dock(DockCommand::ActivatePanel(kind))
            }
            (true, "o") => {
                self.file(FileAction::Open, cx);
                cx.stop_propagation();
                return;
            }
            (true, "n") => {
                self.file(FileAction::New, cx);
                cx.stop_propagation();
                return;
            }
            (true, "a") => EditorCommand::Edit(EditCommand::SelectAll),
            (true, "d") => EditorCommand::Edit(EditCommand::ClearSelection),
            (true, "i") if shift => EditorCommand::Edit(EditCommand::InvertSelection),
            (true, "c") => EditorCommand::Edit(EditCommand::CopySelection),
            (true, "x") => EditorCommand::Edit(EditCommand::CutSelection),
            (true, "v") => EditorCommand::Edit(EditCommand::PasteSelection),
            (false, "b") => EditorCommand::Tool(ToolCommand::CycleBrushFamily),
            (false, "g") => EditorCommand::Tool(ToolCommand::CycleSelectionFamily),
            (false, "f") => EditorCommand::Tool(ToolCommand::CycleFillFamily),
            (false, "c") => EditorCommand::Tool(ToolCommand::Select(DrawingTool::Eyedropper)),
            (false, "e") => EditorCommand::Tool(ToolCommand::Select(DrawingTool::Eraser)),
            (false, "x") => EditorCommand::Tool(ToolCommand::SwapColors),
            (false, "[" | "]") => EditorCommand::Tool(ToolCommand::AdjustSizeSteps {
                tool: self.host.snapshot().drawing_tool,
                steps: if key == "[" { -1 } else { 1 },
            }),
            (_, "z") => EditorCommand::History(if shift {
                HistoryCommand::Redo
            } else {
                HistoryCommand::Undo
            }),
            (_, "s") => {
                self.file(
                    if shift {
                        FileAction::SaveAs
                    } else {
                        FileAction::Save
                    },
                    cx,
                );
                cx.stop_propagation();
                return;
            }
            (false, "escape") if !e.is_held => EditorCommand::Tool(ToolCommand::CancelGesture),
            (false, "delete") => EditorCommand::Edit(EditCommand::DeleteSelectedPixels),
            _ => return,
        };
        self.send(command, cx);
        cx.stop_propagation();
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.resizing = None;
            self.dock_hint = None;
        }
        let epoch = self.host.project_epoch();
        if self.epoch != epoch {
            self.epoch = epoch;
            self.panels.reset_document(window, cx);
            self.wheel.cancel();
            self.rotation = None;
            self.page = None;
            self.menu = false;
            self.settings = None;
            self.notice = None;
            self.dismissed = None;
            self.pending_dock_resize = None;
        }
        self.poll_dock_resize();
        self.poll_page_result(window, cx);
        self.projection = Rc::new(self.host.snapshot());
        let state = self.projection.clone();
        let width = f32::from(window.viewport_size().width);
        self.compact_dock = width < self.dock_min_width(state.dock.root(), false);
        self.canvas_drawn = false;
        if self.dismissed != state.edit.error {
            self.dismissed = None;
        }
        let mut root = div()
            .id("editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_drop(
                cx.listener(|view, drag: &dock::SplitDrag, _, cx| view.finish_resize(drag, cx)),
            )
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x232323))
            .text_color(rgb(0xf1f1f1))
            .text_size(px(12.))
            .when(self.host.panel_visible("toolbar"), |d| {
                d.child(self.actions(state.dock.top(), window, cx))
            })
            .when(
                self.compact_dock && !self.host.panel_visible("toolbar"),
                |d| d.child(self.compact_dock_controls(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .pt(px(4.))
                    .child(self.dock_node(state.dock.root(), 1, width, window, cx)),
            );
        if self.timeline && !state.animation.frames.is_empty() {
            root = root.child(panels::animation(self, window, cx));
        }
        self.host.document_canvas_visible(
            self.canvas_drawn && self.page.is_none() && self.settings.is_none(),
        );
        self.host.document_canvas_dock_drag(cx.has_active_drag());
        if self.page.is_some() {
            root = root.child(self.page_dialog(window, cx));
        }
        if let Some(picker) = &self.settings {
            root = root.child(self.preferences(picker, cx));
        }
        if let Some(message) = self.notice.clone().or_else(|| {
            state
                .edit
                .error
                .clone()
                .filter(|error| Some(error) != self.dismissed.as_ref())
        }) {
            root = root.child(
                div()
                    .absolute()
                    .right_2()
                    .bottom_2()
                    .p_2()
                    .bg(rgb(0x502d2d))
                    .child(message)
                    .child(control("닫기", true).on_click(cx.listener(|v, _, _, cx| {
                        v.notice = None;
                        v.dismissed = v.host.snapshot().edit.error;
                        cx.notify();
                    }))),
            );
        }
        root
    }
}
