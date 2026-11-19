use super::*;
use gpui_kit::component::Sizable;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use nyatidraw_input::{CanvasInputMode, PressurePreset};

pub(super) struct BackgroundPicker {
    picker: Entity<ColorPickerState>,
    _choice: Subscription,
}

impl BackgroundPicker {
    pub(super) fn new(
        [r, g, b]: [u8; 3],
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) -> Self {
        let picker = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(rgba(u32::from_be_bytes([r, g, b, 255])))
        });
        let choice = cx.subscribe(&picker, |view, _, event: &ColorPickerEvent, cx| {
            if let ColorPickerEvent::Change(Some(chosen)) = event {
                let rgb = chosen.to_rgb();
                let mut next = view.host.appearance();
                next.solid_rgb = [rgb.r, rgb.g, rgb.b].map(|c| (c * 255.).round() as u8);
                next.checkerboard = false;
                view.host.set_appearance(next);
                cx.notify();
            }
        });
        Self {
            picker,
            _choice: choice,
        }
    }
}

impl EditorView {
    pub(super) fn preferences(&self, picker: &BackgroundPicker, cx: &Context<Self>) -> AnyElement {
        let mut modes = div().flex().gap_2().items_center().child("그리기 입력");
        for (label, mode) in [
            ("자동", CanvasInputMode::Auto),
            ("펜", CanvasInputMode::Pen),
            ("손가락", CanvasInputMode::Finger),
        ] {
            modes = modes.child(
                control(label, true)
                    .when(self.host.input_mode() == mode, |d| d.bg(rgb(0x576a74)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.notice = v.host.set_input_mode(mode).err();
                        cx.notify();
                    })),
            );
        }
        let mut pressure = div().flex().gap_2().items_center().child("필압 반응");
        for (label, preset) in [
            ("선형", PressurePreset::Linear),
            ("부드럽게", PressurePreset::Soft),
            ("더 부드럽게", PressurePreset::Softer),
        ] {
            pressure = pressure.child(
                control(label, true)
                    .when(self.host.pressure_preset() == preset, |d| {
                        d.bg(rgb(0x576a74))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.notice = v.host.set_pressure_preset(preset).err();
                        cx.notify();
                    })),
            );
        }
        let appearance = self.host.appearance();
        let mut background = div().flex().gap_2().items_center().child("캔버스 바깥");
        for (label, color) in [
            ("체크무늬", None),
            ("어둡게", Some([54; 3])),
            ("회색", Some([128; 3])),
            ("밝게", Some([210; 3])),
        ] {
            let selected = if let Some(rgb) = color {
                !appearance.checkerboard && appearance.solid_rgb == rgb
            } else {
                appearance.checkerboard
            };
            background = background.child(
                control(label, true)
                    .when(selected, |d| d.bg(rgb(0x576a74)))
                    .on_click(cx.listener(move |v, _, window, cx| {
                        let mut next = v.host.appearance();
                        next.checkerboard = color.is_none();
                        if let Some([r, g, b]) = color {
                            next.solid_rgb = [r, g, b];
                            if let Some(picker) = &v.settings {
                                picker.picker.update(cx, |state, cx| {
                                    state.set_value(
                                        rgba(u32::from_be_bytes([r, g, b, 255])),
                                        window,
                                        cx,
                                    )
                                });
                            }
                        }
                        v.host.set_appearance(next);
                        cx.notify();
                    })),
            );
        }
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .justify_center()
            .items_center()
            .bg(rgba(0x000000a8))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0x393939))
                    .child("설정")
                    .child(modes)
                    .child(pressure)
                    .child(
                        background
                            .child(ColorPicker::new(&picker.picker).small().label("직접 선택")),
                    )
                    .child(control("닫기", true).on_click(cx.listener(|v, _, w, cx| {
                        v.settings = None;
                        w.focus(&v.focus, cx);
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }
}
