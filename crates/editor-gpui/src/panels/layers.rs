use super::*;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};

gpui_kit::assets::icon_assets!(LayerIcons, [Eye, EyeOff, Folder]);

fn layer_icon(name: IconName) -> Svg {
    let data = LayerIcons.load(name.path().as_ref()).ok().flatten();
    svg()
        .when_some(data, |svg, bytes| svg.data(&bytes))
        .size(px(16.))
        .text_color(rgb(0xf1f1f1))
}

// The row has its own hit target. Child controls, including disabled controls,
// consume presses before the row starts tracking a click.
fn row_control(id: &'static str, control: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .child(control)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| cx.stop_propagation())
}

fn select_row(
    v: &mut EditorView,
    id: LayerTreeNodeId,
    active_raster: bool,
    revision: Revision,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    window.focus(&v.focus, cx);
    match id {
        LayerTreeNodeId::Raster(layer) if !active_raster => {
            v.send_at(
                revision,
                EditorCommand::Layer(LayerCommand::SetActive(layer)),
                cx,
            );
            if v.notice.is_none() {
                v.panels.group = None;
            }
        }
        LayerTreeNodeId::Raster(_) => {
            if v.panels.group.take().is_some() {
                cx.notify();
            }
        }
        LayerTreeNodeId::Group(group) => {
            if v.panels.group != Some(group) {
                v.panels.group = Some(group);
                cx.notify();
            }
        }
    }
}

pub(super) fn target_button(
    ui: &PanelUi,
    p: &UiProjection,
    layer: &LayerProjection,
    target: RasterEditTarget,
) -> Button {
    let (label, command) = match target {
        RasterEditTarget::Image(id) => (
            "이미지 편집",
            if layer.mask.is_some() {
                EditorCommand::Mask(MaskCommand::SelectImage(id))
            } else {
                EditorCommand::Layer(LayerCommand::SetActive(id))
            },
        ),
        RasterEditTarget::Mask { layer: id, .. } => (
            "마스크 편집",
            EditorCommand::Mask(MaskCommand::SelectMask(id)),
        ),
    };
    let active = p.raster_target == Some(target);
    Button::new(label)
        .small()
        .compact()
        .ghost()
        .accessibility_label(format!("{} {label}", layer.name))
        .tooltip(label)
        .toggled(active)
        .disabled(
            busy(p)
                || (matches!(target, RasterEditTarget::Mask { .. }) && layer.locked)
                || !ui.view.host.supports_command(&command),
        )
        .on_click(ui.cx.listener({
            let revision = p.revision;
            move |v, _, window, cx| {
                cx.stop_propagation();
                window.focus(&v.focus, cx);
                if !active {
                    v.send_at(revision, command.clone(), cx);
                    if v.notice.is_some() {
                        return;
                    }
                }
                if v.panels.group.take().is_some() {
                    cx.notify();
                }
            }
        }))
}

fn edit_target(
    ui: &PanelUi,
    p: &UiProjection,
    layer: &LayerProjection,
    target: RasterEditTarget,
    image: Option<Img>,
) -> Stateful<Div> {
    let mask = matches!(target, RasterEditTarget::Mask { .. });
    let active = p.raster_target == Some(target);
    let backdrop = div().relative().size_full();
    let backdrop = if mask {
        backdrop.bg(black())
    } else {
        backdrop.child(checkerboard())
    };
    let button = target_button(ui, p, layer, target)
        .w(px(if mask { 36. } else { 50. }))
        .h(px(50.))
        .p_0()
        .child(
            div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(backdrop.when_some(image, |b, img| {
                    b.child(img.absolute().inset_0().size_full())
                }))
                .when(mask, |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .bg(rgba(0x202020dd))
                            .text_color(white())
                            .text_size(px(10.))
                            .line_height(px(13.))
                            .text_center()
                            .child("마스크"),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(rgb(0x363636))
                        .when(active, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset(px(1.))
                                    .border_2()
                                    .border_color(rgb(0xf5f5f5)),
                            )
                        }),
                ),
        );
    row_control(
        if mask {
            "mask-control"
        } else {
            "image-control"
        },
        button,
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum DropPosition {
    Above,
    Inside,
    Below,
}

#[derive(Clone)]
struct LayerDrag {
    node: LayerTreeNodeId,
    revision: Revision,
    label: String,
}

impl Render for LayerDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(cx.theme().background)
            .px_2()
            .py_1()
            .child(self.label.clone())
    }
}

fn reorder(
    p: &UiProjection,
    source: LayerTreeNodeId,
    target: LayerTreeNodeId,
    position: DropPosition,
) -> Option<LayerCommand> {
    if source == target {
        return None;
    }
    let moving = p.layers.iter().find(|l| l.id == source)?;
    let destination = p.layers.iter().find(|l| l.id == target)?;
    let (parent, mut index) = match position {
        DropPosition::Above => (destination.parent, destination.index + 1),
        DropPosition::Below => (destination.parent, destination.index),
        DropPosition::Inside => {
            let LayerTreeNodeId::Group(group) = target else {
                return None;
            };
            (group, p.layers.iter().filter(|l| l.parent == group).count())
        }
    };
    let mut ancestor = parent;
    while let Some(node) = p
        .layers
        .iter()
        .find(|n| n.id == LayerTreeNodeId::Group(ancestor))
    {
        if node.id == source {
            return None;
        }
        ancestor = node.parent;
    }
    if moving.parent == parent {
        if moving.index < index {
            index -= 1;
        }
        if moving.index == index {
            return None;
        }
    }
    Some(LayerCommand::Reorder {
        node: source,
        new_parent: parent,
        index,
    })
}

pub(super) fn layers(ui: &mut PanelUi, p: &UiProjection) -> Div {
    let enabled = !busy(p);
    if ui.view.panels.last_active_layer != p.active_layer {
        ui.view.panels.last_active_layer = p.active_layer;
        ui.view.panels.group = None;
    }
    let thumbnails = ui.view.host.layer_thumbnails();
    let mut live_targets = std::collections::BTreeSet::new();
    for layer in &p.layers {
        if let LayerTreeNodeId::Raster(id) = layer.id {
            live_targets.insert(Thumbnail::Layer(id));
            if let Some(mask) = layer.mask {
                live_targets.insert(Thumbnail::Mask(mask.id));
            }
        }
    }
    ui.keep_thumbnails(|key| {
        matches!(key, Thumbnail::Frame(_) | Thumbnail::Brush) || live_targets.contains(&key)
    });
    let selected = selected_layer(ui.view, p);
    let mut panel = column().child(
        row().children(
            [
                ("레이어 추가", LayerCommand::AddRaster),
                ("그룹 추가", LayerCommand::AddGroup),
                ("흰 배경", LayerCommand::AddWhiteBackground),
            ]
            .map(|(label, cmd)| ui.button(label, EditorCommand::Layer(cmd), false, enabled)),
        ),
    );
    let mut hidden_below = None;
    let based_on = p.revision;
    if let Some(layer) = p.layers.iter().find(|l| Some(l.id) == selected) {
        panel = panel.child(style_controls(ui, p, layer));
    }
    for layer in &p.layers {
        if hidden_below.is_some_and(|depth| layer.depth > depth) {
            continue;
        }
        hidden_below = None;
        let id = layer.id;
        let name = layer.name.clone();
        let can_select = match id {
            LayerTreeNodeId::Raster(r) => {
                enabled
                    && ui
                        .view
                        .host
                        .supports_command(&EditorCommand::Layer(LayerCommand::SetActive(r)))
            }
            LayerTreeNodeId::Group(_) => true,
        };
        let active_raster = matches!(id, LayerTreeNodeId::Raster(r) if p.active_layer == Some(r));
        let selected_group = matches!(id, LayerTreeNodeId::Group(_)) && selected == Some(id);
        let select = Button::new("layer-name")
            .small()
            .compact()
            .ghost()
            .min_w_0()
            .max_w_full()
            .p_0()
            .accessibility_label(name.clone())
            .toggled(selected == Some(id))
            .disabled(!can_select)
            .child(div().min_w_0().truncate().child(name))
            .on_click(ui.cx.listener(move |v, _, window, cx| {
                cx.stop_propagation();
                select_row(v, id, active_raster, based_on, window, cx);
            }));
        let mut line = row()
            .id(format!("layer-{id:?}"))
            .w_full()
            .min_w_0()
            .flex_nowrap()
            .gap(px(2.))
            .pr(px(2.))
            .py(px(3.))
            .pl(px(2. + f32::from(layer.depth.saturating_sub(1)) * 9.))
            .min_h(px(if matches!(id, LayerTreeNodeId::Group(_)) {
                58.
            } else {
                66.
            }))
            .border_1()
            .border_color(rgb(0x555555))
            .when(active_raster, |d| {
                d.bg(rgb(0x666666)).border_color(rgb(0xdddddd))
            })
            .when(selected_group, |d| {
                d.bg(rgb(0x5b6368)).border_color(rgb(0xaec6d2))
            })
            .when(!active_raster && !selected_group, |d| {
                d.hover(|s| s.bg(rgb(0x797979)))
            })
            .when(can_select, |d| {
                d.cursor_pointer()
                    .on_click(ui.cx.listener(move |v, _, window, cx| {
                        select_row(v, id, active_raster, based_on, window, cx);
                    }))
            })
            .on_mouse_down(
                MouseButton::Right,
                ui.cx.listener(move |v, _, _, cx| {
                    cx.stop_propagation();
                    v.panels.layer_options = Some(id);
                    cx.notify();
                }),
            );
        let visibility = EditorCommand::Layer(LayerCommand::SetVisibility {
            node: id,
            visible: !layer.visible,
        });
        let eye = Button::new("visibility")
            .small()
            .compact()
            .ghost()
            .p_0()
            .w(px(21.))
            .h(px(26.))
            .tooltip(if layer.visible {
                "숨기기"
            } else {
                "표시하기"
            })
            .accessibility_label(format!("{} 가시성", layer.name))
            .toggled(layer.visible)
            .disabled(!enabled || !ui.view.host.supports_command(&visibility))
            .child(layer_icon(if layer.visible {
                IconName::Eye
            } else {
                IconName::EyeOff
            }))
            .on_click(ui.cx.listener(move |v, _, window, cx| {
                cx.stop_propagation();
                window.focus(&v.focus, cx);
                v.send_at(based_on, visibility.clone(), cx);
            }));
        line = line.child(row_control("visibility-control", eye));
        match id {
            LayerTreeNodeId::Group(group) => {
                let collapsed = ui.view.panels.collapsed_groups.contains(&group);
                if collapsed {
                    hidden_below = Some(layer.depth);
                }
                let expander = Button::new("collapse-group")
                    .compact()
                    .small()
                    .ghost()
                    .p_0()
                    .w(px(16.))
                    .h(px(24.))
                    .accessibility_label(if collapsed {
                        "그룹 펼치기"
                    } else {
                        "그룹 접기"
                    })
                    .child(icon(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    }))
                    .on_click(ui.cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        if !v.panels.collapsed_groups.remove(&group) {
                            v.panels.collapsed_groups.insert(group);
                        }
                        cx.notify();
                    }));
                line = line
                    .child(row_control("expand-control", expander))
                    .child(layer_icon(IconName::Folder));
            }
            LayerTreeNodeId::Raster(layer_id) => {
                let frame = thumbnails.frame(layer_id);
                let image_key = Thumbnail::Layer(layer_id);
                let image = frame
                    .as_ref()
                    .and_then(|f| ui.thumbnail(image_key, &f.data_uri))
                    .or_else(|| ui.cached_thumbnail(image_key));
                line = line.child(edit_target(
                    ui,
                    p,
                    layer,
                    RasterEditTarget::Image(layer_id),
                    image,
                ));
                if let Some(mask) = layer.mask {
                    let mask_key = Thumbnail::Mask(mask.id);
                    let image = frame
                        .as_ref()
                        .and_then(|f| f.mask.as_ref())
                        .filter(|f| f.mask == mask.id)
                        .and_then(|f| ui.thumbnail(mask_key, &f.data_uri))
                        .or_else(|| ui.cached_thumbnail(mask_key));
                    line = line.child(edit_target(
                        ui,
                        p,
                        layer,
                        RasterEditTarget::Mask {
                            layer: layer_id,
                            mask: mask.id,
                        },
                        image,
                    ));
                }
            }
        }
        let detail = if matches!(id, LayerTreeNodeId::Group(_)) {
            if selected_group {
                "그룹 설정".to_owned()
            } else {
                "그룹".to_owned()
            }
        } else {
            format!("{}%", artwork::percent(layer.opacity_u16))
        };
        line = line.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .items_start()
                .child(select)
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(0xe0e0e0))
                        .child(detail),
                ),
        );
        let show = ui.view.panels.layer_options == Some(id);
        let menu = Button::new("layer-options")
            .small()
            .compact()
            .ghost()
            .p_0()
            .w(px(20.))
            .h(px(26.))
            .selected(show)
            .accessibility_label(format!("{} 옵션", layer.name))
            .tooltip("레이어 옵션")
            .child(icon(IconName::Ellipsis))
            .on_click(ui.cx.listener(move |v, _, _, cx| {
                cx.stop_propagation();
                v.panels.layer_options = (!show).then_some(id);
                cx.notify();
            }));
        line = line.child(row_control("options-control", menu));
        let enable_drag = enabled
            && ui
                .view
                .host
                .supports_command(&EditorCommand::Layer(LayerCommand::Reorder {
                    node: id,
                    new_parent: layer.parent,
                    index: layer.index,
                }));
        if enable_drag {
            let hint = ui
                .view
                .panels
                .layer_drop
                .filter(|(node, _)| *node == id)
                .map(|(_, position)| position);
            line = line
                .child(row_control(
                    "drag-control",
                    div()
                        .id("move-layer")
                        .cursor_move()
                        .flex_shrink_0()
                        .w(px(16.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .child(icon(IconName::GripVertical))
                        .on_drag(
                            LayerDrag {
                                node: id,
                                revision: based_on,
                                label: layer.name.clone(),
                            },
                            |drag, _, _, cx| cx.new(|_| drag.clone()),
                        ),
                ))
                .drag_over::<LayerDrag>(move |style, _, _, _| {
                    let style = style.border_color(rgb(0x71aee1));
                    match hint {
                        Some(DropPosition::Above) => style.border_t_2(),
                        Some(DropPosition::Below) => style.border_b_2(),
                        Some(DropPosition::Inside) => style.border_2(),
                        None => style,
                    }
                })
                .on_drag_move(
                    ui.cx
                        .listener(move |v, e: &DragMoveEvent<LayerDrag>, _, cx| {
                            if e.drag(cx).node == id || !e.bounds.contains(&e.event.position) {
                                return;
                            }
                            let y = f32::from(e.event.position.y - e.bounds.origin.y)
                                / f32::from(e.bounds.size.height).max(1.);
                            let position = if matches!(id, LayerTreeNodeId::Group(_))
                                && (0.3..0.7).contains(&y)
                            {
                                DropPosition::Inside
                            } else if y < 0.5 {
                                DropPosition::Above
                            } else {
                                DropPosition::Below
                            };
                            if v.panels.layer_drop != Some((id, position)) {
                                v.panels.layer_drop = Some((id, position));
                                cx.notify();
                            }
                        }),
                )
                .on_drop(ui.cx.listener(move |v, drag: &LayerDrag, _, cx| {
                    let Some((target, position)) = v.panels.layer_drop.take() else {
                        return;
                    };
                    if target != id {
                        return;
                    }
                    let snapshot = v.host.snapshot();
                    if snapshot.revision != drag.revision {
                        v.notice =
                            Some("드래그 도중 문서가 변경됐습니다. 다시 옮겨 주세요.".into());
                        cx.notify();
                    } else if let Some(command) = reorder(&snapshot, drag.node, id, position) {
                        v.send_at(drag.revision, EditorCommand::Layer(command), cx);
                    }
                }));
        }
        panel = panel.child(line);
        if show {
            panel = panel.child(properties(ui, p, layer));
        }
    }
    panel
}

fn style_controls(ui: &mut PanelUi, p: &UiProjection, l: &LayerProjection) -> Div {
    let id = l.id;
    let modes = [
        ("일반", LayerBlendMode::Normal),
        ("곱하기", LayerBlendMode::Multiply),
        ("스크린", LayerBlendMode::Screen),
        ("더하기", LayerBlendMode::Add),
        ("오버레이", LayerBlendMode::Overlay),
    ];
    let selected = l.blend_mode;
    let name = modes
        .iter()
        .find(|(_, m)| *m == selected)
        .map_or("일반", |(n, _)| *n);
    let host = ui.view.host.clone();
    let view = ui.cx.weak_entity();
    let revision = p.revision;
    let enabled = !busy(p);
    let blend = Button::new("blend-mode")
        .small()
        .compact()
        .child(
            row()
                .flex_nowrap()
                .child(name)
                .child(icon(IconName::ChevronDown)),
        )
        .accessibility_label("합성 모드")
        .disabled(!enabled)
        .dropdown_menu(move |mut menu, _, _| {
            for (label, mode) in modes {
                let cmd = EditorCommand::Layer(LayerCommand::SetBlendMode {
                    node: id,
                    blend_mode: mode,
                });
                let target = view.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(mode == selected)
                        .disabled(!host.supports_command(&cmd))
                        .on_click(move |_, window, cx| {
                            let _ = target.update(cx, |v, cx| {
                                window.focus(&v.focus, cx);
                                v.send_at(revision, cmd.clone(), cx);
                            });
                        }),
                );
            }
            menu
        });
    column()
        .gap_1()
        .child(ui.number_slider(
            "레이어 불투명도",
            NumberTarget::Layer(id),
            artwork::percent(l.opacity_u16),
            NumberScale::Percent { min: 0 },
            enabled,
            move |n| {
                EditorCommand::Layer(LayerCommand::SetOpacity {
                    node: id,
                    opacity_u16: artwork::ratio(n),
                })
            },
        ))
        .child(row().child(blend))
}

pub(super) fn selected_layer(view: &EditorView, p: &UiProjection) -> Option<LayerTreeNodeId> {
    view.panels
        .group
        .filter(|_| view.panels.last_active_layer == p.active_layer)
        .map(LayerTreeNodeId::Group)
        .filter(|id| p.layers.iter().any(|l| l.id == *id))
        .or_else(|| p.active_layer.map(LayerTreeNodeId::Raster))
}

fn properties(ui: &mut PanelUi, p: &UiProjection, l: &LayerProjection) -> Div {
    let enabled = !busy(p);
    let id = l.id;
    let duplicate = match id {
        LayerTreeNodeId::Raster(r) => LayerCommand::DuplicateRaster(r),
        LayerTreeNodeId::Group(g) => LayerCommand::DuplicateGroup(g),
    };
    let sibling_count = p
        .layers
        .iter()
        .filter(|node| node.parent == l.parent)
        .count();
    let mut panel = column()
        .child(ui.heading(l.name.clone()))
        .child(
            ui.field(
                "레이어 이름 · Enter",
                format!("{id:?}"),
                l.name.clone(),
                enabled
                    && ui
                        .view
                        .host
                        .supports_command(&EditorCommand::Layer(LayerCommand::Rename {
                            node: id,
                            name: l.name.clone(),
                        })),
                move |v, name, cx| {
                    if name.trim().is_empty() {
                        return Err("레이어 이름을 입력하세요.");
                    }
                    v.send(
                        EditorCommand::Layer(LayerCommand::Rename {
                            node: id,
                            name: name.trim().to_owned(),
                        }),
                        cx,
                    );
                    Ok(())
                },
            ),
        )
        .child(
            row()
                .child(ui.button(
                    "단독 표시",
                    EditorCommand::Layer(LayerCommand::ToggleSolo(id)),
                    p.solo_node == Some(id),
                    enabled,
                ))
                .child(ui.button(
                    "아래로 클리핑",
                    EditorCommand::Layer(LayerCommand::SetClipToBelow {
                        node: id,
                        clip_to_below: !l.clip_to_below,
                    }),
                    l.clip_to_below,
                    enabled,
                ))
                .child(ui.button("복제", EditorCommand::Layer(duplicate), false, enabled))
                .child(ui.button(
                    "삭제",
                    EditorCommand::Layer(LayerCommand::Delete(id)),
                    false,
                    enabled && !l.locked,
                )),
        )
        .child(
            row()
                .child(ui.button(
                    "위로",
                    EditorCommand::Layer(LayerCommand::Reorder {
                        node: id,
                        new_parent: l.parent,
                        index: l.index.saturating_add(1),
                    }),
                    false,
                    enabled && l.index + 1 < sibling_count,
                ))
                .child(ui.button(
                    "아래로",
                    EditorCommand::Layer(LayerCommand::Reorder {
                        node: id,
                        new_parent: l.parent,
                        index: l.index.saturating_sub(1),
                    }),
                    false,
                    enabled && l.index > 0,
                )),
        );
    let mut group_moves = row();
    if let Some(parent) = p
        .layers
        .iter()
        .find(|n| n.id == LayerTreeNodeId::Group(l.parent))
    {
        group_moves = group_moves.child(ui.button(
            "그룹 밖으로",
            EditorCommand::Layer(LayerCommand::Reorder {
                node: id,
                new_parent: parent.parent,
                index: parent.index + 1,
            }),
            false,
            enabled,
        ));
    }
    for group in p
        .layers
        .iter()
        .filter(|n| matches!(n.id, LayerTreeNodeId::Group(_)))
    {
        let LayerTreeNodeId::Group(gid) = group.id else {
            continue;
        };
        let mut ancestor = Some(group);
        let mut descendant = false;
        while let Some(node) = ancestor {
            if node.id == id {
                descendant = true;
                break;
            }
            ancestor = p
                .layers
                .iter()
                .find(|n| n.id == LayerTreeNodeId::Group(node.parent));
        }
        if !descendant && gid != l.parent {
            let index = p.layers.iter().filter(|n| n.parent == gid).count();
            group_moves = group_moves.child(ui.button(
                format!("{} 안으로", group.name),
                EditorCommand::Layer(LayerCommand::Reorder {
                    node: id,
                    new_parent: gid,
                    index,
                }),
                false,
                enabled,
            ));
        }
    }
    panel = panel.child(group_moves);
    match id {
        LayerTreeNodeId::Group(g) => panel.child(ui.button(
            "그룹 변형",
            EditorCommand::Edit(EditCommand::FreeTransform(TransformCommand::BeginGroup(g))),
            false,
            enabled,
        )),
        LayerTreeNodeId::Raster(layer) => {
            let effect = l.color_effect.unwrap_or_default();
            let effect_on = l.color_effect.is_some_and(|e| e.enabled);
            panel = panel.child(
                row()
                    .child(ui.button(
                        "레이어 잠금",
                        EditorCommand::Layer(LayerCommand::SetLocked {
                            layer,
                            locked: !l.locked,
                        }),
                        l.locked,
                        enabled,
                    ))
                    .child(ui.button(
                        "투명도 잠금",
                        EditorCommand::Layer(LayerCommand::SetAlphaLocked {
                            layer,
                            alpha_locked: !l.alpha_locked,
                        }),
                        l.alpha_locked,
                        enabled,
                    ))
                    .child(ui.button(
                        "참조 레이어",
                        EditorCommand::Layer(LayerCommand::SetReference {
                            layer,
                            reference: !l.reference,
                        }),
                        l.reference,
                        enabled,
                    ))
                    .child(ui.button(
                        "레이어 색상화",
                        EditorCommand::Layer(LayerCommand::SetColorEffect {
                            layer,
                            effect: Some(LayerColorEffect {
                                enabled: !effect_on,
                                ..effect
                            }),
                        }),
                        effect_on,
                        enabled,
                    ))
                    .child(ui.button(
                        "색 조정",
                        EditorCommand::Edit(EditCommand::ColorAdjustment(
                            ColorAdjustmentCommand::BeginLayer(layer),
                        )),
                        false,
                        enabled && !l.locked,
                    )),
            );
            if effect_on {
                let [r, g, b, _] = p.brush_color;
                panel = panel.child(
                    row()
                        .child(ui.button(
                            "어두운 색에 전경색",
                            EditorCommand::Layer(LayerCommand::SetColorEffect {
                                layer,
                                effect: Some(LayerColorEffect {
                                    dark_rgb: [r, g, b],
                                    ..effect
                                }),
                            }),
                            false,
                            enabled,
                        ))
                        .child(ui.button(
                            "밝은 색에 전경색",
                            EditorCommand::Layer(LayerCommand::SetColorEffect {
                                layer,
                                effect: Some(LayerColorEffect {
                                    light_rgb: [r, g, b],
                                    ..effect
                                }),
                            }),
                            false,
                            enabled,
                        )),
                );
            }
            let mask = if let Some(mask) = l.mask {
                let image_target = RasterEditTarget::Image(layer);
                let mask_target = RasterEditTarget::Mask {
                    layer,
                    mask: mask.id,
                };
                row()
                    .child(
                        target_button(ui, p, l, image_target)
                            .selected(p.raster_target == Some(image_target))
                            .label("이미지 편집"),
                    )
                    .child(
                        target_button(ui, p, l, mask_target)
                            .selected(p.raster_target == Some(mask_target))
                            .label("마스크 편집"),
                    )
                    .child(ui.button(
                        "마스크 사용",
                        EditorCommand::Mask(MaskCommand::SetEnabled {
                            layer,
                            enabled: !mask.enabled,
                        }),
                        mask.enabled,
                        enabled && !l.locked,
                    ))
                    .child(ui.button(
                        "마스크 제거",
                        EditorCommand::Mask(MaskCommand::Remove(layer)),
                        false,
                        enabled && !l.locked,
                    ))
            } else {
                row()
                    .child(ui.button(
                        "마스크 추가",
                        EditorCommand::Mask(MaskCommand::AddRevealAll(layer)),
                        false,
                        enabled && !l.locked,
                    ))
                    .child(ui.button(
                        "선택에서 마스크",
                        EditorCommand::Mask(MaskCommand::AddFromSelection(layer)),
                        false,
                        enabled && !l.locked && p.edit.has_selection,
                    ))
            };
            panel.child(mask)
        }
    }
}
