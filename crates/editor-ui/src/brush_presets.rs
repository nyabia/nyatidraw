use crate::UiHost;
use dioxus::prelude::*;
use nyatidraw_api::{BrushPresetName, EditorCommand, Revision, ToolCommand, UiProjection};

fn send_preset_command(
    host: &UiHost,
    based_on: Revision,
    command: ToolCommand,
    mut error: Signal<Option<String>>,
) {
    error.set(
        host.push_editor_command(based_on, EditorCommand::Tool(command))
            .err(),
    );
}

#[component]
pub(super) fn BrushPresets(ui_projection: Signal<UiProjection>) -> Element {
    let host = use_context::<UiHost>();
    let error = use_signal(|| None::<String>);
    let mut selected = use_signal(|| None::<BrushPresetName>);
    let mut draft = use_signal(String::new);
    let mut confirm = use_signal(|| None::<(BrushPresetName, bool)>);
    let mut managing = use_signal(|| false);
    let projection = ui_projection.read();
    let presets = projection.brush_presets.clone();
    let writable = projection.brush_presets_writable;
    let tool = projection.drawing_tool;
    let based_on = projection.revision;
    let current = nyatidraw_api::BrushPresetValues::from_projection(&projection);
    let active_name = presets
        .iter()
        .find(|p| Some(p.name) == selected() && p.values.matches(current))
        .or_else(|| presets.iter().find(|p| p.values.matches(current)))
        .map(|p| p.name);
    let selected_index = presets
        .iter()
        .position(|p| Some(p.name) == selected() && p.values.tool == tool)
        .and_then(|i| u8::try_from(i).ok());
    let name = BrushPresetName::new(&draft());
    let save_host = host.clone();
    let rename_host = host.clone();
    rsx! {
        div { class: "brush-presets",
            div { class: "saved-brush-list",
                for (index, preset) in presets.iter().copied().enumerate().filter(|(_, p)| p.values.tool == tool) {
                    button {
                        class: if Some(preset.name) == active_name { "subtool selected" } else { "subtool" },
                        aria_pressed: Some(preset.name) == active_name,
                        title: "저장된 도구와 브러시 설정 적용",
                        onclick: { let host = host.clone(); move |_| {
                            send_preset_command(&host, based_on, ToolCommand::ApplyBrushPreset(u8::try_from(index).unwrap_or_default()), error);
                            selected.set(Some(preset.name));
                            draft.set(preset.name.as_str().to_owned());
                            confirm.set(None);
                        } },
                        "{preset.name.as_str()}"
                    }
                }
            }
            div { class: "brush-preset-shortcuts",
                button { title: "현재 브러시를 새 프리셋으로 저장", aria_label: "브러시 프리셋 추가", disabled: !writable,
                    onclick: move |_| { managing.set(true); selected.set(None); draft.set(String::new()); confirm.set(None); }, "+" }
                button { title: "브러시 프리셋 관리", aria_label: "브러시 프리셋 관리", disabled: !writable,
                    onclick: move |_| { managing.set(!managing()); confirm.set(None); }, "…" }
            }
            if managing() {
            input {
                value: "{draft}", placeholder: "프리셋 이름", aria_label: "브러시 프리셋 이름",
                disabled: !writable,
                onkeydown: move |event| event.stop_propagation(),
                oninput: move |event| { draft.set(event.value()); confirm.set(None); },
            }
            if !draft().is_empty() && name.is_none() { small { role: "status", "이름이 비어 있거나 너무 깁니다." } }
            div { class: "brush-preset-actions",
                button { disabled: !writable || name.is_none(), onclick: move |_| {
                    if let Some(name) = name { send_preset_command(&save_host, based_on, ToolCommand::SaveBrushPreset(name), error); selected.set(Some(name)); confirm.set(None); }
                }, "새로 저장" }
                button { disabled: !writable || selected_index.is_none() || name.is_none(), onclick: move |_| {
                    if let (Some(index), Some(name)) = (selected_index, name) {
                        send_preset_command(&rename_host, based_on, ToolCommand::RenameBrushPreset(index, name), error);
                        selected.set(Some(name)); confirm.set(None);
                    }
                }, "이름 변경" }
                button { disabled: !writable || selected_index.is_none(), onclick: move |_| {
                    confirm.set(selected().map(|name| (name, false)));
                }, "현재 설정으로 갱신…" }
                button { disabled: !writable || selected_index.is_none(), onclick: move |_| {
                    confirm.set(selected().map(|name| (name, true)));
                }, "삭제…" }
            }
            if let Some((target_name, deleting)) = confirm() {
                div { class: "brush-preset-confirm",
                    small { if deleting { "선택한 프리셋을 삭제할까요?" } else { "저장된 프리셋을 현재 설정으로 바꿀까요?" } }
                    button { onclick: move |_| {
                        let latest = ui_projection.read();
                        if let Some(index) = latest.brush_presets.iter().position(|p| p.name == target_name && p.values.tool == latest.drawing_tool).and_then(|i| u8::try_from(i).ok()) {
                            let command = if deleting { ToolCommand::DeleteBrushPreset(index) } else { ToolCommand::UpdateBrushPreset(index) };
                            send_preset_command(&host, latest.revision, command, error);
                        }
                        confirm.set(None);
                    }, "확인" }
                    button { onclick: move |_| confirm.set(None), "취소" }
                }
            }
            }
            if !writable { small { "원본 프리셋을 보존하여 저장을 중단했습니다." } }
            if let Some(message) = error() { small { role: "alert", "{message}" } }
        }
    }
}
