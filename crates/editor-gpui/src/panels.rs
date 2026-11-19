use std::{collections::BTreeMap, ops::RangeInclusive, sync::Arc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
};
use gpui_kit::{prelude::*, *};
use nyatidraw_api::*;

use crate::EditorView;

mod artwork;
mod layers;
mod reference;
mod timeline;

gpui_kit::assets::icon_assets!(
    PanelIcons,
    [ChevronDown, ChevronRight, Ellipsis, GripVertical, Pin]
);

fn icon(name: IconName) -> Svg {
    let data = PanelIcons.load(name.path().as_ref()).ok().flatten();
    svg()
        .when_some(data, |image, bytes| image.data(&bytes))
        .size(px(14.))
        .text_color(rgb(0xf1f1f1))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Thumbnail {
    Brush,
    Layer(LayerId),
    Mask(MaskId),
    Frame(FrameId),
}

struct SliderControl {
    binding: EditorCommand,
    authority: i32,
    state: Entity<SliderState>,
    _subscription: Subscription,
}

struct TextControl {
    target: String,
    authority: String,
    state: Entity<InputState>,
    _subscription: Subscription,
    preview: bool,
    dirty: bool,
}

#[derive(Default)]
pub(crate) struct PanelState {
    reference: reference::Reference,
    navigating: bool,
    pending_center: Option<ViewportCommand>,
    sliders: BTreeMap<&'static str, SliderControl>,
    fields: BTreeMap<&'static str, TextControl>,
    palette: Option<nyatidraw_editor_ui::palette::Palette>,
    group: Option<GroupId>,
    last_active_layer: Option<LayerId>,
    layer_options: Option<LayerTreeNodeId>,
    collapsed_groups: std::collections::BTreeSet<GroupId>,
    layer_drop: Option<(LayerTreeNodeId, layers::DropPosition)>,
    managed_preset: Option<BrushPresetName>,
    selected_brush_preset: Option<BrushPresetName>,
    pending_correction: Option<(LayerId, ColorAdjustment)>,
    preset_management: bool,
    brush_options_open: bool,
    confirm_preset: Option<(BrushPresetName, bool)>,
    selection_radius: Option<u8>,
    input_error: Option<String>,
    thumbnails: BTreeMap<Thumbnail, (Arc<str>, Arc<RenderImage>)>,
    frames_page: usize,
    frames_scroll: ScrollHandle,
    frames_focus: Option<FocusHandle>,
    last_frame: Option<FrameId>,
    requested: Option<(u64, u64, Vec<FrameId>)>,
    brush_sample: Option<(BrushPresetValues, Result<Arc<str>, String>)>,
}

impl PanelState {
    pub(crate) fn frames_have_focus(&self, window: &Window) -> bool {
        self.frames_focus
            .as_ref()
            .is_some_and(|f| f.is_focused(window))
    }

    pub(crate) fn cancel_reference_contact(&mut self) -> bool {
        self.reference.cancel_contact()
    }
    pub(crate) fn has_unapplied_preview(&self) -> bool {
        self.fields
            .values()
            .any(|field| field.preview && field.dirty)
    }

    pub(crate) fn reset_document(&mut self, window: &mut Window, cx: &mut App) {
        for (_, pixels) in std::mem::take(&mut self.thumbnails).into_values() {
            cx.drop_image(pixels, Some(window));
        }
        let mut reference = std::mem::take(&mut self.reference);
        reference.cancel_contact();
        *self = Self {
            reference,
            palette: self.palette.take(),
            ..Self::default()
        };
    }
}

struct PanelUi<'a, 'cx> {
    view: &'a mut EditorView,
    window: &'a mut Window,
    cx: &'a mut Context<'cx, EditorView>,
    revision: Revision,
    preview_fields: bool,
}

#[derive(Clone, Copy, Debug)]
enum NumberTarget {
    Brush(DrawingTool),
    Layer(LayerTreeNodeId),
}

enum NumberScale {
    BrushSize,
    Percent { min: i32 },
    Strength,
}

fn column() -> Div {
    div().flex().flex_col().gap_2().min_w_0()
}

fn row() -> Div {
    div().flex().flex_row().items_center().flex_wrap().gap_1()
}

fn busy(p: &UiProjection) -> bool {
    p.edit.busy
        || p.edit.transform.is_some()
        || p.edit.color_adjustment.is_some()
        || p.animation.playing
}

impl PanelUi<'_, '_> {
    fn number_slider(
        &mut self,
        label: &'static str,
        target: NumberTarget,
        value: i32,
        scale: NumberScale,
        enabled: bool,
        command: impl Fn(i32) -> EditorCommand + Copy + 'static,
    ) -> Div {
        let (range, factor, unit) = match scale {
            NumberScale::BrushSize => (1..=2000, 10., "px"),
            NumberScale::Percent { min } => (min..=100, 1., "%"),
            NumberScale::Strength => (0..=100, 1., ""),
        };
        let enabled = enabled && self.view.host.supports_command(&command(value));
        let edit_range = range.clone();
        let input = self.field_input(
            label,
            format!("{target:?}"),
            (value as f32 / factor).to_string(),
            enabled,
            true,
            move |v, text, cx| {
                let p = v.host.snapshot();
                let same_target = match target {
                    NumberTarget::Brush(tool) => p.drawing_tool == tool,
                    NumberTarget::Layer(id) => layers::selected_layer(v, &p) == Some(id),
                };
                if !same_target {
                    return Ok(());
                }
                let number = text
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or("숫자를 입력하세요.")?;
                let value = ((number * factor).round() as i32)
                    .clamp(*edit_range.start(), *edit_range.end());
                v.send(command(value), cx);
                Ok(())
            },
        );
        let value_editor = row()
            .flex_nowrap()
            .gap(px(3.))
            .child(input.w(px(52.)).text_right())
            .when(!unit.is_empty(), |r| r.child(unit));
        if matches!(scale, NumberScale::Strength) {
            self.bound_slider(
                label,
                value,
                range,
                value_editor,
                enabled,
                command(0),
                move |view, value, released, window, cx| {
                    if released {
                        view.send(command(value), cx);
                    } else if let Some(field) = view.panels.fields.get(label) {
                        field.state.update(cx, |input, cx| {
                            input.set_value(value.to_string(), window, cx);
                        });
                    }
                },
            )
        } else {
            self.slider(label, value, range, value_editor, enabled, command)
        }
    }

    fn cached_thumbnail(&self, key: Thumbnail) -> Option<Img> {
        let (_, pixels) = self.view.panels.thumbnails.get(&key)?;
        Some(img(pixels.clone()).object_fit(ObjectFit::Contain))
    }

    fn thumbnail(&mut self, key: Thumbnail, uri: &Arc<str>) -> Option<Img> {
        use base64::Engine as _;
        let cache = &mut self.view.panels.thumbnails;
        if !cache
            .get(&key)
            .is_some_and(|(cached, _)| Arc::ptr_eq(cached, uri) || cached == uri)
        {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(uri.strip_prefix("data:image/png;base64,")?)
                .ok()?;
            // These bounded panel previews are ready to paint on the frame that swaps them.
            let pixels = Image::from_bytes(ImageFormat::Png, bytes)
                .to_image_data(self.cx.svg_renderer())
                .ok()?;
            if let Some((_, old)) = cache.insert(key, (uri.clone(), pixels)) {
                self.cx.drop_image(old, Some(self.window));
            }
        }
        self.cached_thumbnail(key)
    }

    fn keep_thumbnails(&mut self, keep: impl Fn(Thumbnail) -> bool) {
        self.view.panels.thumbnails.retain(|key, (_, source)| {
            if keep(*key) {
                true
            } else {
                self.cx.drop_image(source.clone(), Some(self.window));
                false
            }
        });
    }
    fn button(
        &self,
        label: impl Into<SharedString>,
        command: EditorCommand,
        selected: bool,
        enabled: bool,
    ) -> Button {
        let label = label.into();
        let based_on = self.revision;
        Button::new(label.clone())
            .label(label)
            .small()
            .compact()
            .ghost()
            .selected(selected)
            .disabled(!enabled || !self.view.host.supports_command(&command))
            .on_click(self.cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus, cx);
                view.send_at(based_on, command.clone(), cx);
            }))
    }

    fn heading(&self, label: impl Into<SharedString>) -> Div {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xdfdfdf))
            .child(label.into())
    }

    // Each target owns its pending drag. Replacing the target drops the old subscription.
    fn slider(
        &mut self,
        label: &'static str,
        value: i32,
        range: RangeInclusive<i32>,
        display: impl IntoElement,
        enabled: bool,
        command: impl Fn(i32) -> EditorCommand + 'static,
    ) -> Div {
        let enabled = enabled && self.view.host.supports_command(&command(value));
        let binding = command(*range.start());
        self.bound_slider(
            label,
            value,
            range,
            display,
            enabled,
            binding,
            move |view, value, released, _, cx| {
                if released {
                    view.send(command(value), cx);
                }
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn bound_slider(
        &mut self,
        label: &'static str,
        value: i32,
        range: RangeInclusive<i32>,
        display: impl IntoElement,
        enabled: bool,
        binding: EditorCommand,
        update: impl Fn(&mut EditorView, i32, bool, &mut Window, &mut Context<EditorView>) + 'static,
    ) -> Div {
        let controls = &mut self.view.panels.sliders;
        if controls.get(label).is_none_or(|c| c.binding != binding) {
            let state = self.cx.new(|_| {
                SliderState::new()
                    .min(*range.start() as f32)
                    .max(*range.end() as f32)
                    .step(1.0)
                    .default_value(value as f32)
            });
            let subscription =
                self.cx
                    .subscribe_in(
                        &state,
                        self.window,
                        move |view, _, event, window, cx| match event {
                            SliderEvent::Change(SliderValue::Single(value)) => {
                                update(view, value.round() as i32, false, window, cx);
                            }
                            SliderEvent::Release(SliderValue::Single(value)) => {
                                update(view, value.round() as i32, true, window, cx);
                            }
                            _ => {}
                        },
                    );
            controls.insert(
                label,
                SliderControl {
                    binding,
                    authority: value,
                    state,
                    _subscription: subscription,
                },
            );
        }
        let control = controls.get_mut(label).expect("slider was installed");
        if control.authority != value {
            control.authority = value;
            control
                .state
                .update(self.cx, |s, cx| s.set_value(value as f32, self.window, cx));
        }
        column()
            .gap_0()
            .child(
                row()
                    .flex_nowrap()
                    .justify_between()
                    .child(label)
                    .child(display),
            )
            .child(
                Slider::new(&control.state)
                    .accessibility_label(label)
                    .disabled(!enabled),
            )
    }

    fn field(
        &mut self,
        label: &'static str,
        target: String,
        value: String,
        enabled: bool,
        commit: impl Fn(&mut EditorView, &str, &mut Context<EditorView>) -> Result<(), &'static str>
        + 'static,
    ) -> Div {
        let input = self.field_input(label, target, value, enabled, false, commit);
        column().gap_0().child(self.heading(label)).child(input)
    }

    fn field_input(
        &mut self,
        label: &'static str,
        target: String,
        value: String,
        enabled: bool,
        commit_on_blur: bool,
        commit: impl Fn(&mut EditorView, &str, &mut Context<EditorView>) -> Result<(), &'static str>
        + 'static,
    ) -> Input {
        let fields = &mut self.view.panels.fields;
        if fields.get(label).is_none_or(|field| field.target != target) {
            let state = self.cx.new(|cx| {
                InputState::new(self.window, cx)
                    .placeholder(label)
                    .default_value(value.clone())
            });
            let subscription = self.cx.subscribe_in(
                &state,
                self.window,
                move |view, input, event: &InputEvent, window, cx| {
                    let Some(field) = view.panels.fields.get_mut(label) else {
                        return;
                    };
                    if field.state.entity_id() != input.entity_id() {
                        return;
                    }
                    if matches!(event, InputEvent::Change) {
                        field.dirty = input.read(cx).value().as_str() != field.authority;
                        if field.preview {
                            cx.notify();
                        }
                    }
                    let enter = matches!(event, InputEvent::PressEnter { .. });
                    if enter || (commit_on_blur && field.dirty && matches!(event, InputEvent::Blur))
                    {
                        let text = input.read(cx).value();
                        let error = commit(view, text.as_str(), cx).err().map(str::to_owned);
                        if let Some(field) = view.panels.fields.get_mut(label) {
                            if commit_on_blur {
                                // Restore the authoritative, rounded value after the edit.
                                field.authority.clear();
                                field.dirty = false;
                            } else if error.is_none() && view.notice.is_none() {
                                field.dirty = false;
                            }
                        }
                        if enter && error.is_none() && view.notice.is_none() {
                            window.focus(&view.focus, cx);
                        }
                        view.panels.input_error = error;
                        cx.notify();
                    }
                },
            );
            fields.insert(
                label,
                TextControl {
                    target,
                    authority: value.clone(),
                    state,
                    _subscription: subscription,
                    preview: self.preview_fields,
                    dirty: false,
                },
            );
        }
        let control = fields.get_mut(label).expect("field was installed");
        if control.authority != value {
            control.authority.clone_from(&value);
            control
                .state
                .update(self.cx, |input, cx| input.set_value(value, self.window, cx));
        }
        Input::new(&control.state)
            .small()
            .aria_label(label)
            .disabled(!enabled)
    }
}

pub(crate) fn panel(
    kind: PanelKind,
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> AnyElement {
    let p = view.projection.clone();
    let preview_fields = kind == PanelKind::ToolProperties
        && (p.edit.transform.is_some() || p.edit.color_adjustment.is_some());
    if p.edit.transform.is_none() && p.edit.color_adjustment.is_none() {
        view.panels.fields.retain(|_, field| !field.preview);
    }
    if p.edit.color_adjustment.is_none() {
        view.panels.pending_correction = None;
    }
    let mut ui = PanelUi {
        view,
        window,
        cx,
        revision: p.revision,
        preview_fields,
    };
    let content = match kind {
        PanelKind::Tools => artwork::tools(&ui, &p),
        PanelKind::Brush => artwork::presets(&mut ui, &p),
        PanelKind::BrushSizes => artwork::sizes(&ui, &p),
        PanelKind::ToolProperties => artwork::properties(&mut ui, &p),
        PanelKind::Color => colors(&mut ui, &p, false),
        PanelKind::QuickColors => colors(&mut ui, &p, true),
        PanelKind::Layers => layers::layers(&mut ui, &p),
        PanelKind::History => history(&ui, &p),
        PanelKind::Navigator => navigator(&mut ui, &p),
        PanelKind::Canvas | PanelKind::CanvasActions | PanelKind::Viewport => {
            unreachable!("rendered by workspace")
        }
    };
    if kind == PanelKind::QuickColors {
        return content.into_any_element();
    }
    let panel_id = match kind {
        PanelKind::ToolProperties if p.edit.color_adjustment.is_some() => {
            "panel-ToolProperties-color-adjustment".to_owned()
        }
        PanelKind::ToolProperties if p.edit.transform.is_some() => {
            "panel-ToolProperties-transform".to_owned()
        }
        PanelKind::ToolProperties => format!("panel-ToolProperties-{:?}", p.drawing_tool),
        _ => format!("panel-{kind:?}"),
    };
    div()
        .id(panel_id)
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .text_sm()
        .child(content)
        .when_some(ui.view.panels.input_error.as_ref(), |d, message| {
            d.child(
                div()
                    .text_color(ui.cx.theme().danger)
                    .child(message.clone()),
            )
        })
        .into_any_element()
}

pub(crate) fn animation(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> AnyElement {
    let p = view.projection.clone();
    timeline::animation(
        &mut PanelUi {
            view,
            window,
            cx,
            revision: p.revision,
            preview_fields: false,
        },
        &p,
    )
    .into_any_element()
}

fn colors(ui: &mut PanelUi, p: &UiProjection, quick: bool) -> Div {
    let can_pick = ui
        .view
        .host
        .supports_command(&EditorCommand::Tool(ToolCommand::SetColor(p.brush_color)));
    let palette = ui.view.panels.palette.get_or_insert_with(|| {
        nyatidraw_editor_ui::palette::Palette::new(ui.view.host.palette_pins())
    });
    palette.observe(&p.recent_colors);
    let slots = palette.slots;
    let mut recent = row()
        .flex_nowrap()
        .gap(px(3.))
        .h(px(22.))
        .flex_shrink_0()
        .when(quick, |d| d.w(px(237.)).max_w_full())
        .when(!quick, |d| d.w_full());
    for (index, slot_color) in slots.into_iter().enumerate() {
        let slot = div()
            .id(("palette-slot", index))
            .group("palette-slot")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x444444))
            .bg(rgb(0x505050));
        let Some(entry) = slot_color else {
            recent = recent.child(slot);
            continue;
        };
        let color = entry.color;
        let label = format!("#{:08X}", u32::from_be_bytes(color));
        let swatch = Button::new(("recent-color", index))
            .small()
            .compact()
            .ghost()
            .tooltip(label.clone())
            .accessibility_label(label)
            .disabled(!can_pick)
            .size_full()
            .p_0()
            .child(
                div()
                    .size_full()
                    .rounded_sm()
                    .when(color == p.brush_color, |d| {
                        d.border_1().border_color(white())
                    })
                    .bg(rgba(u32::from_be_bytes(color))),
            )
            .on_click(ui.cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus, cx);
                view.send(EditorCommand::Tool(ToolCommand::SetColor(color)), cx)
            }));
        let pinned = entry.pinned;
        let pin_label = if pinned {
            "색상 고정 해제"
        } else {
            "색상 고정"
        };
        let pin = div()
            .absolute()
            .occlude()
            .right_0()
            .top_0()
            .size(px(12.))
            .when(!pinned, |d| {
                d.opacity(0.).group_hover("palette-slot", |s| s.opacity(1.))
            })
            .child(
                Button::new("pin-color")
                    .small()
                    .compact()
                    .size_full()
                    .p_0()
                    .tooltip(pin_label)
                    .accessibility_label(pin_label)
                    .child(icon(IconName::Pin).size(px(10.)))
                    .on_click(ui.cx.listener(move |v, _, window, cx| {
                        cx.stop_propagation();
                        let Some(mut next) = v.panels.palette.clone() else {
                            return;
                        };
                        let Some(pins) = next.toggle_pin(index, color) else {
                            return;
                        };
                        match v.host.save_palette_pins(pins) {
                            Ok(()) => v.panels.palette = Some(next),
                            Err(message) => v.notice = Some(message),
                        }
                        window.focus(&v.focus, cx);
                        cx.notify();
                    })),
            );
        recent = recent.child(slot.child(swatch).child(pin));
    }
    if quick {
        let chip = |color: [u8; 4]| {
            div()
                .absolute()
                .size(px(23.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(0x343434))
                .bg(rgba(u32::from_be_bytes(color)))
        };
        let cmd = EditorCommand::Tool(ToolCommand::SwapColors);
        let based_on = p.revision;
        let swap = Button::new("swap-colors")
            .small()
            .compact()
            .ghost()
            .w(px(39.))
            .h(px(30.))
            .p_0()
            .accessibility_label("전경/배경 색상 교환")
            .tooltip("전경/배경 색상 교환 (X)")
            .disabled(!ui.view.host.supports_command(&cmd))
            .child(
                div()
                    .relative()
                    .w(px(35.))
                    .h(px(29.))
                    .child(chip(p.background_color).right_0().bottom_0())
                    .child(chip(p.brush_color).left_0().top_0())
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .bottom_0()
                            .text_size(px(10.))
                            .line_height(px(11.))
                            .bg(rgb(0x454545))
                            .text_color(white())
                            .child("X"),
                    ),
            )
            .on_click(ui.cx.listener(move |v, _, window, cx| {
                window.focus(&v.focus, cx);
                v.send_at(based_on, cmd.clone(), cx);
            }));
        row().flex_nowrap().child(swap).child(recent)
    } else {
        column()
            .size_full()
            .min_h_0()
            .gap(px(6.))
            .when(can_pick, |el| {
                el.child(ui.view.color_wheel(p.brush_color, ui.cx))
            })
            .child(recent)
    }
}

fn history(ui: &PanelUi, p: &UiProjection) -> Div {
    let h = &p.history;
    let mut panel = column().child(
        row()
            .child(ui.button(
                "실행 취소",
                EditorCommand::History(HistoryCommand::Undo),
                false,
                !busy(p),
            ))
            .child(ui.button(
                "다시 실행",
                EditorCommand::History(HistoryCommand::Redo),
                false,
                !busy(p),
            )),
    );
    let name = |op| match op {
        HistoryOperationLabel::Initial => "보관 시작점",
        HistoryOperationLabel::Stroke => "스트로크",
        HistoryOperationLabel::Structural => "구조 변경",
    };
    for branch in &h.redo_branches {
        panel = panel.child(ui.button(
            format!("{} · 분기 {}", name(branch.operation), branch.node.0),
            EditorCommand::History(HistoryCommand::RedoTo(branch.node)),
            false,
            !busy(p),
        ));
    }
    if h.redo_page_after.is_some() {
        panel = panel.child(ui.button(
            "처음 분기",
            EditorCommand::History(HistoryCommand::ShowRedoBranches { after: None }),
            false,
            true,
        ));
    }
    if h.has_more_redo_branches {
        panel = panel.child(ui.button(
            "다음 분기",
            EditorCommand::History(HistoryCommand::ShowRedoBranches {
                after: h.redo_branches.last().map(|b| b.node),
            }),
            false,
            true,
        ));
    }
    for (index, entry) in h.entries.iter().enumerate() {
        let mut line = row().child(name(entry.operation));
        if index == h.current {
            line = line.child(ui.heading("현재"));
        }
        panel = panel.child(line);
    }
    panel.when(h.has_older_entries, |d| {
        d.child(ui.heading("이전 작업 생략"))
    })
}

fn checkerboard() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |b, (), w, _| {
            w.paint_quad(fill(b, rgb(0xe5e5e5)));
            let cell = 6.;
            let rows = (f32::from(b.size.height) / cell).ceil() as i32;
            let columns = (f32::from(b.size.width) / cell).ceil() as i32;
            for row in 0..rows {
                for col in ((row % 2)..columns).step_by(2) {
                    let rect = Bounds::new(
                        b.origin + point(px(col as f32 * cell), px(row as f32 * cell)),
                        size(px(cell), px(cell)),
                    );
                    w.paint_quad(fill(rect.intersect(&b), rgb(0xbbbbbb)));
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn center_at(point: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<ViewportCommand> {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    let p = point - bounds.origin;
    let unit = |v: Pixels, extent: Pixels| {
        ((f32::from(v) / f32::from(extent)).clamp(0., 1.) * 10_000.).round() as i32
    };
    Some(ViewportCommand::CenterPageAt {
        page_x_10k: unit(p.x, bounds.size.width),
        page_y_10k: unit(p.y, bounds.size.height),
    })
}

fn navigator(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let reference_tabs = reference::tabs(ui);
    if ui.view.panels.reference.show {
        return column()
            .size_full()
            .min_h_0()
            .child(reference_tabs)
            .child(reference::panel(ui, p));
    }
    let preview = ui
        .view
        .host
        .navigator_preview(ui.window, ui.cx)
        .map(|image| {
            let ratio = p.canvas.width_px.max(1) as f32 / p.canvas.height_px.max(1) as f32;
            let bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
            let capture = bounds.clone();
            let press = bounds.clone();
            let motion = bounds.clone();
            let quad = ui.view.host.navigator_quad();
            let center = EditorCommand::Viewport(ViewportCommand::CenterPageAt {
                page_x_10k: 5000,
                page_y_10k: 5000,
            });
            div()
                .id("navigator-image")
                .relative()
                .w(px(160. * ratio.min(1.)))
                .max_w_full()
                .aspect_ratio(ratio)
                .overflow_hidden()
                .border_1()
                .border_color(rgb(0x242424))
                .child(checkerboard())
                .child(image)
                .child(
                    canvas(
                        move |r, _, _| capture.set(r),
                        move |area, _, window, _| {
                            let Some(points) = quad else {
                                return;
                            };
                            let mut outline = PathBuilder::stroke(px(1.4));
                            for (n, [x, y]) in points.into_iter().enumerate() {
                                let point = area.origin
                                    + gpui_kit::point(
                                        area.size.width * (x as f32 / 10_000.),
                                        area.size.height * (y as f32 / 10_000.),
                                    );
                                if n == 0 {
                                    outline.move_to(point)
                                } else {
                                    outline.line_to(point)
                                }
                            }
                            outline.close();
                            if let Ok(path) = outline.build() {
                                window.paint_path(path, rgb(0xc94546));
                            }
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .when(ui.view.host.supports_command(&center), |frame| {
                    frame
                        .cursor_crosshair()
                        .on_mouse_down(
                            MouseButton::Left,
                            ui.cx.listener(move |v, e: &MouseDownEvent, window, cx| {
                                v.panels.navigating = true;
                                window.focus(&v.focus, cx);
                                if let Some(cmd) = center_at(e.position, press.get()) {
                                    v.send(EditorCommand::Viewport(cmd), cx);
                                }
                            }),
                        )
                        .on_mouse_move(ui.cx.listener(move |v, e: &MouseMoveEvent, window, cx| {
                            if !v.panels.navigating || e.pressed_button != Some(MouseButton::Left) {
                                return;
                            }
                            let Some(cmd) = center_at(e.position, motion.get()) else {
                                return;
                            };
                            if v.panels.pending_center.replace(cmd).is_none() {
                                let view = cx.weak_entity();
                                window.on_next_frame(move |_, cx| {
                                    let _ = view.update(cx, |v, cx| {
                                        if let Some(cmd) = v.panels.pending_center.take() {
                                            v.send(EditorCommand::Viewport(cmd), cx);
                                        }
                                    });
                                });
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            ui.cx.listener(move |v, e: &MouseUpEvent, _, cx| {
                                if std::mem::take(&mut v.panels.navigating) {
                                    v.panels.pending_center = None;
                                    if let Some(cmd) = center_at(e.position, bounds.get()) {
                                        v.send(EditorCommand::Viewport(cmd), cx);
                                    }
                                }
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            ui.cx.listener(|v, _, _, _| {
                                v.panels.navigating = false;
                                v.panels.pending_center = None;
                            }),
                        )
                })
        });
    let mut controls = row().flex_nowrap().gap_0();
    for (label, command) in [
        ("1:1", ViewportCommand::ActualPixels),
        ("맞춤", ViewportCommand::FitDocument),
        ("−", ViewportCommand::ZoomSteps(-1)),
        ("+", ViewportCommand::ZoomSteps(1)),
        ("좌우 반전", ViewportCommand::ToggleMirrorHorizontal),
    ] {
        if label == "+" {
            controls = controls.child(format!("{}%", p.viewport.zoom_ppm / 10_000));
        }
        controls = controls.child(ui.view.command_button(
            label,
            EditorCommand::Viewport(command),
            true,
            ui.cx,
        ));
    }
    column()
        .gap_1()
        .child(reference_tabs)
        .children(preview)
        .child(controls)
}
