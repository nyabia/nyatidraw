use crate::UiHost;
use dioxus::prelude::*;
use nyatidraw_api::{AffineTransform, EditCommand, EditorCommand, TransformCommand, UiProjection};

#[derive(Clone, Copy)]
pub struct TransformPanelOpen(pub Signal<bool>);

#[derive(Clone, Copy)]
pub struct TransformDraftGuard(pub Signal<bool>);

#[derive(Clone, Copy)]
struct PendingPreview {
    transform: AffineTransform,
    event_sequence: u64,
    project_epoch: u64,
}

pub fn can_begin_transform(projection: &UiProjection) -> bool {
    !projection.edit.busy
        && projection.edit.transform.is_none()
        && projection.edit.color_adjustment.is_none()
        && !projection.animation.playing
        && !matches!(
            projection.raster_target,
            Some(nyatidraw_api::RasterEditTarget::Mask { .. })
        )
        && projection.active_layer.is_some_and(|id| {
            projection.layers.iter().any(|layer| {
                layer.id == nyatidraw_api::LayerTreeNodeId::Raster(id)
                    && !layer.locked
                    && !layer.alpha_locked
            })
        })
}

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
    bridge: &UiHost,
    _projection: Signal<UiProjection>,
    command: TransformCommand,
    mut error: Signal<Option<String>>,
) {
    if !matches!(command, TransformCommand::Preview(_)) {
        crate::focus_editor_shortcuts();
    }
    let result =
        bridge.push_ui_editor_command(EditorCommand::Edit(EditCommand::FreeTransform(command)));
    error.set(result.err());
}

fn preview_inputs(
    bridge: &UiHost,
    projection: Signal<UiProjection>,
    inputs: Signal<TransformInputs>,
    mut dirty: Signal<bool>,
    mut pending: Signal<Option<PendingPreview>>,
    mut error: Signal<Option<String>>,
) {
    if !*dirty.peek() || projection.peek().edit.busy {
        return;
    }
    let value = match inputs.peek().parse() {
        Ok(value) => value,
        Err(message) => {
            error.set(Some(message.into()));
            return;
        }
    };
    if pending.peek().is_some() {
        return;
    }
    if projection
        .peek()
        .edit
        .transform
        .as_ref()
        .is_some_and(|current| current.can_commit && current.transform == value)
    {
        dirty.set(false);
        pending.set(None);
        error.set(None);
        return;
    }
    let event = bridge.protocol_snapshot().1;
    pending.set(Some(PendingPreview {
        transform: value,
        event_sequence: event.map_or(0, |event| event.sequence),
        project_epoch: bridge.project_epoch(),
    }));
    let result = bridge.push_ui_editor_command(EditorCommand::Edit(EditCommand::FreeTransform(
        TransformCommand::Preview(value),
    )));
    error.set(result.err());
    if error.peek().is_some() {
        pending.set(None);
    }
}

#[component]
pub fn TransformPanel(ui_projection: Signal<UiProjection>) -> Element {
    let bridge = use_context::<UiHost>();
    let begin_bridge = bridge.clone();
    let preview_bridge = bridge.clone();
    let commit_bridge = bridge.clone();
    let cancel_bridge = bridge.clone();
    let keyboard_bridge = bridge.clone();
    let TransformPanelOpen(mut opened) = use_context();
    let mut inputs = use_signal(|| TransformInputs::from(AffineTransform::default()));
    let TransformDraftGuard(mut dirty) = use_context();
    let crate::host::HostUpdate(host_update) = use_context();
    let mut pending = use_signal(|| None::<PendingPreview>);
    let mut error = use_signal(|| None::<String>);
    let mut previous = use_signal(|| None::<nyatidraw_api::TransformProjection>);
    let preview = use_callback(move |()| {
        preview_inputs(
            &preview_bridge,
            ui_projection,
            inputs,
            dirty,
            pending,
            error,
        );
    });
    use_effect(move || {
        let _ = host_update.read();
        let state = ui_projection.read();
        let current = state.edit.transform.clone();
        let request = *pending.peek();
        if let Some(request) = request {
            if begin_bridge.project_epoch() != request.project_epoch {
                pending.set(None);
                previous.set(None);
                dirty.set(false);
                error.set(None);
            } else if let Some(event) = begin_bridge
                .protocol_snapshot()
                .1
                .filter(|event| event.sequence > request.event_sequence)
                && let nyatidraw_api::EditorEvent::CommandRejected { reason, .. } = event.event
            {
                pending.set(None);
                error.set(Some(crate::reject_label(reason).to_owned()));
                return;
            }
        }
        if let Some(transform) = current.as_ref() {
            if previous.peek().is_none() {
                crate::send_dock_command(
                    &begin_bridge,
                    nyatidraw_api::DockCommand::ActivatePanel(
                        nyatidraw_api::PanelKind::ToolProperties,
                    ),
                );
            }
            let changed = previous.peek().as_ref().is_none_or(|last| {
                last.generation != transform.generation || last.transform != transform.transform
            });
            let preview_ready = !state.edit.busy
                && transform.can_commit
                && pending
                    .peek()
                    .as_ref()
                    .is_some_and(|request| request.transform == transform.transform);
            let acknowledged =
                preview_ready && inputs.peek().parse().ok() == Some(transform.transform);
            if preview_ready {
                pending.set(None);
            }
            if previous.peek().is_none() || acknowledged || (changed && !*dirty.peek()) {
                inputs.set(TransformInputs::from(transform.transform));
                dirty.set(false);
                pending.set(None);
                error.set(None);
            }
            if !state.edit.busy && pending.peek().is_some() && state.edit.error.is_some() {
                pending.set(None);
                error.set(state.edit.error.clone());
            }
            previous.set(current);
        } else if previous.peek().is_some() {
            // Admission is not completion. Only the authoritative removal closes this panel.
            opened.set(false);
            previous.set(None);
            inputs.set(TransformInputs::from(AffineTransform::default()));
            dirty.set(false);
            pending.set(None);
        }
    });
    let current = ui_projection.read();
    let transform = current.edit.transform.as_ref();
    let busy = current.edit.busy;
    let mask_selected = matches!(
        current.raster_target,
        Some(nyatidraw_api::RasterEditTarget::Mask { .. })
    );
    let can_cancel = current.edit.can_cancel;
    let has_selection = current.edit.has_selection;
    let active = transform.is_some();
    let can_begin = can_begin_transform(&current);
    let can_commit = transform.is_some_and(|value| value.can_commit) && !busy && !dirty();
    let generation = transform.map_or(0, |value| value.generation);
    let worker_error = current.edit.error.clone();
    let values = inputs.read().clone();
    let commit_hint = if dirty() {
        "입력한 값을 먼저 미리보기로 확인하세요 (Enter)"
    } else if can_commit {
        "미리보기 확정 (Enter)"
    } else {
        "그림을 이동하거나 배율·회전을 변경하세요"
    };
    rsx! {
        section { class: "transform-properties", aria_label: "자유 변형",
            if active {
            div { class: "transform-grid", onkeydown: move |event| {
                event.stop_propagation();
                match event.key() {
                    Key::Escape if can_cancel => {
                        event.prevent_default();
                        send(&keyboard_bridge, ui_projection, TransformCommand::Cancel, error);
                    }
                    Key::Enter if !busy && !event.is_auto_repeating() => {
                        event.prevent_default();
                        if dirty() {
                            preview.call(());
                        } else if can_commit {
                            send(&keyboard_bridge, ui_projection, TransformCommand::Commit { generation }, error);
                        }
                    }
                    _ => {}
                }
            },
            label { "X (px)"
                input { r#type: "number", step: "0.001", value: values.x, disabled: !active || busy,
                    aria_label: "가로 이동", oninput: move |e| { inputs.write().x = e.value(); dirty.set(true); }, onblur: move |_| preview.call(()) }
            }
            label { "Y (px)"
                input { r#type: "number", step: "0.001", value: values.y, disabled: !active || busy,
                    aria_label: "세로 이동", oninput: move |e| { inputs.write().y = e.value(); dirty.set(true); }, onblur: move |_| preview.call(()) }
            }
            label { "폭 (%)"
                input { r#type: "number", min: "0.0001", step: "0.1", value: values.width, disabled: !active || busy,
                    aria_label: "가로 배율", oninput: move |e| { inputs.write().width = e.value(); dirty.set(true); }, onblur: move |_| preview.call(()) }
            }
            label { "높이 (%)"
                input { r#type: "number", min: "0.0001", step: "0.1", value: values.height, disabled: !active || busy,
                    aria_label: "세로 배율", oninput: move |e| { inputs.write().height = e.value(); dirty.set(true); }, onblur: move |_| preview.call(()) }
            }
            label { "회전 (°)"
                input { r#type: "number", step: "0.1", value: values.angle, disabled: !active || busy,
                    aria_label: "변형 회전", oninput: move |e| { inputs.write().angle = e.value(); dirty.set(true); }, onblur: move |_| preview.call(()) }
            }
            label { class: "transform-check",
                input { r#type: "checkbox", checked: values.flip_x, disabled: !active || busy,
                    onchange: move |e| { inputs.write().flip_x = e.checked(); dirty.set(true); preview.call(()); } } "좌우 반전"
            }
            label { class: "transform-check",
                input { r#type: "checkbox", checked: values.flip_y, disabled: !active || busy,
                    onchange: move |e| { inputs.write().flip_y = e.checked(); dirty.set(true); preview.call(()); } } "상하 반전"
            }
            }
            }
            if mask_selected { crate::layer_mask::MaskImageTargetSwitch { ui_projection } }
            if !active && !has_selection && !mask_selected {
                p { class: "transform-help", "현재 레이어의 그림 전체를 변형합니다. 일부만 바꾸려면 먼저 영역을 선택하세요." }
            }
            if active {
                p { class: "transform-help", role: "status",
                    if dirty() { "입력값 확인이 필요합니다. 입력을 마치거나 Enter로 미리본 뒤 확정하세요. 도구 전환은 확인 후 가능합니다." }
                    else { "끌어서 이동 · 모서리로 크기 조절 · 손을 떼어도 미리보기 유지 · 확정 또는 취소" }
                }
            }
            div { class: "transform-actions",
            if !active {
                button { disabled: !can_begin, onclick: move |_| send(&bridge, ui_projection, TransformCommand::Begin, error), "변형 시작" }
            } else {
            button { disabled: !active || busy, title: "입력한 이동·배율·회전 미리보기 (Enter)", onclick: move |_| {
                preview.call(());
            }, "미리보기" }
            button { disabled: !can_commit, title: commit_hint, onclick: move |_| {
                send(&commit_bridge, ui_projection, TransformCommand::Commit { generation }, error);
            }, "확정" }
            }
            if active || can_cancel {
            button { disabled: !can_cancel, title: "변형 취소 (Esc)", onclick: move |_| {
                send(&cancel_bridge, ui_projection, TransformCommand::Cancel, error);
            }, "취소" }
            }
            }
            if let Some(message) = error().or(worker_error) { p { role: "alert", "{message}" } }
        }
    }
}
