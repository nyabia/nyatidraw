use super::*;
use std::cell::Cell;

const DIVIDER_WIDTH: f32 = 4.;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum CompactPanels {
    #[default]
    Tools,
    Artwork,
    Collapsed,
}

pub(super) struct PendingResize {
    receipt: u128,
    split: usize,
    ratio: u16,
    previous: Rc<DockTree>,
}

#[derive(Clone, Copy)]
pub(crate) struct PanelDrag(pub PanelKind);

impl Render for PanelDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .bg(rgb(0x325568))
            .text_color(white())
            .child(title(self.0))
    }
}

pub(crate) fn handle(
    kind: PanelKind,
    label: &'static str,
    cx: &Context<EditorView>,
) -> Stateful<Div> {
    let view = cx.weak_entity();
    div()
        .id(("panel-handle", kind as usize))
        .px_1()
        .cursor_move()
        .bg(rgb(0x494949))
        .on_drag(PanelDrag(kind), move |drag, _, _, cx| {
            let _ = view.update(cx, |v, cx| {
                v.host.document_canvas_dock_drag(true);
                cx.notify();
            });
            cx.new(|_| *drag)
        })
        .when(label != "⠿", |d| d.child(label))
        .when(label == "⠿", |d| {
            d.h(px(26.))
                .justify_center()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children((0..4).map(|_| div().w(px(3.)).h(px(2.)).bg(rgb(0xb4b4b4))))
        })
}

fn title(panel: PanelKind) -> &'static str {
    match panel {
        PanelKind::Canvas => "캔버스",
        PanelKind::Tools => "도구",
        PanelKind::Brush => "세부 도구",
        PanelKind::ToolProperties => "도구 속성",
        PanelKind::BrushSizes => "크기 빠른 선택",
        PanelKind::Color => "색상",
        PanelKind::Layers => "레이어",
        PanelKind::History => "히스토리",
        PanelKind::Navigator => "내비게이터",
        PanelKind::CanvasActions => "캔버스 작업",
        PanelKind::Viewport => "보기",
        PanelKind::QuickColors => "최근 색상",
    }
}

impl EditorView {
    pub(super) fn visible(&self, panel: PanelKind) -> bool {
        self.enabled_panel(panel)
            && (!self.compact_dock
                || match panel {
                    PanelKind::Canvas | PanelKind::Tools => true,
                    PanelKind::Brush | PanelKind::BrushSizes | PanelKind::ToolProperties => {
                        self.compact_panels == CompactPanels::Tools
                    }
                    _ => panel.is_toolbar() || self.compact_panels == CompactPanels::Artwork,
                })
    }

    fn enabled_panel(&self, panel: PanelKind) -> bool {
        self.host.panel_visible(match panel {
            PanelKind::Tools => "tools",
            PanelKind::Brush | PanelKind::BrushSizes | PanelKind::ToolProperties => "brush",
            PanelKind::Color | PanelKind::QuickColors => "colors",
            PanelKind::Layers => "layers",
            PanelKind::History => "history",
            PanelKind::Navigator => "navigator",
            PanelKind::Canvas => return true,
            PanelKind::CanvasActions | PanelKind::Viewport => "toolbar",
        })
    }
    pub(super) fn dock_min_width(&self, node: &DockNode, compact: bool) -> f32 {
        let panel_width = |kind| {
            if !(if compact {
                self.visible(kind)
            } else {
                self.enabled_panel(kind)
            }) {
                return 0.;
            }
            if compact && self.compact_dock && kind == PanelKind::Canvas {
                return 96.;
            }
            f32::from(nyatidraw_editor_ui::layout_store::minimum_width(
                &DockNode::Panel(kind),
            ))
        };
        match node {
            DockNode::Panel(p) => panel_width(*p),
            DockNode::Tabs { panels, .. } => {
                panels.iter().copied().map(panel_width).fold(0., f32::max)
            }
            DockNode::Split {
                axis,
                first,
                second,
                ..
            } => {
                let first = self.dock_min_width(first, compact);
                let second = self.dock_min_width(second, compact);
                if *axis == DockAxis::Horizontal && first > 0. && second > 0. {
                    first + second + DIVIDER_WIDTH
                } else {
                    first.max(second)
                }
            }
        }
    }

    pub(super) fn compact_dock_controls(&self, cx: &mut Context<Self>) -> Div {
        let mut controls = div().flex().flex_shrink_0().items_center().gap_1().px_1();
        for (label, group) in [
            ("속성", CompactPanels::Tools),
            ("색·레이어", CompactPanels::Artwork),
        ] {
            let opened = self.compact_panels == group;
            controls = controls.child(
                action_control(label, true)
                    .when(opened, |button| button.bg(rgb(0x325568)))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.compact_panels = if opened {
                            CompactPanels::Collapsed
                        } else {
                            group
                        };
                        cx.notify();
                    })),
            );
        }
        controls
    }
    pub(super) fn dock_node(
        &mut self,
        node: &DockNode,
        id: usize,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            DockNode::Split {
                axis,
                first_per_mille,
                first,
                second,
            } => {
                let min_first = self.dock_min_width(first, true);
                let min_second = self.dock_min_width(second, true);
                if min_first == 0. {
                    return self.dock_node(second, id * 2 + 1, width, window, cx);
                }
                if min_second == 0. {
                    return self.dock_node(first, id * 2, width, window, cx);
                }
                let horizontal = *axis == DockAxis::Horizontal;
                let ratio = self
                    .resizing
                    .or_else(|| {
                        self.pending_dock_resize.as_ref().and_then(|pending| {
                            (*pending.previous == self.projection.dock)
                                .then_some((pending.split, pending.ratio))
                        })
                    })
                    .filter(|(key, _)| *key == id)
                    .map_or(*first_per_mille, |(_, ratio)| ratio);
                let fraction = f32::from(ratio) / 1000.;
                let room = (width - DIVIDER_WIDTH).max(0.);
                let left = if min_first + min_second <= room {
                    (room * fraction).clamp(min_first, room - min_second)
                } else {
                    room * min_first / (min_first + min_second)
                };
                let (first_width, second_width) = if horizontal {
                    (left, room - left)
                } else {
                    (width, width)
                };
                let layout = Rc::new(self.projection.dock.clone());
                let divider = div()
                    .id(("split-handle", id))
                    .focusable()
                    .tab_index(0)
                    .flex_shrink_0()
                    .bg(rgb(0x242424))
                    .cursor(if horizontal {
                        CursorStyle::ResizeLeftRight
                    } else {
                        CursorStyle::ResizeUpDown
                    })
                    .when(horizontal, |d| d.w(px(DIVIDER_WIDTH)).h_full())
                    .when(!horizontal, |d| d.h(px(DIVIDER_WIDTH)).w_full())
                    .hover(|s| s.bg(rgb(0x6dc4ec)))
                    .focus_visible(|s| s.bg(rgb(0x6dc4ec)))
                    .on_key_down(cx.listener(move |view, e: &KeyDownEvent, _, cx| {
                        let change = match (horizontal, e.keystroke.key.as_str()) {
                            (true, "left") | (false, "up") => -1.,
                            (true, "right") | (false, "down") => 1.,
                            _ => return,
                        };
                        let shift = if e.keystroke.modifiers.shift {
                            40.
                        } else {
                            12.
                        };
                        let ratio = if horizontal {
                            resize_ratio(left + change * shift, room)
                        } else {
                            (i32::from(ratio) + (change * shift).round() as i32).clamp(20, 980)
                                as u16
                        };
                        view.save_split_ratio(id, ratio, None, cx);
                        cx.stop_propagation();
                    }))
                    .on_drag(SplitDrag { id, layout }, |drag, _, _, cx| {
                        cx.new(|_| drag.clone())
                    });

                div()
                    .size_full()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .when(!horizontal, |d| d.flex_col())
                    .on_drag_move(cx.listener(move |v, e: &DragMoveEvent<SplitDrag>, _, cx| {
                        if e.drag(cx).id != id {
                            return;
                        }
                        let (offset, length) = if horizontal {
                            (e.event.position.x - e.bounds.origin.x, e.bounds.size.width)
                        } else {
                            (e.event.position.y - e.bounds.origin.y, e.bounds.size.height)
                        };
                        if length <= px(0.) {
                            return;
                        }
                        let ratio = resize_ratio(
                            f32::from(offset) - DIVIDER_WIDTH / 2.,
                            f32::from(length) - DIVIDER_WIDTH,
                        );
                        if v.resizing != Some((id, ratio)) {
                            v.resizing = Some((id, ratio));
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .when(horizontal, |d| d.w(px(first_width)).h_full())
                            .when(!horizontal, |d| d.h(relative(fraction)).w_full())
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(self.dock_node(first, id * 2, first_width, window, cx)),
                    )
                    .child(divider)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.dock_node(second, id * 2 + 1, second_width, window, cx)),
                    )
                    .into_any_element()
            }
            DockNode::Panel(p) => self.dock_panel(*p, None, window, cx),
            DockNode::Tabs { active, panels } => {
                let Some(selected) = panels
                    .get(*active)
                    .copied()
                    .filter(|p| self.visible(*p))
                    .or_else(|| panels.iter().copied().find(|p| self.visible(*p)))
                else {
                    return div().into_any_element();
                };
                let mut tabs = div().flex().flex_wrap().bg(rgb(0x424242));
                for p in panels.iter().copied().filter(|p| self.visible(*p)) {
                    tabs = tabs.child(
                        handle(p, title(p), cx)
                            .when(p == selected, |d| {
                                d.border_b_2().border_color(rgb(0x73b7d5))
                            })
                            .when(p != selected, |tab| {
                                tab.on_click(cx.listener(move |view, _, _, cx| {
                                    view.send(
                                        EditorCommand::Dock(DockCommand::ActivatePanel(p)),
                                        cx,
                                    )
                                }))
                            }),
                    );
                }
                self.dock_panel(selected, Some(tabs), window, cx)
            }
        }
    }
    fn dock_panel(
        &mut self,
        kind: PanelKind,
        header: Option<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.visible(kind) {
            return div().into_any_element();
        }
        let panel = if kind == PanelKind::Canvas {
            self.canvas_drawn = true;
            self.canvas_workspace(window, cx)
        } else if matches!(kind, PanelKind::CanvasActions | PanelKind::Viewport) {
            self.toolbar(kind, false, window, cx)
        } else {
            panels::panel(kind, self, window, cx)
        };
        let top = header.or_else(|| {
            (kind != PanelKind::Canvas)
                .then(|| div().bg(rgb(0x494949)).child(handle(kind, title(kind), cx)))
        });
        let hint = self
            .dock_hint
            .filter(|(target, _)| *target == kind)
            .map(|(_, position)| position);
        let drop_bounds = Rc::new(Cell::new(None));
        let measure_bounds = drop_bounds.clone();
        div()
            .id(("dock-panel", kind as usize))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .border_1()
            .border_color(rgb(0x242424))
            // The separate canvas surface cannot display GPUI above itself.
            .when(kind == PanelKind::Canvas, |d| d.p(px(3.)))
            .bg(rgb(0x555555))
            .overflow_hidden()
            .on_drag_move(
                cx.listener(move |view, e: &DragMoveEvent<PanelDrag>, _, cx| {
                    if e.drag(cx).0 == kind {
                        return;
                    }
                    let target = dock_position(e.bounds, e.event.position).map(|pos| (kind, pos));
                    if view.dock_hint != target
                        && (target.is_some()
                            || view.dock_hint.is_some_and(|(target, _)| target == kind))
                    {
                        view.dock_hint = target;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |view, drag: &PanelDrag, window, cx| {
                if drag.0 != kind
                    && let Some(position) = drop_bounds
                        .get()
                        .and_then(|bounds| dock_position(bounds, window.mouse_position()))
                {
                    view.send(
                        EditorCommand::Dock(DockCommand::MovePanel {
                            panel: drag.0,
                            target: kind,
                            position,
                        }),
                        cx,
                    );
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| measure_bounds.set(Some(bounds)),
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .children(top)
            .child(
                div()
                    .id(("panel-scroll", kind as usize))
                    .relative()
                    .when(kind == PanelKind::Color, |d| {
                        d.child(self.wheel.measure_panel())
                    })
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(kind != PanelKind::Canvas, |d| d.overflow_y_scroll())
                    .child(panel),
            )
            .children(hint.map(|position| drop_marker(position, kind == PanelKind::Canvas)))
            .into_any_element()
    }
}

#[derive(Clone)]
pub(super) struct SplitDrag {
    id: usize,
    layout: Rc<DockTree>,
}

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn dock_position(bounds: Bounds<Pixels>, point: Point<Pixels>) -> Option<DockPosition> {
    if !bounds.contains(&point) {
        return None;
    }
    let relative = point - bounds.origin;
    let x = f32::from(relative.x) / f32::from(bounds.size.width).max(1.);
    let y = f32::from(relative.y) / f32::from(bounds.size.height).max(1.);
    Some(if x < 0.25 {
        DockPosition::Left
    } else if x > 0.75 {
        DockPosition::Right
    } else if y < 0.25 {
        DockPosition::Top
    } else if y > 0.75 {
        DockPosition::Bottom
    } else {
        DockPosition::Tab
    })
}

fn resize_ratio(offset: f32, room: f32) -> u16 {
    (offset / room.max(1.) * 1000.).clamp(20., 980.).round() as u16
}

fn drop_marker(position: DockPosition, canvas: bool) -> Div {
    let marker = div().absolute().bg(rgb(0x6dc4ec));
    match position {
        DockPosition::Left => marker.left_0().top_0().bottom_0().w(px(3.)),
        DockPosition::Right => marker.right_0().top_0().bottom_0().w(px(3.)),
        DockPosition::Top => marker.top_0().left_0().right_0().h(px(3.)),
        DockPosition::Bottom => marker.bottom_0().left_0().right_0().h(px(3.)),
        DockPosition::Tab if canvas => div()
            .absolute()
            .inset_0()
            .border(px(3.))
            .border_color(rgb(0x6dc4ec)),
        DockPosition::Tab => marker.top(px(18.)).left_0().right_0().h(px(3.)),
    }
}

impl EditorView {
    pub(super) fn finish_resize(&mut self, drag: &SplitDrag, cx: &mut Context<Self>) {
        let Some((id, ratio)) = self.resizing.take() else {
            return;
        };
        if id != drag.id {
            return;
        }
        self.save_split_ratio(id, ratio, Some(drag.layout.clone()), cx);
    }

    pub(super) fn poll_dock_resize(&mut self) {
        if let Some(receipt) = self
            .pending_dock_resize
            .as_ref()
            .map(|pending| pending.receipt)
            && let Some(result) = self.host.take_command_result(receipt)
        {
            self.pending_dock_resize = None;
            self.notice = result.err();
        }
    }

    fn save_split_ratio(
        &mut self,
        id: usize,
        ratio: u16,
        expected: Option<Rc<DockTree>>,
        cx: &mut Context<Self>,
    ) {
        self.poll_dock_resize();
        let snapshot = self.host.snapshot();
        let dock = snapshot.dock;
        if self.pending_dock_resize.is_some()
            || expected.as_ref().is_some_and(|expected| **expected != dock)
        {
            return;
        }
        let mut root = dock.root().clone();
        let mut node = &mut root;
        let depth = id.ilog2();
        for shift in (0..depth).rev() {
            let DockNode::Split { first, second, .. } = node else {
                return;
            };
            node = if id & (1 << shift) == 0 {
                first
            } else {
                second
            };
        }
        if let DockNode::Split {
            first_per_mille, ..
        } = node
        {
            if *first_per_mille == ratio {
                return;
            }
            *first_per_mille = ratio;
            if let Ok(layout) = DockTree::with_top(root, dock.top().to_vec()) {
                let command = EditorCommand::Dock(DockCommand::Replace(layout));
                if let Some(previous) = expected {
                    match self.host.begin_tracked_command(snapshot.revision, command) {
                        Ok(Some(receipt)) => {
                            self.pending_dock_resize = Some(PendingResize {
                                receipt,
                                split: id,
                                ratio,
                                previous,
                            })
                        }
                        Ok(None) => self.notice = None,
                        Err(error) => self.notice = Some(error),
                    }
                    cx.notify();
                } else {
                    self.send_at(snapshot.revision, command, cx);
                }
            }
        }
    }
}
