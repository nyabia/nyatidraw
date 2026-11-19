use super::*;
use gpui_kit::assets::IconName;

gpui_kit::assets::icon_assets!(
    ToolIcons,
    [
        Pencil,
        PenTool,
        Paintbrush,
        Eraser,
        Pipette,
        PaintBucket,
        Blend,
        SquareDashed,
        Lasso,
        Wand,
        Scaling,
        Move,
        Hand
    ]
);

const TOOLS: [(DrawingTool, &str); 13] = [
    (DrawingTool::MoveArtwork, "그림 이동"),
    (DrawingTool::Move, "화면 이동"),
    (DrawingTool::MoveSelection, "자유 변형"),
    (DrawingTool::Wand, "마법봉"),
    (DrawingTool::Lasso, "올가미"),
    (DrawingTool::RectangleSelection, "사각형 선택"),
    (DrawingTool::Pencil, "연필"),
    (DrawingTool::Pen, "펜"),
    (DrawingTool::Brush, "브러시"),
    (DrawingTool::Eraser, "지우개"),
    (DrawingTool::Fill, "채우기"),
    (DrawingTool::Gradient, "그라데이션"),
    (DrawingTool::Eyedropper, "스포이트"),
];

pub(super) fn tools(ui: &PanelUi, p: &UiProjection) -> Div {
    column()
        .items_center()
        .gap_1()
        .children(TOOLS.into_iter().map(|(tool, label)| {
            let (icon, help) = match tool {
                DrawingTool::Pencil => (IconName::Pencil, "연필 (B) · 필압으로 연필선 그리기"),
                DrawingTool::Pen => (IconName::PenTool, "펜 (B) · 단단한 선 그리기"),
                DrawingTool::Brush => (IconName::Paintbrush, "브러시 (B) · 부드러운 붓으로 칠하기"),
                DrawingTool::Eraser => (IconName::Eraser, "지우개 (E) · 현재 레이어 지우기"),
                DrawingTool::Eyedropper => (IconName::Pipette, "스포이트 (C) · 그림에서 색 채취"),
                DrawingTool::Fill => (IconName::PaintBucket, "채우기 (F) · 연결된 영역 채우기"),
                DrawingTool::Gradient => (IconName::Blend, "그라데이션 (F) · 끌어서 색 연결"),
                DrawingTool::RectangleSelection => {
                    (IconName::SquareDashed, "사각형 선택 (G) · 끌어서 영역 선택")
                }
                DrawingTool::Lasso => (IconName::Lasso, "올가미 (G) · 영역을 둘러싸서 선택"),
                DrawingTool::Wand => (IconName::Wand, "마법봉 (G) · 색이 연결된 영역 선택"),
                DrawingTool::MoveSelection => (
                    IconName::Scaling,
                    "자유 변형 (G) · 선택 영역 또는 레이어 변형",
                ),
                DrawingTool::MoveArtwork => (IconName::Move, "그림 이동 (G) · 그림 픽셀 이동"),
                DrawingTool::Move => (IconName::Hand, "화면 이동 (G) · 보기 이동"),
            };
            let command = EditorCommand::Tool(ToolCommand::Select(tool));
            let revision = p.revision;
            let key = match tool {
                DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush => "B",
                DrawingTool::Eraser => "E",
                DrawingTool::Fill | DrawingTool::Gradient => "F",
                DrawingTool::Eyedropper => "C",
                _ => "G",
            };
            let button = Button::new(label)
                .compact()
                .small()
                .ghost()
                .selected(p.drawing_tool == tool)
                .toggled(p.drawing_tool == tool)
                .tooltip(help)
                .w(px(38.))
                .h(px(36.))
                .px_0()
                .py_0()
                .accessibility_label(label)
                .disabled(
                    p.edit.busy
                        || p.animation.playing
                        || p.edit
                            .transform
                            .as_ref()
                            .is_some_and(|draft| !draft.can_commit)
                        || p.edit.color_adjustment.is_some()
                        || !ui.view.host.supports_command(&command),
                )
                .on_click(ui.cx.listener(move |view, _, window, cx| {
                    window.focus(&view.focus, cx);
                    view.send_at(revision, command.clone(), cx);
                }));
            match ToolIcons.load(icon.path().as_ref()) {
                Ok(Some(bytes)) => button.child(
                    div()
                        .relative()
                        .flex()
                        .justify_center()
                        .items_center()
                        .w(px(32.))
                        .h(px(30.))
                        .child(svg().data(&bytes).size(px(21.)).text_color(rgb(0xf1f1f1)))
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .right_0()
                                .bg(rgb(0x454545))
                                .text_color(rgb(0xf1f1f1))
                                .text_size(px(10.))
                                .line_height(px(12.))
                                .px(px(2.))
                                .child(key),
                        ),
                ),
                _ => button.label(label),
            }
        }))
}

pub(super) fn sizes(ui: &PanelUi, p: &UiProjection) -> Div {
    let size_button = |size: u16, recent: bool| {
        let value = f32::from(size) / 10.;
        let diameter = (2. + value.sqrt() * 1.7).clamp(3., 20.);
        let text = value.to_string();
        let command = EditorCommand::Tool(ToolCommand::SetSizeTenths(size));
        let revision = p.revision;
        let dot = div()
            .size(px(diameter))
            .flex_shrink_0()
            .rounded_full()
            .bg(rgb(0xe9e9e9));
        let body = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .when(recent, |d| d.flex_col())
            .child(
                div()
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(dot),
            )
            .child(
                div()
                    .text_color(rgb(0xf1f1f1))
                    .text_size(px(11.))
                    .child(text.clone()),
            );
        Button::new(format!("size-{recent}-{size}"))
            .small()
            .compact()
            .ghost()
            .selected(size == p.brush_size_tenths)
            .flex_1()
            .min_w_0()
            .h(px(if recent { 40. } else { 32. }))
            .p_0()
            .border_1()
            .border_color(if size == p.brush_size_tenths {
                rgb(0xdadada)
            } else {
                rgb(0x727272)
            })
            .accessibility_label(format!("{text} px"))
            .tooltip(format!("브러시 크기 {text} px"))
            .disabled(!ui.view.host.supports_command(&command))
            .on_click(ui.cx.listener(move |view, _, window, cx| {
                window.focus(&view.focus, cx);
                view.send_at(revision, command.clone(), cx);
            }))
            .child(body)
    };
    let mut recent = row().flex_nowrap().gap(px(2.)).child(ui.heading("최근"));
    for index in 0..4 {
        recent = if let Some(&size) = p.recent_brush_sizes.get(index) {
            recent.child(size_button(size, true))
        } else {
            recent.child(
                div()
                    .flex_1()
                    .h(px(40.))
                    .border_b_1()
                    .border_color(rgb(0x767676)),
            )
        };
    }
    column().gap_1().child(recent).children(
        [
            [10, 20, 30],
            [50, 80, 100],
            [150, 200, 300],
            [400, 800, 1200],
        ]
        .map(|group| {
            row()
                .flex_nowrap()
                .gap(px(2.))
                .children(group.map(|size| size_button(size, false)))
        }),
    )
}

pub(super) fn presets(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let mut panel = column();
    if p.drawing_tool == DrawingTool::Pencil {
        panel = panel.child(
            row().children(
                [
                    (PencilTemplate::Mechanical2H, "2H 샤프펜슬"),
                    (PencilTemplate::Graphite2B, "2B 연필"),
                ]
                .map(|(template, name)| {
                    ui.button(
                        name,
                        EditorCommand::Tool(ToolCommand::SelectPencilTemplate(template)),
                        p.pencil_template == template,
                        !busy(p),
                    )
                }),
            ),
        );
    }
    if p.drawing_tool == DrawingTool::Fill {
        return panel.child(source_choices(ui, p, true));
    }
    let current = BrushPresetValues::from_projection(p);
    if !current.valid() {
        return panel.child(
            TOOLS
                .iter()
                .find(|(t, _)| *t == p.drawing_tool)
                .map(|(_, name)| *name)
                .unwrap_or_default(),
        );
    }
    if p.drawing_tool != DrawingTool::Pencil {
        let name = match p.drawing_tool {
            DrawingTool::Pen => "기본 펜",
            DrawingTool::Brush => "소프트 브러시",
            DrawingTool::Eraser => "기본 지우개",
            _ => "연필",
        };
        panel = panel.child(
            div()
                .w_full()
                .py_1()
                .px_2()
                .bg(rgb(0x696969))
                .text_color(rgb(0xf1f1f1))
                .child(name),
        );
    }
    let managing_list = ui.view.panels.preset_management;
    let matching = || {
        p.brush_presets
            .iter()
            .filter(|preset| preset.values.matches(current))
    };
    let active_name = matching()
        .find(|preset| Some(preset.name) == ui.view.panels.selected_brush_preset)
        .or_else(|| matching().next())
        .map(|preset| preset.name);
    for (index, preset) in p.brush_presets.iter().enumerate() {
        if preset.values.tool != p.drawing_tool {
            continue;
        }
        let Ok(index) = u8::try_from(index) else {
            break;
        };
        let name = preset.name;
        let managing = ui.view.panels.managed_preset == Some(name);
        let command = EditorCommand::Tool(ToolCommand::ApplyBrushPreset(index));
        let revision = p.revision;
        panel = panel.child(
            row()
                .id(("brush-preset", usize::from(index)))
                .child(
                    Button::new(name.as_str().to_owned())
                        .label(name.as_str().to_owned())
                        .small()
                        .compact()
                        .ghost()
                        .selected(active_name == Some(name))
                        .disabled(busy(p) || !ui.view.host.supports_command(&command))
                        .on_click(ui.cx.listener(move |v, _, window, cx| {
                            window.focus(&v.focus, cx);
                            v.send_at(revision, command.clone(), cx);
                            if v.notice.is_none() {
                                v.panels.selected_brush_preset = Some(name);
                            }
                        })),
                )
                .when(managing_list, |d| {
                    d.child(
                        Button::new("manage-preset")
                            .compact()
                            .small()
                            .ghost()
                            .selected(managing)
                            .disabled(!p.brush_presets_writable || busy(p))
                            .label("관리")
                            .on_click(ui.cx.listener(move |v, _, _, cx| {
                                v.panels.managed_preset = (!managing).then_some(name);
                                v.panels.confirm_preset = None;
                                cx.notify();
                            })),
                    )
                }),
        );
        if managing && managing_list {
            let enabled = p.brush_presets_writable && !busy(p);
            panel = panel.child(ui.field(
                "프리셋 이름 변경 · Enter",
                name.as_str().to_owned(),
                name.as_str().to_owned(),
                enabled,
                move |v, text, cx| {
                    let new_name =
                        BrushPresetName::new(text).ok_or("새 프리셋 이름을 확인하세요.")?;
                    let p = v.host.snapshot();
                    let index = p
                        .brush_presets
                        .iter()
                        .position(|p| p.name == name)
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or("이 프리셋이 변경되었습니다.")?;
                    v.send_at(
                        p.revision,
                        EditorCommand::Tool(ToolCommand::RenameBrushPreset(index, new_name)),
                        cx,
                    );
                    if v.notice.is_none() {
                        if v.panels.selected_brush_preset == Some(name) {
                            v.panels.selected_brush_preset = Some(new_name);
                        }
                        v.panels.managed_preset = Some(new_name);
                    }
                    Ok(())
                },
            ));
            if let Some((target, deleting)) =
                ui.view.panels.confirm_preset.filter(|(t, _)| *t == name)
            {
                let command = if deleting {
                    ToolCommand::DeleteBrushPreset(index)
                } else {
                    ToolCommand::UpdateBrushPreset(index)
                };
                panel = panel.child(
                    row()
                        .child(ui.heading(format!(
                            "{} {}",
                            target.as_str(),
                            if deleting {
                                "삭제할까요?"
                            } else {
                                "현재 설정으로 덮어쓸까요?"
                            }
                        )))
                        .child(
                            Button::new("confirm-preset")
                                .small()
                                .compact()
                                .label("확인")
                                .disabled(
                                    !enabled
                                        || !ui
                                            .view
                                            .host
                                            .supports_command(&EditorCommand::Tool(command)),
                                )
                                .on_click(ui.cx.listener(move |v, _, window, cx| {
                                    let p = v.host.snapshot();
                                    if let Some(i) = p
                                        .brush_presets
                                        .iter()
                                        .position(|preset| {
                                            preset.name == target
                                                && preset.values.tool == p.drawing_tool
                                        })
                                        .and_then(|n| u8::try_from(n).ok())
                                    {
                                        v.send_at(
                                            p.revision,
                                            EditorCommand::Tool(if deleting {
                                                ToolCommand::DeleteBrushPreset(i)
                                            } else {
                                                ToolCommand::UpdateBrushPreset(i)
                                            }),
                                            cx,
                                        );
                                    }
                                    v.panels.confirm_preset = None;
                                    window.focus(&v.focus, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("cancel-preset")
                                .small()
                                .compact()
                                .label("취소")
                                .on_click(ui.cx.listener(|v, _, _, cx| {
                                    v.panels.confirm_preset = None;
                                    cx.notify();
                                })),
                        ),
                );
            } else {
                panel = panel.child(row().children(
                    [("현재 설정으로 덮어쓰기", false), ("프리셋 삭제", true)].map(
                        |(label, deleting)| {
                            Button::new(label)
                                .small()
                                .compact()
                                .ghost()
                                .label(label)
                                .disabled(!enabled)
                                .on_click(ui.cx.listener(move |v, _, _, cx| {
                                    v.panels.confirm_preset = Some((name, deleting));
                                    cx.notify();
                                }))
                        },
                    ),
                ));
            }
        }
    }
    let enabled = p.brush_presets_writable
        && !busy(p)
        && p.brush_presets.len() < BRUSH_PRESETS_MAX
        && ui
            .view
            .host
            .supports_command(&EditorCommand::Tool(ToolCommand::SaveBrushPreset(
                BrushPresetName::new("preset").unwrap(),
            )));
    panel = panel.child(
        Button::new("toggle-preset-management")
            .compact()
            .small()
            .ghost()
            .label("프리셋 관리")
            .selected(managing_list)
            .on_click(ui.cx.listener(|v, _, _, cx| {
                v.panels.preset_management = !v.panels.preset_management;
                cx.notify();
            })),
    );
    if !managing_list {
        return panel;
    }
    panel.child(ui.field(
        "새 프리셋 이름 · Enter로 저장",
        String::new(),
        String::new(),
        enabled,
        |v, text, cx| {
            let name =
                BrushPresetName::new(text).ok_or("프리셋 이름을 확인하세요 (최대 96바이트).")?;
            v.send(EditorCommand::Tool(ToolCommand::SaveBrushPreset(name)), cx);
            Ok(())
        },
    ))
}

pub(super) fn properties(ui: &mut PanelUi, p: &UiProjection) -> Div {
    if let Some(adjustment) = &p.edit.color_adjustment {
        return correction(ui, p, adjustment);
    }
    if let Some(transform) = &p.edit.transform {
        return transform_controls(ui, p, transform);
    }
    let mut panel = match p.drawing_tool {
        DrawingTool::Move => column().child("끌어서 화면 이동"),
        DrawingTool::MoveSelection => column().child(ui.button(
            "자유 변형 시작",
            EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::Begin)),
            false,
            !busy(p),
        )),
        DrawingTool::MoveArtwork => column().child(row().children(
            [("가로 중앙", true, false), ("세로 중앙", false, true)].map(|(name, h, v)| {
                ui.button(
                    name,
                    EditorCommand::Edit(EditCommand::CenterArtwork {
                        horizontal: h,
                        vertical: v,
                    }),
                    false,
                    !busy(p),
                )
            }),
        )),
        DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush | DrawingTool::Eraser => {
            return brush_controls(ui, p);
        }
        DrawingTool::Wand | DrawingTool::Eyedropper => column().child(source_choices(ui, p, false)),
        DrawingTool::Fill => fill_controls(ui, p),
        DrawingTool::Gradient => column().child(ui.heading("그라데이션 끝 색")).child(
            row().children(
                [
                    (GradientMode::ForegroundToTransparent, "투명"),
                    (GradientMode::ForegroundToBackground, "배경색"),
                ]
                .map(|(mode, label)| {
                    ui.button(
                        label,
                        EditorCommand::Tool(ToolCommand::SetGradientMode(mode)),
                        p.edit_settings.gradient_mode == mode,
                        true,
                    )
                }),
            ),
        ),
        DrawingTool::Lasso | DrawingTool::RectangleSelection => column(),
    };
    if p.drawing_tool == DrawingTool::Wand {
        panel = panel.child(ui.slider(
            "마법봉 허용 오차",
            p.edit_settings.tolerance.into(),
            0..=255,
            p.edit_settings.tolerance.to_string(),
            true,
            |n| EditorCommand::Tool(ToolCommand::SetEditTolerance(n as u8)),
        ));
    }
    if matches!(
        p.drawing_tool,
        DrawingTool::Wand | DrawingTool::Lasso | DrawingTool::RectangleSelection
    ) {
        panel = panel
            .child(ui.heading("선택 조합"))
            .child(
                row().children(
                    [
                        (SelectionMode::Replace, "새로 선택"),
                        (SelectionMode::Add, "추가"),
                        (SelectionMode::Subtract, "빼기"),
                    ]
                    .map(|(mode, label)| {
                        ui.button(
                            label,
                            EditorCommand::Tool(ToolCommand::SetSelectionMode(mode)),
                            p.edit_settings.selection_mode == mode,
                            true,
                        )
                    }),
                ),
            )
            .child(selection(ui, p));
    }
    panel
}

pub(super) fn percent(v: u16) -> i32 {
    ((u32::from(v) * 100 + 32767) / 65535) as i32
}
pub(super) fn ratio(p: i32) -> u16 {
    ((p.clamp(0, 100) * 65535 + 50) / 100) as u16
}

fn brush_controls(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let b = p.brush_settings;
    let size = p.brush_size_tenths;
    let target = NumberTarget::Brush(p.drawing_tool);
    let values = BrushPresetValues::from_projection(p);
    if ui
        .view
        .panels
        .brush_sample
        .as_ref()
        .is_none_or(|(prev, _)| *prev != values)
    {
        ui.view.panels.brush_sample = Some((values, ui.view.host.brush_preview(p)));
    }
    let preview = ui
        .view
        .panels
        .brush_sample
        .as_ref()
        .and_then(|(_, result)| result.as_ref().ok())
        .cloned()
        .and_then(|uri| ui.thumbnail(Thumbnail::Brush, &uri));
    let mut panel = column()
        .when_some(preview, |d, image| {
            d.child(
                div()
                    .w_full()
                    .h(px(48.))
                    .bg(rgb(0xf4f4f4))
                    .child(image.size_full()),
            )
        })
        .child(ui.number_slider(
            "크기",
            target,
            size.into(),
            NumberScale::BrushSize,
            true,
            |n| EditorCommand::Tool(ToolCommand::SetSizeTenths(n as u16)),
        ))
        .child(ui.number_slider(
            "불투명도",
            target,
            percent(p.brush_opacity_u16),
            NumberScale::Percent { min: 1 },
            true,
            |n| EditorCommand::Tool(ToolCommand::SetOpacityU16(ratio(n))),
        ))
        .child(ui.number_slider(
            "손떨림 보정",
            target,
            b.smoothing.into(),
            NumberScale::Strength,
            true,
            |n| EditorCommand::Tool(ToolCommand::SetSmoothing(n as u8)),
        ));
    let open = ui.view.panels.brush_options_open;
    panel = panel.child(
        Button::new("brush-extra-options")
            .small()
            .compact()
            .ghost()
            .accessibility_label("추가 옵션")
            .child(
                row()
                    .flex_nowrap()
                    .child(icon(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }))
                    .child("추가 옵션"),
            )
            .on_click(ui.cx.listener(move |v, _, window, cx| {
                v.panels.brush_options_open = !open;
                window.focus(&v.focus, cx);
                cx.notify();
            })),
    );
    if !open {
        return panel;
    }
    if p.drawing_tool != DrawingTool::Pencil {
        panel = panel.child(ui.number_slider(
            "경도",
            target,
            percent(b.hardness_u16),
            NumberScale::Percent { min: 0 },
            true,
            |n| EditorCommand::Tool(ToolCommand::SetHardnessU16(ratio(n))),
        ));
    }
    panel
        .child(
            row()
                .child(ui.button(
                    "크기 필압",
                    EditorCommand::Tool(ToolCommand::SetSizePressure(!b.size_pressure)),
                    b.size_pressure,
                    true,
                ))
                .child(ui.button(
                    "농도 필압",
                    EditorCommand::Tool(ToolCommand::SetOpacityPressure(!b.opacity_pressure)),
                    b.opacity_pressure,
                    true,
                )),
        )
        .child(ui.number_slider(
            "최소 크기",
            target,
            percent(b.size_minimum_u16),
            NumberScale::Percent { min: 0 },
            b.size_pressure,
            |n| EditorCommand::Tool(ToolCommand::SetSizeMinimumU16(ratio(n))),
        ))
        .child(ui.number_slider(
            "최소 농도",
            target,
            percent(b.opacity_minimum_u16),
            NumberScale::Percent { min: 0 },
            b.opacity_pressure,
            |n| EditorCommand::Tool(ToolCommand::SetOpacityMinimumU16(ratio(n))),
        ))
}

fn source_choices(ui: &PanelUi, p: &UiProjection, fill: bool) -> Div {
    let current = if fill {
        p.edit_settings.fill_source
    } else {
        p.edit_settings.source
    };
    column().child(ui.heading("참조 범위")).child(
        row().children(
            [
                (EditSource::ActiveLayer, "현재 레이어"),
                (EditSource::ReferenceLayers, "참조 레이어"),
                (EditSource::AllVisible, "보이는 레이어"),
            ]
            .map(|(source, label)| {
                ui.button(
                    label,
                    EditorCommand::Tool(if fill {
                        ToolCommand::SetFillSource(source)
                    } else {
                        ToolCommand::SetEditSource(source)
                    }),
                    source == current,
                    true,
                )
            }),
        ),
    )
}

fn fill_controls(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let edit = p.edit_settings;
    let color = nyatidraw_tiles::color::srgb8_to_linear_premultiplied(p.brush_color);
    let target = p.raster_target;
    let mask_selected = matches!(target, Some(RasterEditTarget::Mask { .. }));
    let target_layer = target.map(RasterEditTarget::layer).or(p.active_layer);
    let layer = target_layer.and_then(|id| {
        p.layers
            .iter()
            .find(|layer| layer.id == LayerTreeNodeId::Raster(id))
    });
    let hidden = layer.is_some_and(|layer| layer_path(&p.layers, layer).any(|node| !node.visible));
    let blocked = busy(p) || hidden || layer.is_none_or(|layer| layer.locked);
    let alpha_lock = !mask_selected && layer.is_some_and(|layer| layer.alpha_locked);
    let mut panel = column();
    if let Some(layer) = layer {
        panel = panel.child(ui.heading(format!(
            "채우기 대상 · {} ({})",
            layer.name,
            if mask_selected {
                "마스크"
            } else {
                "이미지"
            },
        )));
        if let (Some(id), Some(mask)) = (target_layer, &layer.mask) {
            panel = panel.child(
                row()
                    .child(
                        layers::target_button(ui, p, layer, RasterEditTarget::Image(id))
                            .label("이미지"),
                    )
                    .child(
                        layers::target_button(
                            ui,
                            p,
                            layer,
                            RasterEditTarget::Mask {
                                layer: id,
                                mask: mask.id,
                            },
                        )
                        .label("마스크"),
                    ),
            );
        }
    } else {
        panel = panel.child(ui.heading("채울 레이어를 선택하세요."));
    }
    if let Some(message) = fill_notice(p, layer, hidden, mask_selected, alpha_lock) {
        panel = panel.child(div().text_color(rgb(0xe4c896)).child(message));
    }
    if p.edit.has_selection {
        panel = panel.child("선택 영역 안에서만 채웁니다.");
    }
    if mask_selected {
        panel =
            panel.child("마스크는 선택 영역 채우기를 사용합니다. 검정은 숨김, 흰색은 표시입니다.");
    } else {
        panel = panel
            .child(source_choices(ui, p, true))
            .child(ui.slider(
                "채우기 허용 오차",
                edit.fill_tolerance.into(),
                0..=255,
                edit.fill_tolerance.to_string(),
                true,
                |n| EditorCommand::Tool(ToolCommand::SetFillTolerance(n as u8)),
            ))
            .child(ui.slider(
                "틈 닫기",
                edit.fill.gap_close_px.into(),
                0..=8,
                format!("{} px", edit.fill.gap_close_px),
                true,
                |n| EditorCommand::Tool(ToolCommand::SetFillGapClose(n as u8)),
            ))
            .child(ui.slider(
                "선 밑으로 확장",
                edit.fill.expand_px.into(),
                0..=64,
                format!("{} px", edit.fill.expand_px),
                !alpha_lock,
                |n| EditorCommand::Tool(ToolCommand::SetFillExpansion(n as u8)),
            ))
            .child(ui.button(
                "최소 1px 빈틈 보정",
                EditorCommand::Tool(ToolCommand::SetFillAntialias(!edit.fill.antialias)),
                edit.fill.antialias,
                !alpha_lock,
            ));
    }
    panel.child(
        row()
            .child(ui.button(
                if mask_selected {
                    "선택 마스크에 명도 채우기"
                } else {
                    "선택 전체 채우기"
                },
                EditorCommand::Edit(EditCommand::FillSelection { color }),
                false,
                p.edit.has_selection && !blocked,
            ))
            .child(ui.button(
                "선택 해제",
                EditorCommand::Edit(EditCommand::ClearSelection),
                false,
                p.edit.has_selection && !busy(p),
            )),
    )
}

fn fill_notice(
    p: &UiProjection,
    layer: Option<&LayerProjection>,
    hidden: bool,
    mask_selected: bool,
    alpha_locked: bool,
) -> Option<&'static str> {
    if layer.is_some_and(|layer| layer.locked) {
        Some("칠할 레이어의 잠금을 해제하세요.")
    } else if hidden {
        Some("칠할 레이어나 상위 그룹의 표시를 켜주세요.")
    } else if layer
        .is_some_and(|layer| layer_path(&p.layers, layer).any(|node| node.opacity_u16 == 0))
    {
        Some("레이어나 상위 그룹의 불투명도가 0%여서 칠한 색이 보이지 않습니다.")
    } else if !mask_selected
        && p.edit_settings.fill_source == EditSource::ReferenceLayers
        && !p.layers.iter().any(|layer| {
            layer.reference
                && layer_path(&p.layers, layer)
                    .all(|node| has_visible_alpha_source(&p.layers, node, p.layers.len()))
        })
    {
        Some(
            "보이는 선화에 참조 표시를 켜주세요. 클리핑된 선화는 아래 기준 레이어도 표시되어야 합니다.",
        )
    } else if alpha_locked {
        Some("투명도 잠금 중입니다. 이미 칠한 영역만 다시 칠할 수 있습니다.")
    } else {
        None
    }
}

fn layer_path<'a>(
    layers: &'a [LayerProjection],
    layer: &'a LayerProjection,
) -> impl Iterator<Item = &'a LayerProjection> {
    std::iter::successors(Some(layer), |child| {
        layers
            .iter()
            .find(|candidate| candidate.id == LayerTreeNodeId::Group(child.parent))
    })
    .take(layers.len())
}

fn has_visible_alpha_source(
    layers: &[LayerProjection],
    layer: &LayerProjection,
    remaining: usize,
) -> bool {
    if remaining == 0 || !layer.visible || layer.opacity_u16 == 0 {
        return false;
    }
    if layer.clip_to_below {
        // Bind before filtering visibility, matching the original sibling stack.
        let base = layers
            .iter()
            .filter(|candidate| {
                candidate.parent == layer.parent
                    && candidate.index < layer.index
                    && !candidate.clip_to_below
            })
            .max_by_key(|candidate| candidate.index);
        if base.is_none_or(|base| !has_visible_alpha_source(layers, base, remaining - 1)) {
            return false;
        }
    }
    match layer.id {
        LayerTreeNodeId::Raster(_) => true,
        LayerTreeNodeId::Group(group) => layers.iter().any(|child| {
            child.parent == group
                && !child.clip_to_below
                && has_visible_alpha_source(layers, child, remaining - 1)
        }),
    }
}

fn selection(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let buttons = [
        ("전체 선택", EditCommand::SelectAll, false),
        ("반전", EditCommand::InvertSelection, false),
        ("해제", EditCommand::ClearSelection, true),
        ("그림에서 선택", EditCommand::SelectLayerAlpha, false),
        ("선택 픽셀 삭제", EditCommand::DeleteSelectedPixels, true),
        ("복사", EditCommand::CopySelection, true),
        ("오리기", EditCommand::CutSelection, true),
        ("붙여넣기", EditCommand::PasteSelection, false),
    ];
    let radius = ui.view.panels.selection_radius.unwrap_or(1);
    column()
        .child(row().children(buttons.map(|(label, cmd, needs)| {
            ui.button(
                label,
                EditorCommand::Edit(cmd),
                false,
                !busy(p) && (!needs || p.edit.has_selection),
            )
        })))
        .child(ui.field(
            "선택 경계 반경 (px) · Enter",
            String::new(),
            radius.to_string(),
            true,
            |v, t, _| {
                v.panels.selection_radius = Some(
                    t.parse::<u8>()
                        .ok()
                        .filter(|n| (1..=64).contains(n))
                        .ok_or("반경은 1~64 px입니다.")?,
                );
                Ok(())
            },
        ))
        .child(
            row().children(
                [
                    ("확장", EditCommand::GrowSelection { radius }),
                    ("축소", EditCommand::ShrinkSelection { radius }),
                    ("페더", EditCommand::FeatherSelection { radius }),
                ]
                .map(|(label, cmd)| {
                    ui.button(
                        label,
                        EditorCommand::Edit(cmd),
                        false,
                        p.edit.has_selection && !busy(p),
                    )
                }),
            ),
        )
}

fn transform_controls(ui: &mut PanelUi, p: &UiProjection, t: &TransformProjection) -> Div {
    let mut panel = column();
    let value = t.transform;
    for (i, (label, n, scale)) in [
        ("가로 이동 (px)", value.offset_milli[0] as f64, 1000.),
        ("세로 이동 (px)", value.offset_milli[1] as f64, 1000.),
        ("너비 (%)", f64::from(value.scale_ppm[0]), 10000.),
        ("높이 (%)", f64::from(value.scale_ppm[1]), 10000.),
        ("회전 (°)", f64::from(value.rotation_millidegrees), 1000.),
    ]
    .into_iter()
    .enumerate()
    {
        panel = panel.child(ui.field(
            label,
            format!("{:?}", p.active_layer),
            (n / scale).to_string(),
            !p.edit.busy,
            move |v, text, cx| {
                let p = v.host.snapshot();
                let mut current = p
                    .edit
                    .transform
                    .as_ref()
                    .ok_or("변형이 종료되었습니다.")?
                    .transform;
                let x = (text
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "숫자를 입력하세요.")?
                    * scale)
                    .round();
                let valid = match i {
                    0 | 1 => (-2_147_483_648_000. ..=2_147_483_647_000.).contains(&x),
                    2 | 3 => (1. ..=f64::from(u32::MAX)).contains(&x),
                    _ => (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&x),
                };
                if !valid {
                    return Err("변형 값의 범위를 확인하세요.");
                }
                match i {
                    0 | 1 => current.offset_milli[i] = x as i64,
                    2 | 3 => current.scale_ppm[i - 2] = x as u32,
                    _ => current.rotation_millidegrees = x as i32,
                }
                v.send(
                    EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::Preview(
                        current,
                    ))),
                    cx,
                );
                Ok(())
            },
        ));
    }
    panel.child(
        row().children(
            [
                (
                    "좌우 반전",
                    TransformCommand::Preview(AffineTransform {
                        flip_x: !value.flip_x,
                        ..value
                    }),
                    value.flip_x,
                    !p.edit.busy,
                ),
                (
                    "상하 반전",
                    TransformCommand::Preview(AffineTransform {
                        flip_y: !value.flip_y,
                        ..value
                    }),
                    value.flip_y,
                    !p.edit.busy,
                ),
                (
                    "변형 적용",
                    TransformCommand::Commit {
                        generation: t.generation,
                    },
                    false,
                    t.can_commit && !p.edit.busy,
                ),
                (
                    "변형 취소",
                    TransformCommand::Cancel,
                    false,
                    p.edit.can_cancel,
                ),
            ]
            .map(|(label, cmd, on, enabled)| {
                ui.button(
                    label,
                    EditorCommand::Edit(EditCommand::FreeTransform(cmd)),
                    on,
                    enabled,
                )
            }),
        ),
    )
}

fn correction(ui: &mut PanelUi, p: &UiProjection, a: &ColorAdjustmentProjection) -> Div {
    let pending = &mut ui.view.panels.pending_correction;
    if pending.is_some_and(|(target, values)| {
        target != a.target || values == a.adjustment || p.edit.error.is_some()
    }) {
        *pending = None;
    }
    let values = pending.map_or(a.adjustment, |(_, values)| values);
    let target = a.target;
    let mut panel = column();
    if let Some(layer) = p
        .layers
        .iter()
        .find(|layer| layer.id == LayerTreeNodeId::Raster(target))
    {
        panel = panel.child(ui.heading(format!("색 조정 대상 · {}", layer.name)));
    }
    panel = panel.child(
        row()
            .child(
                Button::new("reset-color-adjustment")
                    .small()
                    .compact()
                    .ghost()
                    .label("초기화")
                    .disabled(!p.edit.can_cancel)
                    .on_click(ui.cx.listener(move |v, _, window, cx| {
                        window.focus(&v.focus, cx);
                        v.panels.fields.retain(|_, field| !field.preview);
                        v.panels.input_error = None;
                        cx.notify();
                        let _ = preview_correction(
                            v,
                            target,
                            |values| *values = ColorAdjustment::default(),
                            cx,
                        );
                    })),
            )
            .child(
                ui.button(
                    "색 조정 적용",
                    EditorCommand::Edit(EditCommand::ColorAdjustment(
                        ColorAdjustmentCommand::Commit {
                            generation: a.generation,
                        },
                    )),
                    false,
                    a.can_commit && !p.edit.busy && ui.view.panels.pending_correction.is_none(),
                )
                .label("적용")
                .accessibility_label("색 조정 적용"),
            )
            .child(
                ui.button(
                    "색 조정 취소",
                    EditorCommand::Edit(EditCommand::ColorAdjustment(
                        ColorAdjustmentCommand::Cancel,
                    )),
                    false,
                    p.edit.can_cancel,
                )
                .label("취소")
                .accessibility_label("색 조정 취소"),
            ),
    );
    for (label, value, max, channel) in [
        ("색상", values.hue_degrees, 180, CorrectionChannel::Hue),
        (
            "채도",
            values.saturation_percent,
            100,
            CorrectionChannel::Saturation,
        ),
        ("명도", values.value_percent, 100, CorrectionChannel::Value),
        (
            "밝기",
            values.brightness_percent,
            100,
            CorrectionChannel::Brightness,
        ),
        (
            "대비",
            values.contrast_percent,
            100,
            CorrectionChannel::Contrast,
        ),
    ] {
        let input = ui.field_input(
            label,
            format!("{target:?}"),
            value.to_string(),
            p.edit.can_cancel,
            false,
            move |v, text, cx| {
                let number = text
                    .trim()
                    .parse::<i16>()
                    .ok()
                    .filter(|n| (-max..=max).contains(n))
                    .ok_or("색 조정 값의 범위를 확인하세요.")?;
                preview_correction(v, target, |values| channel.set(values, number), cx)
            },
        );
        panel = panel.child(ui.bound_slider(
            label,
            i32::from(value),
            -i32::from(max)..=i32::from(max),
            input.w(px(52.)).text_right(),
            p.edit.can_cancel,
            EditorCommand::Edit(EditCommand::ColorAdjustment(
                ColorAdjustmentCommand::BeginLayer(target),
            )),
            move |v, value, _, _, cx| {
                let _ =
                    preview_correction(v, target, |values| channel.set(values, value as i16), cx);
            },
        ));
    }
    panel
}

#[derive(Clone, Copy)]
enum CorrectionChannel {
    Hue,
    Saturation,
    Value,
    Brightness,
    Contrast,
}

impl CorrectionChannel {
    fn set(self, values: &mut ColorAdjustment, value: i16) {
        *match self {
            Self::Hue => &mut values.hue_degrees,
            Self::Saturation => &mut values.saturation_percent,
            Self::Value => &mut values.value_percent,
            Self::Brightness => &mut values.brightness_percent,
            Self::Contrast => &mut values.contrast_percent,
        } = value;
    }
}

fn preview_correction(
    view: &mut EditorView,
    target: LayerId,
    update: impl FnOnce(&mut ColorAdjustment),
    cx: &mut Context<EditorView>,
) -> Result<(), &'static str> {
    let projection = view.host.snapshot();
    let draft = projection
        .edit
        .color_adjustment
        .filter(|draft| draft.target == target)
        .ok_or("색 조정이 종료되었습니다.")?;
    let pending = view
        .panels
        .pending_correction
        .filter(|(layer, _)| *layer == target);
    let mut values = pending.map_or(draft.adjustment, |(_, values)| values);
    let previous = values;
    update(&mut values);
    if values == previous && (pending.is_some() || draft.can_commit) {
        return Ok(());
    }
    view.send_at(
        projection.revision,
        EditorCommand::Edit(EditCommand::ColorAdjustment(
            ColorAdjustmentCommand::Preview(values),
        )),
        cx,
    );
    if view.notice.is_none() {
        view.panels.pending_correction = Some((target, values));
    }
    Ok(())
}
