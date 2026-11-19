use super::*;
use std::{cell::Cell, f32::consts::TAU, sync::Arc};

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Hue,
    SaturationValue,
    Value,
}

pub(super) struct Wheel {
    hue: f32,
    saturation: f32,
    value: f32,
    authority: Option<[u8; 4]>,
    drag: Option<(Axis, [f32; 3])>,
    focus: [FocusHandle; 3],
    ring: ImageSource,
    panel_edge: Rc<Cell<f32>>,
}

impl Wheel {
    pub(super) fn has_keyboard_focus(&self, window: &Window) -> bool {
        self.focus.iter().any(|axis| axis.is_focused(window))
    }

    pub(super) fn new(cx: &App) -> Self {
        Self {
            hue: 0.,
            saturation: 0.,
            value: 0.,
            authority: None,
            drag: None,
            focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            panel_edge: Rc::new(Cell::new(136.)),
            ring: ImageSource::Image(Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../assets/hue-ring.png").to_vec(),
            ))),
        }
    }

    pub(super) fn measure_panel(&self) -> AnyElement {
        let edge = self.panel_edge.clone();
        canvas(
            move |bounds, window, _| {
                let available = (f32::from(bounds.size.height) - 52.)
                    .min(f32::from(bounds.size.width) - 40.)
                    .clamp(48., 210.)
                    .floor();
                if edge.replace(available) != available {
                    window.refresh();
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .inset_0()
        .size_full()
        .into_any_element()
    }
    pub(super) fn cancel(&mut self) {
        self.authority = None;
        self.drag = None;
    }

    fn sync(&mut self, rgba: [u8; 4]) {
        if self.authority == Some(rgba) {
            return;
        }
        let echoed = self.authority.is_some()
            && Self::rgb(self.hue, self.saturation, self.value) == rgba[..3];
        self.authority = Some(rgba);
        self.drag = None;
        if echoed {
            return;
        }
        let [r, g, b, _] = rgba.map(|c| f32::from(c) / 255.);
        let max = r.max(g).max(b);
        let delta = max - r.min(g).min(b);
        self.value = max;
        if max > 0. {
            self.saturation = delta / max;
        }
        if delta > 0. {
            let sector = if max == r {
                (g - b) / delta
            } else if max == g {
                (b - r) / delta + 2.
            } else {
                (r - g) / delta + 4.
            };
            self.hue = (sector / 6.).rem_euclid(1.);
        }
    }

    fn rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
        [5., 3., 1.].map(|n| {
            let k = (n + h * 6.) % 6.;
            (255. * v * (1. - s * k.min(4. - k).clamp(0., 1.))).round() as u8
        })
    }

    fn sample(&mut self, axis: Axis, p: Point<Pixels>, area: Bounds<Pixels>, begin: bool) -> bool {
        if area.size.width <= px(0.) || area.size.height <= px(0.) {
            return false;
        }
        let offset = p - area.origin;
        let x = offset.x / area.size.width;
        let y = offset.y / area.size.height;
        if begin {
            if self.drag.is_some()
                || (axis == Axis::Hue && !(0.37..=0.5).contains(&(x - 0.5).hypot(y - 0.5)))
            {
                return false;
            }
            self.drag = Some((axis, [self.hue, self.saturation, self.value]));
        } else if !self.drag.is_some_and(|(active, _)| active == axis) {
            return false;
        }
        match axis {
            Axis::Hue => self.hue = ((x - 0.5).atan2(0.5 - y) / TAU).rem_euclid(1.),
            Axis::SaturationValue => {
                self.saturation = x.clamp(0., 1.);
                self.value = (1. - y).clamp(0., 1.);
            }
            Axis::Value => self.value = (1. - y).clamp(0., 1.),
        }
        true
    }

    fn adjust(&mut self, axis: Axis, key: &str, shift: bool) -> bool {
        let step = if shift { 10. } else { 1. };
        let delta = match key {
            "right" | "up" => step,
            "left" | "down" => -step,
            "home" | "end" => 0.,
            _ => return false,
        };
        if axis == Axis::Hue {
            self.hue = match key {
                "home" => 0.,
                "end" => 359. / 360.,
                _ => (self.hue + delta / 360.).rem_euclid(1.),
            };
        } else {
            let channel = if axis == Axis::Value || matches!(key, "up" | "down") {
                &mut self.value
            } else {
                &mut self.saturation
            };
            *channel = match key {
                "home" => 0.,
                "end" => 1.,
                _ => (*channel + delta / 100.).clamp(0., 1.),
            };
        }
        true
    }
}

impl EditorView {
    fn commit_color(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.host.snapshot();
        let [r, g, b] = Wheel::rgb(self.wheel.hue, self.wheel.saturation, self.wheel.value);
        let color = [r, g, b, snapshot.brush_color[3]];
        if self.epoch == self.host.project_epoch()
            && self.wheel.authority == Some(snapshot.brush_color)
            && color != snapshot.brush_color
        {
            self.send_at(
                snapshot.revision,
                EditorCommand::Tool(ToolCommand::SetColor(color)),
                cx,
            );
        }
        cx.notify();
    }

    fn end_color(
        &mut self,
        axis: Axis,
        p: Point<Pixels>,
        area: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.wheel.sample(axis, p, area, false) {
            self.wheel.drag = None;
            self.commit_color(cx);
            cx.stop_propagation();
        }
    }

    fn color_axis(&self, axis: Axis, cx: &Context<Self>) -> Stateful<Div> {
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let (measure, press, motion, release) = (
            bounds.clone(),
            bounds.clone(),
            bounds.clone(),
            bounds.clone(),
        );
        let (id, label) = match axis {
            Axis::Hue => ("color-hue", "색조"),
            Axis::SaturationValue => ("color-saturation-value", "채도와 명도"),
            Axis::Value => ("color-value", "명도"),
        };
        let mut control = div()
            .id(id)
            .track_focus(&self.wheel.focus[axis as usize])
            .aria_label(label)
            .focus_visible(|s| s.border_2().border_color(rgb(0x72baff)))
            .cursor_crosshair();
        if axis == Axis::SaturationValue {
            control = control.role(Role::Group).aria_value(format!(
                "채도 {:.0}%, 명도 {:.0}%",
                self.wheel.saturation * 100.,
                self.wheel.value * 100.
            ));
        } else {
            let (maximum, current) = if axis == Axis::Hue {
                (359., (self.wheel.hue * 360.).round().rem_euclid(360.))
            } else {
                (100., (self.wheel.value * 100.).round())
            };
            control = control
                .role(Role::Slider)
                .aria_min_numeric_value(0.)
                .aria_max_numeric_value(maximum)
                .aria_numeric_value(f64::from(current))
                .aria_numeric_value_step(1.);
        }
        control
            .child(
                canvas(move |b, _, _| measure.set(b), |_, (), _, _| {})
                    .absolute()
                    .inset_0()
                    .size_full(),
            )
            .on_key_down(cx.listener(move |view, e: &KeyDownEvent, window, cx| {
                if !view.wheel.focus[axis as usize].is_focused(window) {
                    return;
                }
                let key = e.keystroke.key.as_str();
                if key == "escape"
                    && let Some((_, before)) = view.wheel.drag.take()
                {
                    [view.wheel.hue, view.wheel.saturation, view.wheel.value] = before;
                    cx.notify();
                    cx.stop_propagation();
                } else if view.wheel.drag.is_none()
                    && view.wheel.adjust(axis, key, e.keystroke.modifiers.shift)
                {
                    view.commit_color(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, e: &MouseDownEvent, window, cx| {
                    if view.wheel.sample(axis, e.position, press.get(), true) {
                        window.focus(&view.wheel.focus[axis as usize], cx);
                        cx.notify();
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(move |view, e: &MouseMoveEvent, _, cx| {
                if e.pressed_button == Some(MouseButton::Left)
                    && view.wheel.sample(axis, e.position, motion.get(), false)
                {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, e: &MouseUpEvent, _, cx| {
                    view.end_color(axis, e.position, release.get(), cx)
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |view, e: &MouseUpEvent, _, cx| {
                    view.end_color(axis, e.position, bounds.get(), cx)
                }),
            )
    }

    pub(crate) fn color_wheel(&mut self, color: [u8; 4], cx: &Context<Self>) -> AnyElement {
        self.wheel.sync(color);
        let (h, s, v) = (self.wheel.hue, self.wheel.saturation, self.wheel.value);
        let sample = canvas(
            |_, _, _| {},
            move |b, (), window, _| {
                let center = b.center();
                let radius = b.size.width.min(b.size.height) / 2.;
                let origin = center - point(radius / 2., radius / 2.);
                let square = Bounds::new(origin, size(radius, radius));
                let [r, g, blue] = Wheel::rgb(h, 1., 1.);
                let hue = rgba(u32::from_be_bytes([r, g, blue, 255]));
                window.paint_quad(fill(
                    square,
                    linear_gradient(
                        90.,
                        linear_color_stop(white(), 0.),
                        linear_color_stop(hue, 1.),
                    ),
                ));
                window.paint_quad(fill(
                    square,
                    linear_gradient(
                        180.,
                        linear_color_stop(transparent_black(), 0.),
                        linear_color_stop(black(), 1.),
                    ),
                ));
                let markers = [
                    center
                        + point(
                            radius * (h * TAU).sin() * 0.87,
                            -radius * (h * TAU).cos() * 0.87,
                        ),
                    origin + point(radius * s, radius * (1. - v)),
                ];
                for pos in markers {
                    for (edge, ink) in [(px(5.), black()), (px(4.), white())] {
                        let bounds =
                            Bounds::new(pos - point(edge, edge), size(edge * 2., edge * 2.));
                        window.paint_quad(quad(
                            bounds,
                            edge,
                            transparent_black(),
                            px(1.),
                            ink,
                            BorderStyle::default(),
                        ));
                    }
                }
            },
        )
        .absolute()
        .inset_0()
        .size_full();
        let edge = px(self.wheel.panel_edge.get());
        let hue = self
            .color_axis(Axis::Hue, cx)
            .absolute()
            .inset_0()
            .rounded_full();
        let square = self
            .color_axis(Axis::SaturationValue, cx)
            .absolute()
            .left(edge / 4.)
            .top(edge / 4.)
            .size(edge / 2.);
        let picker = div()
            .relative()
            .size(edge)
            .flex_shrink_0()
            .cursor_crosshair()
            .child(
                img(self.wheel.ring.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size(edge)
                    .object_fit(ObjectFit::Contain),
            )
            .child(sample)
            .child(hue)
            .child(square);
        let [r, g, b] = Wheel::rgb(h, s, 1.);
        let top = rgba(u32::from_be_bytes([r, g, b, 255]));
        let strip = self
            .color_axis(Axis::Value, cx)
            .relative()
            .w(px(18.))
            .h(edge)
            .flex_shrink_0()
            .border_1()
            .border_color(rgb(0x555555))
            .bg(linear_gradient(
                180.,
                linear_color_stop(top, 0.),
                linear_color_stop(black(), 1.),
            ))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top((edge - px(4.)) * (1. - v))
                    .h(px(4.))
                    .border_1()
                    .border_color(black())
                    .bg(white()),
            );
        div()
            .flex()
            .justify_center()
            .gap(px(6.))
            .w_full()
            .h(edge)
            .flex_shrink_0()
            .child(picker)
            .child(strip)
            .into_any_element()
    }
}
