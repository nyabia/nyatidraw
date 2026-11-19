use crate::canvas_size::{COMMON_PRESETS, CanvasDraft, MORE_PRESETS};

use crate::UiHost;
use dioxus::prelude::*;
use nyatidraw_api::{EditCommand, EditorCommand, UiProjection};

#[derive(Clone, Copy)]
pub struct PagePanelOpen(pub Signal<bool>);

#[derive(Clone, Copy)]
enum PageOutcome {
    Resize([u32; 2]),
    Crop,
}

#[derive(Clone)]
struct PendingPage {
    outcome: PageOutcome,
    project_epoch: u64,
    revision: nyatidraw_api::Revision,
    event_sequence: u64,
    error: Option<String>,
}

fn submit_page(
    host: &UiHost,
    command: EditCommand,
    outcome: PageOutcome,
    mut pending: Signal<Option<PendingPage>>,
    mut error: Signal<Option<String>>,
) {
    let (before, event) = host.protocol_snapshot();
    pending.set(Some(PendingPage {
        outcome,
        project_epoch: host.project_epoch(),
        revision: before.revision,
        event_sequence: event.map_or(0, |event| event.sequence),
        error: before.edit.error,
    }));
    error.set(None);
    if let Err(message) = host.push_editor_command(before.revision, EditorCommand::Edit(command)) {
        pending.set(None);
        error.set(Some(message));
    }
}

#[component]
pub fn PagePanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let crop_ink = live_ink.clone();
    let PagePanelOpen(mut opened) = use_context();
    let mut draft = use_signal(|| CanvasDraft::new(ui_projection.read().canvas));
    let mut appearance = use_signal(|| live_ink.workspace_appearance());
    let mut error = use_signal(|| None::<String>);
    let mut pending = use_signal(|| None::<PendingPage>);
    let crate::host::HostUpdate(host_update) = use_context();
    let completed_ink = live_ink.clone();
    use_effect(move || {
        let _ = host_update.read();
        let current = ui_projection.read();
        let Some(request) = pending.read().clone() else {
            return;
        };
        if completed_ink.project_epoch() != request.project_epoch {
            pending.set(None);
            opened.set(false);
            return;
        }
        let event = completed_ink.protocol_snapshot().1;
        if let Some(nyatidraw_api::EditorEvent::CommandRejected { reason, .. }) = event
            .filter(|event| event.sequence > request.event_sequence)
            .map(|event| event.event)
        {
            pending.set(None);
            error.set(Some(crate::reject_label(reason).to_owned()));
            return;
        }
        if current.edit.busy {
            return;
        }
        if let Some(message) = &current.edit.error
            && (current.revision != request.revision || current.edit.error != request.error)
        {
            pending.set(None);
            error.set(Some(message.clone()));
            return;
        }
        let complete = current.revision != request.revision
            && match request.outcome {
                PageOutcome::Resize(size) => {
                    size == [current.canvas.width_px, current.canvas.height_px]
                }
                PageOutcome::Crop => !current.edit.has_selection,
            };
        if complete {
            pending.set(None);
            completed_ink.set_workspace_appearance(appearance());
            opened.set(false);
        }
    });
    let current = ui_projection.read();
    let busy = current.edit.busy
        || pending.read().is_some()
        || current.edit.transform.is_some()
        || current.edit.color_adjustment.is_some()
        || current.animation.playing;
    let can_crop = current.edit.has_selection && !busy;
    let size = draft.read().current.size();
    let source = draft.read().current.source;
    rsx! {
        div { class: "page-backdrop", onclick: move |_| opened.set(false),
        section { id: "page-settings-dialog", class: "page-dialog transform-properties", role: "dialog", aria_modal: "true", aria_label: "캔버스",
            onmounted: move |_| crate::contain_overlay_focus("page-settings-dialog"),
            onclick: move |event| event.stop_propagation(),
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape { event.prevent_default(); opened.set(false); }
            },
            h3 { "캔버스" }
            fieldset { class: "canvas-size-settings", disabled: busy,
                div { class: "canvas-presets", role: "group", aria_label: "자주 쓰는 캔버스 프리셋",
                    for preset in COMMON_PRESETS {
                        button {
                            aria_pressed: source == Some((preset, 1)),
                            onclick: move |_| { draft.write().choose_preset(preset); error.set(None); },
                            strong { "{preset.name}" }
                            span { "{preset.ratio}" }
                            if preset.size[0] != preset.size[1] { small { "{preset.size[0]} × {preset.size[1]}" } }
                        }
                    }
                }
                select { class: "canvas-more-presets", aria_label: "캔버스 프리셋 더보기",
                    value: source.filter(|(preset, multiplier)| *multiplier == 1 && MORE_PRESETS.contains(preset)).map_or("", |(preset, _)| preset.name),
                    onchange: move |event| {
                        if let Some(&preset) = MORE_PRESETS.iter().find(|preset| preset.name == event.value()) {
                            draft.write().choose_preset(preset); error.set(None);
                        }
                    },
                    option {
                        value: "", disabled: true,
                        selected: !source.is_some_and(|(preset, multiplier)| multiplier == 1 && MORE_PRESETS.contains(&preset)),
                        "더보기…"
                    }
                    for preset in MORE_PRESETS {
                        option { value: preset.name, selected: source == Some((preset, 1)), "{preset.name} · {preset.ratio} ({preset.size[0]} × {preset.size[1]})" }
                    }
                }
                div { class: "canvas-multipliers", role: "group", aria_label: "현재 크기 조절",
                    button { disabled: !draft.read().can_undo(), title: "이 창에서 바꾼 크기만 한 단계 되돌립니다.",
                        onclick: move |_| { draft.write().undo(); error.set(None); },
                        crate::UiIcon { name: "undo" } "되돌리기"
                    }
                    for factor in [2, 3, 5] {
                        button {
                            disabled: draft.read().current.multiplied(factor).is_none(),
                            title: if draft.read().current.multiplied(factor).is_none() { "입력값이 잘못되었거나 현재 출력 한도를 초과합니다." }
                                else { "현재 가로·세로에 곱합니다. 여러 번 누르면 누적됩니다." },
                            onclick: move |_| { draft.write().multiply(factor); error.set(None); },
                            "×{factor}"
                        }
                    }
                }
                output { class: "canvas-size-source", aria_live: "polite", "{draft.read().current.label()}" }
                div { class: "transform-grid canvas-dimensions",
                    label { "가로 (px)"
                        input { r#type: "number", min: "1", step: "1", value: "{draft.read().current.width}", aria_label: "캔버스 가로",
                            oninput: move |event| { draft.write().type_dimension(true, event.value()); error.set(None); },
                            onblur: move |_| draft.write().finish_typing(),
                        }
                    }
                    button { class: "canvas-swap", aria_label: "가로·세로 교환", title: "가로·세로 교환 · 그림은 회전하지 않습니다.",
                        onclick: move |_| { draft.write().swap_axes(); error.set(None); },
                        crate::UiIcon { name: "swap-axes" }
                    }
                    label { "세로 (px)"
                        input { r#type: "number", min: "1", step: "1", value: "{draft.read().current.height}", aria_label: "캔버스 세로",
                            oninput: move |event| { draft.write().type_dimension(false, event.value()); error.set(None); },
                            onblur: move |_| draft.write().finish_typing(),
                        }
                    }
                }
            }
            p { class: "transform-help", "왼쪽 위 기준으로 출력 영역만 변경합니다. 그림과 바깥쪽 내용은 유지됩니다." }
            if let Err(message) = size { p { class: "canvas-size-error", role: "status", "{message}" } }
            fieldset { class: "canvas-background-settings", disabled: busy,
                legend { "바깥 배경" }
                div { class: "canvas-background-options", role: "group", aria_label: "캔버스 바깥 배경",
                    button { aria_pressed: appearance.read().checkerboard,
                        onclick: move |_| appearance.write().checkerboard = true,
                        span { class: "canvas-background-swatch canvas-background-checker", aria_hidden: "true" }
                        "체크무늬"
                    }
                    button { aria_pressed: !appearance.read().checkerboard,
                        onclick: move |_| appearance.write().checkerboard = false,
                        "단색"
                    }
                    input { r#type: "color", aria_label: "바깥 배경색", title: "바깥 배경색",
                        disabled: appearance.read().checkerboard,
                        value: appearance.read().solid_hex(),
                        oninput: move |event| appearance.write().set_solid_hex(&event.value()),
                    }
                }
                small { "화면에만 적용 · 그림과 출력은 그대로" }
            }
            div { class: "transform-actions",
                button { disabled: busy || size.is_err(), onclick: move |_| {
                    let Ok([w, h]) = draft.read().current.size() else { return; };
                    let canvas = ui_projection.read().canvas;
                    if [w, h] == [canvas.width_px, canvas.height_px] {
                        live_ink.set_workspace_appearance(appearance());
                        opened.set(false);
                        return;
                    }
                    submit_page(&live_ink, EditCommand::ResizePage { size: [w, h] }, PageOutcome::Resize([w, h]), pending, error);
                }, "적용" }
                button { disabled: !can_crop, title: "선택 경계에 캔버스를 맞춥니다. 바깥 그림도 보존하며 함께 이동합니다.", onclick: move |_| {
                    submit_page(&crop_ink, EditCommand::CropPageToSelection, PageOutcome::Crop, pending, error);
                }, "선택에 맞추기" }
                button { onclick: move |_| opened.set(false), "닫기" }
            }
            if pending.read().is_some() { p { role: "status", "캔버스 변경 확인 중… 닫아도 접수된 작업은 계속됩니다." } }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
        }
        }
    }
}
