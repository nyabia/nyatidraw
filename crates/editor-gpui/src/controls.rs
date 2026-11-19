use super::EditorView;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Sizable,
        button::{Button, ButtonVariants},
    },
    prelude::*,
    *,
};
use nyatidraw_api::{EditorCommand, ViewportCommand};

pub(super) struct RotationField {
    angle: i32,
    input: Entity<InputState>,
    _events: Subscription,
}

impl RotationField {
    fn degrees(angle: i32) -> String {
        let signed = (angle.rem_euclid(360_000) + 180_000) % 360_000 - 180_000;
        (f64::from(signed) / 1_000.).to_string()
    }

    pub(super) fn new(angle: i32, window: &mut Window, cx: &mut Context<EditorView>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(Self::degrees(angle)));
        let events = cx.subscribe_in(&input, window, |view, field, event, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let value = field.read(cx).value().trim().parse::<f64>();
            let current = view.host.snapshot().viewport.rotation_millidegrees;
            if let Ok(degrees) = value
                && degrees.is_finite()
            {
                let angle = ((degrees.rem_euclid(360.0) * 1_000.0).round() as i32) % 360_000;
                if angle != current.rem_euclid(360_000) {
                    view.send(
                        EditorCommand::Viewport(ViewportCommand::SetRotationMillidegrees(angle)),
                        cx,
                    );
                }
            } else {
                field.update(cx, |f, cx| f.set_value(Self::degrees(current), window, cx));
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                window.focus(&view.focus, cx);
            }
        });
        Self {
            angle,
            input,
            _events: events,
        }
    }

    pub(super) fn render(
        &mut self,
        angle: i32,
        enabled: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        if self.angle != angle {
            self.angle = angle;
            self.input
                .update(cx, |f, cx| f.set_value(Self::degrees(angle), window, cx));
        }
        div().w(px(71.)).child(
            Input::new(&self.input)
                .small()
                .text_right()
                .suffix("°")
                .disabled(!enabled),
        )
    }
}

gpui_kit::assets::icon_assets!(
    ToolbarIcons,
    [
        Menu,
        FilePlus,
        FolderOpen,
        Save,
        Undo2,
        Redo2,
        SquareX,
        Square,
        Scaling,
        Frame,
        X,
        Maximize,
        Minus,
        Plus,
        FlipHorizontal2
    ]
);

pub(super) fn control(label: &'static str, enabled: bool) -> Button {
    Button::new(label)
        .small()
        .compact()
        .ghost()
        .disabled(!enabled)
        .label(label)
}

pub(super) fn action_control(label: &'static str, enabled: bool) -> Button {
    use IconName::*;
    let (icon, shortcut) = match label {
        "메뉴" => (Menu, ""),
        "새 그림" => (FilePlus, ""),
        "열기" => (FolderOpen, "Alt O"),
        "저장" => (Save, "S"),
        "실행 취소" => (Undo2, "Z"),
        "다시 실행" => (Redo2, "Shift Z"),
        "비우기" => (SquareX, ""),
        "흰 배경" => (Square, ""),
        "변형" => (Scaling, ""),
        "캔버스" => (Frame, ""),
        "취소" => (X, "Esc"),
        "맞춤" => (Maximize, ""),
        "−" => (Minus, ""),
        "+" => (Plus, ""),
        "좌우 반전" => (FlipHorizontal2, ""),
        _ => return control(label, enabled),
    };
    let tooltip = if shortcut.is_empty() {
        label.to_owned()
    } else {
        format!("{label} ({shortcut})")
    };
    let button = Button::new(label)
        .small()
        .compact()
        .ghost()
        .disabled(!enabled)
        .accessibility_label(label)
        .tooltip(tooltip);
    match ToolbarIcons.load(icon.path().as_ref()) {
        Ok(Some(bytes)) => button.child(
            div()
                .flex()
                .items_end()
                .gap(px(2.))
                .child(svg().data(&bytes).size_4().text_color(rgb(0xf1f1f1)))
                .when(!shortcut.is_empty(), |d| {
                    d.child(div().text_size(px(11.)).child(shortcut))
                }),
        ),
        _ => button.label(label),
    }
}
