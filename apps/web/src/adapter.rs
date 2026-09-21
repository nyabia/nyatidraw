use dioxus::prelude::ReadableExt;
use std::{cell::RefCell, rc::Rc, sync::Arc};

use nyatidraw_api::{
    BrushSettings, DockCommand, DockTree, DrawingTool, EditCommand, EditorCommand, EditorEvent,
    EventEnvelope, HistoryCommand, HistoryEntryProjection, HistoryOperationLabel, LayerCommand,
    LayerProjection, LayerProjectionKind, LayerTreeNodeId, PencilTemplate, Revision, ToolCommand,
    UiProjection, ViewportCommand, ViewportProjection, WorkspaceProjection,
};
use nyatidraw_editor_ui::{
    UiBackend,
    layout_store::PanelHeights,
    preview::{LayerThumbnailSnapshot, NavigatorSnapshot, NavigatorViewport},
    workspace_appearance::WorkspaceAppearance,
};
use nyatidraw_input::Point;
use nyatidraw_web_core::{LayerTreeNode, WebDocument, WebTool};

use crate::{browser, runtime::Editor};

#[derive(Default)]
struct Preferences {
    dock: DockTree,
    heights: PanelHeights,
    appearance: WorkspaceAppearance,
    pins: [Option<[u8; 4]>; 10],
    pencil: PencilTemplate,
    project_epoch: u64,
}

impl Preferences {
    fn load(editor: Editor) -> Self {
        let mut preferences = Self::default();
        for key in ["pins", "heights", "appearance"] {
            match browser::load_preference(key) {
                Ok(Some(value)) if preferences.restore(key, &value).is_none() => editor.warn(
                    "화면 설정 일부를 읽지 못해 기본값을 사용합니다. 그림 데이터는 유지됩니다.",
                ),
                Err(error) => editor.warn(format!(
                    "화면 설정을 읽지 못했습니다: {}",
                    browser::error_text(&error)
                )),
                _ => {}
            }
        }
        preferences
    }

    fn restore(&mut self, key: &str, value: &str) -> Option<()> {
        match key {
            "pins" => {
                let entries: Vec<_> = value.split(':').collect();
                if entries.len() != 10 {
                    return None;
                }
                let mut pins = [None; 10];
                for (index, entry) in entries.into_iter().enumerate() {
                    if entry == "-" {
                        continue;
                    }
                    if entry.len() != 8 {
                        return None;
                    }
                    pins[index] = Some(u32::from_str_radix(entry, 16).ok()?.to_be_bytes());
                }
                self.pins = pins;
            }
            "heights" => {
                let entries: Vec<_> = value
                    .split(',')
                    .map(str::parse::<u16>)
                    .collect::<Result<_, _>>()
                    .ok()?;
                if entries.len() != 12
                    || entries.iter().any(|v| *v != 0 && !(64..=4096).contains(v))
                {
                    return None;
                }
                self.heights.values.copy_from_slice(&entries);
            }
            "appearance" => {
                let (check, hex) = value.split_once(':')?;
                if !["0", "1"].contains(&check) || hex.len() != 6 {
                    return None;
                }
                let [_, red, green, blue] = u32::from_str_radix(hex, 16).ok()?.to_be_bytes();
                self.appearance.checkerboard = check == "1";
                self.appearance.solid_rgb = [red, green, blue];
            }
            _ => return None,
        }
        Some(())
    }
}

pub struct WebUiBackend {
    pub editor: Editor,
    preferences: RefCell<Preferences>,
    previews: RefCell<(u64, NavigatorSnapshot, LayerThumbnailSnapshot)>,
}

impl WebUiBackend {
    pub fn new(editor: Editor) -> Rc<Self> {
        Rc::new(Self {
            editor,
            preferences: RefCell::new(Preferences::load(editor)),
            previews: RefCell::new((
                u64::MAX,
                NavigatorSnapshot::default(),
                LayerThumbnailSnapshot::default(),
            )),
        })
    }

    fn save_preference(&self, key: &str, value: &str) {
        if let Err(error) = browser::store_preference(key, value) {
            self.editor.warn(format!(
                "화면 설정을 저장하지 못했습니다: {}",
                browser::error_text(&error)
            ));
        }
    }

    fn restore_project_tool(&self) {
        let Some((epoch, tool)) = self
            .editor
            .read(|runtime| (runtime.project_epoch, runtime.document.tool()))
        else {
            return;
        };
        let mut preferences = self.preferences.borrow_mut();
        if preferences.project_epoch == epoch {
            return;
        }
        preferences.project_epoch = epoch;
        preferences.pencil = if tool == WebTool::Pencil2B {
            PencilTemplate::Graphite2B
        } else {
            PencilTemplate::Mechanical2H
        };
    }

    fn update_previews(&self) {
        let revision = self
            .editor
            .read(|runtime| runtime.preview_generation)
            .unwrap_or(0);
        if self.previews.borrow().0 == revision {
            return;
        }
        let started = browser::monotonic_now();
        let Some((navigator, layers)) = self.editor.read(|runtime| {
            let document = &runtime.document;
            let frame = crate::preview::navigator(document, revision).map(Arc::new);
            let rect = runtime.canvas.get_bounding_client_rect();
            let page = document.canvas();
            let corners = [
                [0.0, 0.0],
                [rect.width(), 0.0],
                [rect.width(), rect.height()],
                [0.0, rect.height()],
            ];
            let viewport = NavigatorViewport {
                quad_page_10k: corners.map(|[x, y]| {
                    let point = runtime
                        .viewport
                        .logical_to_document(Point { x, y })
                        .unwrap_or_default();
                    [
                        bounded_i32(point.x / f64::from(page.width_px) * 10_000.0),
                        bounded_i32(point.y / f64::from(page.height_px) * 10_000.0),
                    ]
                }),
            };
            let frames = document
                .layers()
                .root()
                .children
                .iter()
                .filter_map(|node| {
                    let LayerTreeNode::Raster(layer) = node else {
                        return None;
                    };
                    crate::preview::layer_thumbnail(document, layer.id)
                        .map(|frame| (layer.id, Arc::new(frame)))
                })
                .collect();
            (
                NavigatorSnapshot {
                    frame,
                    viewport: Some(viewport),
                },
                LayerThumbnailSnapshot {
                    generation: revision,
                    frames,
                },
            )
        }) else {
            return;
        };
        *self.previews.borrow_mut() = (revision, navigator, layers);
        browser::record_work("previews", started);
    }

    fn select_tool(&self, tool: DrawingTool) -> Result<(), String> {
        let web_tool = match tool {
            DrawingTool::Pencil => Some(
                if self.preferences.borrow().pencil == PencilTemplate::Graphite2B {
                    WebTool::Pencil2B
                } else {
                    WebTool::Pencil2H
                },
            ),
            DrawingTool::Pen => Some(WebTool::Pen),
            DrawingTool::Brush => Some(WebTool::SoftBrush),
            DrawingTool::Eraser => Some(WebTool::Eraser),
            _ => None,
        };
        if let Some(tool) = web_tool {
            self.editor
                .try_preferences(|document| document.select_tool(tool))?;
        }
        if let Some(runtime) = self.editor.runtime.peek().borrow_mut().as_mut() {
            runtime.selected_tool = tool;
            runtime.document.set_project_tool(tool);
        }
        browser::set_canvas_tool(crate::runtime::canvas_input_mode(tool));
        self.editor.refresh();
        self.editor.persist();
        Ok(())
    }

    fn brush(
        &self,
        change: impl FnOnce(&mut nyatidraw_web_core::BrushSettings),
    ) -> Result<(), String> {
        self.editor.try_preferences(|document| {
            let mut settings = document.brush_settings();
            change(&mut settings);
            document.set_brush_settings(settings)
        })
    }

    fn tool_command(&self, command: ToolCommand) -> Result<(), String> {
        match command {
            ToolCommand::Select(tool) => self.select_tool(tool),
            ToolCommand::SelectPencilTemplate(template) => {
                self.preferences.borrow_mut().pencil = template;
                self.select_tool(DrawingTool::Pencil)
            }
            ToolCommand::CycleBrushFamily => {
                let tool = self
                    .editor
                    .read(|runtime| runtime.selected_tool)
                    .unwrap_or(DrawingTool::Pencil);
                self.select_tool(match tool {
                    DrawingTool::Pencil => DrawingTool::Pen,
                    DrawingTool::Pen => DrawingTool::Brush,
                    _ => DrawingTool::Pencil,
                })
            }
            ToolCommand::CycleSelectionFamily => self.select_tool(DrawingTool::Move),
            ToolCommand::CancelGesture => {
                self.editor.input("cancel", 0.0, 0.0, 0.0, 0.0);
                Ok(())
            }
            ToolCommand::SetSizeTenths(value) => {
                self.brush(|s| s.size_px = f32::from(value) / 10.0)
            }
            ToolCommand::AdjustSizeSteps { steps, .. } => {
                self.brush(|s| s.size_px = (s.size_px + f32::from(steps)).clamp(0.1, 200.0))
            }
            ToolCommand::SetOpacityU16(value) => {
                if value == 0 {
                    return Err("브러시 불투명도는 0보다 커야 합니다.".into());
                }
                self.brush(|s| s.opacity = f32::from(value) / 65535.0)
            }
            ToolCommand::SetHardnessU16(value) => {
                self.brush(|s| s.hardness = f32::from(value) / 65535.0)
            }
            ToolCommand::SetSizePressure(value) => self.brush(|s| s.size_pressure = value),
            ToolCommand::SetOpacityPressure(value) => self.brush(|s| s.opacity_pressure = value),
            ToolCommand::SetSmoothing(value) => self.brush(|s| s.smoothing = value),
            ToolCommand::SetColor(color) => self.editor.try_preferences(|document| {
                if color[3] == 0 {
                    return Err(nyatidraw_web_core::WebError::InvalidBrush);
                }
                document.set_foreground(color);
                Ok(())
            }),
            ToolCommand::SwapColors => self.editor.try_preferences(|document| {
                let foreground = document.foreground();
                document.set_foreground(document.background());
                document.set_background(foreground);
                Ok(())
            }),
            _ => Err("이 도구 옵션은 웹판에 아직 연결되지 않았습니다.".into()),
        }
    }

    fn viewport_command(&self, command: ViewportCommand) {
        if command == ViewportCommand::FitDocument {
            self.editor.fit();
            return;
        }
        if let Some(runtime) = self.editor.runtime.peek().borrow_mut().as_mut() {
            runtime.fit_to_canvas = false;
            let rect = runtime.canvas.get_bounding_client_rect();
            let center = Point {
                x: rect.width() * 0.5,
                y: rect.height() * 0.5,
            };
            let view = runtime.viewport;
            match command {
                ViewportCommand::ZoomSteps(steps) => {
                    runtime.zoom_at(center, 1.25_f64.powi(i32::from(steps)));
                }
                ViewportCommand::ZoomAt {
                    steps,
                    logical_x,
                    logical_y,
                } => runtime.zoom_at(
                    Point {
                        x: f64::from(logical_x),
                        y: f64::from(logical_y),
                    },
                    1.25_f64.powi(i32::from(steps)),
                ),
                ViewportCommand::PanBy {
                    logical_x,
                    logical_y,
                } => {
                    runtime.viewport.pan.x += f64::from(logical_x);
                    runtime.viewport.pan.y += f64::from(logical_y);
                }
                ViewportCommand::CenterPageAt {
                    page_x_10k,
                    page_y_10k,
                } => {
                    let page = runtime.document.canvas();
                    let point = Point {
                        x: f64::from(page_x_10k) * f64::from(page.width_px) / 10_000.0,
                        y: f64::from(page_y_10k) * f64::from(page.height_px) / 10_000.0,
                    };
                    if let Some(display) = view.document_to_logical(point) {
                        runtime.viewport.pan.x += center.x - display.x;
                        runtime.viewport.pan.y += center.y - display.y;
                    }
                }
                _ => {
                    let (zoom, rotation, mirror) = match command {
                        ViewportCommand::ActualPixels => (1.0 / view.dpi_scale, 0.0, false),
                        ViewportCommand::RotateQuarterSteps(steps) => (
                            view.zoom,
                            view.rotation_radians + f64::from(steps) * std::f64::consts::FRAC_PI_4,
                            view.mirrored_horizontal,
                        ),
                        ViewportCommand::ToggleMirrorHorizontal => {
                            (view.zoom, view.rotation_radians, !view.mirrored_horizontal)
                        }
                        _ => (view.zoom, 0.0, view.mirrored_horizontal),
                    };
                    if let Some(changed) = view.with_view_at(center, zoom, rotation, mirror) {
                        runtime.viewport = changed;
                    }
                }
            }
            runtime.viewport.revision += 1;
        }
        self.editor.refresh();
        self.editor.redraw();
    }

    fn layer_command(&self, command: LayerCommand) -> Result<(), String> {
        match command {
            LayerCommand::AddRaster => self.editor.try_edit(|document| {
                document.add_layer(&format!(
                    "Layer {}",
                    document.layers().root().children.len() + 1
                ))
            }),
            LayerCommand::SetActive(layer) => self
                .editor
                .try_preferences(|document| document.set_active_layer(layer)),
            LayerCommand::SetLocked { layer, locked } => self
                .editor
                .try_edit(|document| document.set_layer_locked(layer, locked)),
            LayerCommand::SetAlphaLocked {
                layer,
                alpha_locked,
            } => self
                .editor
                .try_edit(|document| document.set_layer_alpha_locked(layer, alpha_locked)),
            LayerCommand::SetVisibility {
                node: LayerTreeNodeId::Raster(layer),
                visible,
            } => self
                .editor
                .try_edit(|document| document.set_layer_visible(layer, visible)),
            LayerCommand::SetOpacity {
                node: LayerTreeNodeId::Raster(layer),
                opacity_u16,
            } => self.editor.try_edit(|document| {
                document.set_layer_opacity(layer, f32::from(opacity_u16) / 65535.0)
            }),
            LayerCommand::SetBlendMode {
                node: LayerTreeNodeId::Raster(layer),
                blend_mode,
            } => self
                .editor
                .try_edit(|document| document.set_layer_blend_mode(layer, blend_mode)),
            LayerCommand::Rename {
                node: LayerTreeNodeId::Raster(layer),
                name,
            } => self
                .editor
                .try_edit(|document| document.rename_layer(layer, &name)),
            LayerCommand::Delete(LayerTreeNodeId::Raster(layer)) => self
                .editor
                .try_edit(|document| document.delete_layer(layer)),
            LayerCommand::Reorder {
                node: LayerTreeNodeId::Raster(layer),
                index,
                ..
            } => self
                .editor
                .try_edit(|document| document.reorder_layer(layer, index)),
            _ => Err("이 레이어 명령은 웹판에 아직 연결되지 않았습니다.".into()),
        }
    }
}

impl UiBackend for WebUiBackend {
    fn protocol_snapshot(&self) -> (UiProjection, Option<EventEnvelope>) {
        self.restore_project_tool();
        let mut projection = UiProjection::empty();
        projection.document_title = "NyatiDraw Web · 실험판".into();
        projection.revision = Revision(*self.editor.revision.peek());
        projection.workspace = WorkspaceProjection::Loading;
        projection.dock = self.preferences.borrow().dock.clone();
        projection.pencil_template = self.preferences.borrow().pencil;
        self.editor.read(|runtime| {
            let document = &runtime.document;
            let settings = document.brush_settings();
            let preset = document.brush_preset();
            projection.workspace = WorkspaceProjection::Ready;
            projection.canvas = document.canvas();
            projection.active_layer = Some(document.active_layer());
            projection.drawing_tool = runtime.selected_tool;
            projection.brush_size_tenths = ratio_u16(settings.size_px * 10.0);
            projection.brush_opacity_u16 = ratio_u16(settings.opacity * 65535.0);
            projection.brush_settings = BrushSettings {
                size_pressure: settings.size_pressure,
                opacity_pressure: settings.opacity_pressure,
                size_minimum_u16: ratio_u16(preset.size_min_ratio * 65535.0),
                opacity_minimum_u16: ratio_u16(preset.opacity_min_ratio * 65535.0),
                hardness_u16: ratio_u16(settings.hardness * 65535.0),
                smoothing: settings.smoothing,
            };
            projection.brush_color = document.foreground();
            projection.background_color = document.background();
            projection.recent_colors.clone_from(&runtime.recent_colors);
            projection
                .recent_brush_sizes
                .clone_from(&runtime.recent_sizes);
            projection.edit.can_cancel = document.is_drawing();
            projection.layers = document
                .layers()
                .root()
                .children
                .iter()
                .enumerate()
                .rev()
                .filter_map(|(index, node)| {
                    let LayerTreeNode::Raster(layer) = node else {
                        return None;
                    };
                    Some(LayerProjection {
                        alpha_locked: layer.alpha_locked,
                        clip_to_below: layer.clip_to_below,
                        blend_mode: layer.blend_mode,
                        id: LayerTreeNodeId::Raster(layer.id),
                        parent: document.layers().root_id(),
                        index,
                        depth: 1,
                        kind: LayerProjectionKind::Raster,
                        name: layer.name.clone(),
                        visible: layer.visible,
                        locked: layer.locked,
                        reference: layer.reference,
                        opacity_u16: layer.opacity_u16,
                    })
                })
                .collect();
            projection.history.entries = (0..document.undo_len())
                .map(|_| HistoryEntryProjection {
                    operation: HistoryOperationLabel::Structural,
                })
                .chain(std::iter::once(HistoryEntryProjection {
                    operation: HistoryOperationLabel::Initial,
                }))
                .collect();
            projection.viewport = ViewportProjection {
                pan_x_milli: bounded_i64(runtime.viewport.pan.x * 1000.0),
                pan_y_milli: bounded_i64(runtime.viewport.pan.y * 1000.0),
                zoom_ppm: bounded_u32(runtime.viewport.zoom * 1_000_000.0),
                rotation_millidegrees: bounded_i32(
                    runtime.viewport.rotation_radians.to_degrees() * 1000.0,
                ),
                mirrored_horizontal: runtime.viewport.mirrored_horizontal,
            };
        });
        let event = (projection.workspace == WorkspaceProjection::Ready).then_some(EventEnvelope {
            sequence: projection.revision.0,
            event: EditorEvent::ProjectionChanged {
                revision: projection.revision,
            },
        });
        (projection, event)
    }

    fn supports_command(&self, command: &EditorCommand) -> bool {
        use EditorCommand as C;
        match command {
            C::History(HistoryCommand::Undo) => {
                self.editor.read(|r| r.document.can_undo()).unwrap_or(false)
            }
            C::History(HistoryCommand::Redo) => {
                self.editor.read(|r| r.document.can_redo()).unwrap_or(false)
            }
            C::Project(_)
            | C::Viewport(_)
            | C::Edit(EditCommand::ClearActiveLayer | EditCommand::ResizePage { .. })
            | C::Dock(DockCommand::ActivatePanel(_) | DockCommand::ResetToSafeDefault) => true,
            C::Tool(tool) => match tool {
                ToolCommand::Select(value) => matches!(
                    value,
                    DrawingTool::Move
                        | DrawingTool::Pencil
                        | DrawingTool::Pen
                        | DrawingTool::Brush
                        | DrawingTool::Eraser
                        | DrawingTool::Eyedropper
                ),
                ToolCommand::CancelGesture
                | ToolCommand::CycleBrushFamily
                | ToolCommand::CycleSelectionFamily
                | ToolCommand::SelectPencilTemplate(_)
                | ToolCommand::SetSizeTenths(_)
                | ToolCommand::AdjustSizeSteps { .. }
                | ToolCommand::SetOpacityU16(_)
                | ToolCommand::SetSizePressure(_)
                | ToolCommand::SetOpacityPressure(_)
                | ToolCommand::SetHardnessU16(_)
                | ToolCommand::SetSmoothing(_)
                | ToolCommand::SetColor(_)
                | ToolCommand::SwapColors => true,
                _ => false,
            },
            C::Layer(layer) => matches!(
                layer,
                LayerCommand::AddRaster
                    | LayerCommand::SetActive(_)
                    | LayerCommand::SetLocked { .. }
                    | LayerCommand::SetAlphaLocked { .. }
                    | LayerCommand::SetVisibility {
                        node: LayerTreeNodeId::Raster(_),
                        ..
                    }
                    | LayerCommand::SetOpacity {
                        node: LayerTreeNodeId::Raster(_),
                        ..
                    }
                    | LayerCommand::SetBlendMode {
                        node: LayerTreeNodeId::Raster(_),
                        ..
                    }
                    | LayerCommand::Rename {
                        node: LayerTreeNodeId::Raster(_),
                        ..
                    }
                    | LayerCommand::Delete(LayerTreeNodeId::Raster(_))
                    | LayerCommand::Reorder {
                        node: LayerTreeNodeId::Raster(_),
                        new_parent: nyatidraw_api::GroupId(0),
                        ..
                    }
            ),
            C::Dock(DockCommand::Replace(dock)) => {
                same_topology(self.preferences.borrow().dock.root(), dock.root())
                    && self.preferences.borrow().dock.top() == dock.top()
            }
            _ => false,
        }
    }

    fn push_editor_command(
        &self,
        based_on: Revision,
        command: EditorCommand,
    ) -> Result<(), String> {
        self.restore_project_tool();
        if based_on != Revision(*self.editor.revision.peek()) {
            return Err("화면 상태가 바뀌었습니다. 다시 시도해주세요.".into());
        }
        if !self.supports_command(&command) {
            return Err("이 기능은 웹판에 아직 연결되지 않았습니다.".into());
        }
        if self.editor.modal.peek().is_some() {
            return Err("열려 있는 대화상자를 먼저 닫아주세요.".into());
        }
        if self.editor.read(|_| ()).is_none() {
            return Err("그림을 여는 중입니다.".into());
        }
        if self
            .editor
            .read(|r| r.document.is_drawing())
            .unwrap_or(false)
            && command != EditorCommand::Tool(ToolCommand::CancelGesture)
        {
            return Err("먼저 현재 획을 마쳐주세요.".into());
        }
        match command {
            EditorCommand::Project(_) => self.editor.download_project(),
            EditorCommand::Tool(command) => return self.tool_command(command),
            EditorCommand::Layer(command) => return self.layer_command(command),
            EditorCommand::Viewport(command) => self.viewport_command(command),
            EditorCommand::History(HistoryCommand::Undo) => {
                return self.editor.try_edit(WebDocument::undo);
            }
            EditorCommand::History(HistoryCommand::Redo) => {
                return self.editor.try_edit(WebDocument::redo);
            }
            EditorCommand::Edit(EditCommand::ClearActiveLayer) => {
                return self.editor.try_edit(WebDocument::clear_active_layer);
            }
            EditorCommand::Edit(EditCommand::ResizePage {
                size: [width_px, height_px],
            }) => {
                return self.editor.try_edit(|document| {
                    document.set_canvas(nyatidraw_api::CanvasSpec {
                        width_px,
                        height_px,
                        pixels_per_inch: document.canvas().pixels_per_inch,
                    })
                });
            }
            EditorCommand::Dock(DockCommand::ActivatePanel(panel)) => {
                self.preferences.borrow_mut().dock.activate_panel(panel);
                self.editor.refresh();
            }
            EditorCommand::Dock(DockCommand::ResetToSafeDefault) => {
                self.preferences.borrow_mut().dock = DockTree::safe_default();
                self.editor.refresh();
            }
            EditorCommand::Dock(DockCommand::Replace(dock)) => {
                self.preferences.borrow_mut().dock = dock;
                self.editor.refresh();
            }
            _ => return Err("이 기능은 웹판에 아직 연결되지 않았습니다.".into()),
        }
        Ok(())
    }

    fn project_epoch(&self) -> u64 {
        self.editor.read(|r| r.project_epoch).unwrap_or(0)
    }
    fn navigator_snapshot(&self) -> NavigatorSnapshot {
        self.update_previews();
        let mut snapshot = self.previews.borrow().1.clone();
        snapshot.viewport = self.editor.read(|runtime| {
            let rect = runtime.canvas.get_bounding_client_rect();
            let page = runtime.document.canvas();
            NavigatorViewport {
                quad_page_10k: [
                    [0.0, 0.0],
                    [rect.width(), 0.0],
                    [rect.width(), rect.height()],
                    [0.0, rect.height()],
                ]
                .map(|[x, y]| {
                    let point = runtime
                        .viewport
                        .logical_to_document(Point { x, y })
                        .unwrap_or_default();
                    [
                        bounded_i32(point.x / f64::from(page.width_px) * 10_000.0),
                        bounded_i32(point.y / f64::from(page.height_px) * 10_000.0),
                    ]
                }),
            }
        });
        snapshot
    }
    fn layer_thumbnail_snapshot(&self) -> LayerThumbnailSnapshot {
        self.update_previews();
        self.previews.borrow().2.clone()
    }
    fn panel_heights(&self) -> PanelHeights {
        self.preferences.borrow().heights.clone()
    }
    fn submit_panel_heights(&self, heights: &PanelHeights) {
        self.preferences.borrow_mut().heights = heights.clone();
        self.save_preference(
            "heights",
            &heights
                .values
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    fn workspace_appearance(&self) -> WorkspaceAppearance {
        self.preferences.borrow().appearance
    }
    fn set_workspace_appearance(&self, appearance: WorkspaceAppearance) {
        self.preferences.borrow_mut().appearance = appearance;
        if let Some(runtime) = self.editor.runtime.peek().borrow_mut().as_mut() {
            runtime
                .renderer
                .set_background(appearance.background_colors());
        }
        self.editor.redraw();
        self.save_preference(
            "appearance",
            &format!(
                "{}:{:02x}{:02x}{:02x}",
                u8::from(appearance.checkerboard),
                appearance.solid_rgb[0],
                appearance.solid_rgb[1],
                appearance.solid_rgb[2]
            ),
        );
    }
    fn palette_pins(&self) -> [Option<[u8; 4]>; 10] {
        self.preferences.borrow().pins
    }
    fn save_palette_pins(&self, pins: [Option<[u8; 4]>; 10]) {
        self.preferences.borrow_mut().pins = pins;
        self.save_preference(
            "pins",
            &pins
                .iter()
                .map(|value| {
                    value.map_or_else(
                        || "-".to_owned(),
                        |color| format!("{:08x}", u32::from_be_bytes(color)),
                    )
                })
                .collect::<Vec<_>>()
                .join(":"),
        );
    }
    fn now_millis(&self) -> f64 {
        js_sys::Date::now()
    }
    fn preview_preset(&self, _projection: &UiProjection) -> nyatidraw_web_core::BrushPreset {
        self.editor
            .read(|r| r.document.brush_preset())
            .unwrap_or_else(|| WebDocument::default().brush_preset())
    }
}

pub fn drawing_tool(tool: WebTool) -> DrawingTool {
    match tool {
        WebTool::Pencil2H | WebTool::Pencil2B => DrawingTool::Pencil,
        WebTool::Pen => DrawingTool::Pen,
        WebTool::SoftBrush => DrawingTool::Brush,
        WebTool::Eraser => DrawingTool::Eraser,
    }
}

fn same_topology(left: &nyatidraw_api::DockNode, right: &nyatidraw_api::DockNode) -> bool {
    use nyatidraw_api::DockNode;
    match (left, right) {
        (DockNode::Panel(left), DockNode::Panel(right)) => left == right,
        (
            DockNode::Tabs {
                active: la,
                panels: lp,
            },
            DockNode::Tabs {
                active: ra,
                panels: rp,
            },
        ) => la == ra && lp == rp,
        (
            DockNode::Split {
                axis: la,
                first: lf,
                second: ls,
                ..
            },
            DockNode::Split {
                axis: ra,
                first: rf,
                second: rs,
                ..
            },
        ) => la == ra && same_topology(lf, rf) && same_topology(ls, rs),
        _ => false,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ratio_u16(value: f32) -> u16 {
    value.round().clamp(0.0, 65535.0) as u16
}
#[allow(clippy::cast_possible_truncation)]
fn bounded_i32(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
#[allow(clippy::cast_possible_truncation)]
fn bounded_i64(value: f64) -> i64 {
    value.round().clamp(-9.0e18, 9.0e18) as i64
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn bounded_u32(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}
