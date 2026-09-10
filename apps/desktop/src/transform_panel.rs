use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{AffineTransform, EditCommand, EditorCommand, TransformCommand, UiProjection};

#[derive(Clone, Copy)]
pub(crate) struct TransformPanelOpen(pub Signal<bool>);

#[derive(Clone)]
struct TransformInputs {
    x: String,
    y: String,
    width: String,
    height: String,
    angle: String,
    flip_x: bool,
    flip_y: bool,
}

impl From<AffineTransform> for TransformInputs {
    #[allow(clippy::cast_precision_loss)]
    fn from(value: AffineTransform) -> Self {
        Self {
            x: (value.offset_milli[0] as f64 / 1_000.0).to_string(),
            y: (value.offset_milli[1] as f64 / 1_000.0).to_string(),
            width: (f64::from(value.scale_ppm[0]) / 10_000.0).to_string(),
            height: (f64::from(value.scale_ppm[1]) / 10_000.0).to_string(),
            angle: (f64::from(value.rotation_millidegrees) / 1_000.0).to_string(),
            flip_x: value.flip_x,
            flip_y: value.flip_y,
        }
    }
}

impl TransformInputs {
    fn parse(&self) -> Result<AffineTransform, &'static str> {
        let offset = |text: &str| number(text, 1_000.0, -2_147_483_648_000.0, 2_147_483_647_000.0);
        let scale = |text: &str| {
            u32::try_from(number(text, 10_000.0, 1.0, f64::from(u32::MAX))?)
                .map_err(|_| "배율 범위를 확인하세요.")
        };
        Ok(AffineTransform {
            offset_milli: [offset(&self.x)?, offset(&self.y)?],
            scale_ppm: [scale(&self.width)?, scale(&self.height)?],
            rotation_millidegrees: i32::try_from(number(
                &self.angle,
                1_000.0,
                f64::from(i32::MIN),
                f64::from(i32::MAX),
            )?)
            .map_err(|_| "회전 범위를 확인하세요.")?,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
        })
    }
}

// UI conversion only: artwork geometry and allocation limits are enforced by the worker.
#[allow(clippy::cast_possible_truncation)]
fn number(text: &str, factor: f64, min: f64, max: f64) -> Result<i64, &'static str> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| "숫자를 입력하세요.")?
        * factor;
    let value = value.round();
    if !value.is_finite() || value < min || value > max {
        return Err("이동·배율·회전 범위를 확인하세요. 배율은 0보다 커야 합니다.");
    }
    Ok(value as i64)
}

fn send(
    bridge: &LiveInkBridge,
    _projection: Signal<UiProjection>,
    command: TransformCommand,
    mut error: Signal<Option<String>>,
) {
    let result =
        bridge.push_ui_editor_command(EditorCommand::Edit(EditCommand::FreeTransform(command)));
    error.set(
        result
            .err()
            .map(|_| "현재 작업이 끝난 뒤 다시 시도하세요.".to_owned()),
    );
}

#[component]
pub(crate) fn TransformPanel(ui_projection: Signal<UiProjection>) -> Element {
    let bridge = use_context::<LiveInkBridge>();
    let begin_bridge = bridge.clone();
    let escape_bridge = bridge.clone();
    let preview_bridge = bridge.clone();
    let commit_bridge = bridge.clone();
    let cancel_bridge = bridge.clone();
    let TransformPanelOpen(mut opened) = use_context();
    let mut inputs = use_signal(|| TransformInputs::from(AffineTransform::default()));
    let mut dirty = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut started = use_signal(|| false);
    let mut previous = use_signal(|| None::<nyatidraw_api::TransformProjection>);
    use_effect(move || {
        let current = ui_projection.read().edit.transform.clone();
        if let Some(transform) = current.as_ref() {
            if previous.peek().is_none() {
                crate::send_dock_command(
                    &begin_bridge,
                    nyatidraw_api::DockCommand::ActivatePanel(
                        nyatidraw_api::PanelKind::ToolProperties,
                    ),
                );
            }
            if previous.peek().as_ref().is_none_or(|last| {
                last.generation != transform.generation || last.transform != transform.transform
            }) {
                inputs.set(TransformInputs::from(transform.transform));
                dirty.set(false);
                error.set(None);
            }
            previous.set(current);
            started.set(true);
        } else if previous.peek().is_some() {
            // Admission is not completion. Only the authoritative removal closes this panel.
            opened.set(false);
        } else if !*started.peek() {
            started.set(true);
            send(&begin_bridge, ui_projection, TransformCommand::Begin, error);
        }
    });
    let current = ui_projection.read();
    let transform = current.edit.transform.as_ref();
    let busy = current.edit.busy;
    let active = transform.is_some();
    let can_commit = transform.is_some_and(|value| value.can_commit) && !busy && !dirty();
    let generation = transform.map_or(0, |value| value.generation);
    let worker_error = current.edit.error.clone();
    let values = inputs.read().clone();
    rsx! {
        section { class: "transform-properties", aria_label: "자유 변형",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape {
                    event.prevent_default();
                    send(&escape_bridge, ui_projection, TransformCommand::Cancel, error);
                    if !busy && !active { opened.set(false); }
                }
            },
            onkeyup: move |event| event.stop_propagation(),
            h3 { "변형" }
            label { "X (px)"
                input { r#type: "number", step: "0.001", value: values.x, disabled: !active || busy,
                    aria_label: "가로 이동", oninput: move |e| { inputs.write().x = e.value(); dirty.set(true); } }
            }
            label { "Y (px)"
                input { r#type: "number", step: "0.001", value: values.y, disabled: !active || busy,
                    aria_label: "세로 이동", oninput: move |e| { inputs.write().y = e.value(); dirty.set(true); } }
            }
            label { "폭 (%)"
                input { r#type: "number", min: "0.0001", step: "0.1", value: values.width, disabled: !active || busy,
                    aria_label: "가로 배율", oninput: move |e| { inputs.write().width = e.value(); dirty.set(true); } }
            }
            label { "높이 (%)"
                input { r#type: "number", min: "0.0001", step: "0.1", value: values.height, disabled: !active || busy,
                    aria_label: "세로 배율", oninput: move |e| { inputs.write().height = e.value(); dirty.set(true); } }
            }
            label { "회전 (°)"
                input { r#type: "number", step: "0.1", value: values.angle, disabled: !active || busy,
                    aria_label: "변형 회전", oninput: move |e| { inputs.write().angle = e.value(); dirty.set(true); } }
            }
            label { class: "transform-check",
                input { r#type: "checkbox", checked: values.flip_x, disabled: !active || busy,
                    onchange: move |e| { inputs.write().flip_x = e.checked(); dirty.set(true); } } "좌우 반전"
            }
            label { class: "transform-check",
                input { r#type: "checkbox", checked: values.flip_y, disabled: !active || busy,
                    onchange: move |e| { inputs.write().flip_y = e.checked(); dirty.set(true); } } "상하 반전"
            }
            if !active {
                button { disabled: busy, onclick: move |_| send(&bridge, ui_projection, TransformCommand::Begin, error), "시작" }
            }
            button { disabled: !active || busy, onclick: move |_| {
                match inputs.read().parse() {
                    Ok(value) => send(&preview_bridge, ui_projection, TransformCommand::Preview(value), error),
                    Err(message) => error.set(Some(message.to_owned())),
                }
            }, "미리보기" }
            button { disabled: !can_commit, onclick: move |_| {
                send(&commit_bridge, ui_projection, TransformCommand::Commit { generation }, error);
            }, "확정" }
            button { disabled: busy, onclick: move |_| {
                send(&cancel_bridge, ui_projection, TransformCommand::Cancel, error);
                // With no draft, no authoritative removal can arrive (e.g. Begin rejected).
                if ui_projection.read().edit.transform.is_none() { opened.set(false); }
            }, "취소" }
            if let Some(message) = error().or(worker_error) { p { role: "alert", "{message}" } }
        }
    }
}
