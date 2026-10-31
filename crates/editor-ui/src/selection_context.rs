use crate::transform_panel::{TransformDraftGuard, can_begin_transform};
use crate::{UiHost, UiIcon};
use dioxus::prelude::*;
use nyatidraw_api::{EditCommand, EditorCommand, RasterEditTarget, TransformCommand, UiProjection};

fn dispatch(host: &UiHost, command: EditCommand, mut error: Signal<Option<String>>) {
    crate::focus_editor_shortcuts();
    error.set(
        host.push_ui_editor_command(EditorCommand::Edit(command))
            .err(),
    );
}

#[component]
pub fn SelectionContext(ui_projection: Signal<UiProjection>) -> Element {
    let host = use_context::<UiHost>();
    let apply_host = host.clone();
    let cancel_host = host.clone();
    let copy_host = host.clone();
    let clear_host = host.clone();
    let TransformDraftGuard(dirty) = use_context();
    let mut error = use_signal(|| None::<String>);
    let state = ui_projection.read();
    if !state.edit.has_selection && state.edit.transform.is_none() {
        return rsx! {};
    }
    let busy = state.edit.busy || state.edit.color_adjustment.is_some() || state.animation.playing;
    let can_begin = can_begin_transform(&state);
    let can_copy = !busy
        && state.edit.selected_pixels > 0
        && !matches!(state.raster_target, Some(RasterEditTarget::Mask { .. }))
        && host.supports_command(&EditorCommand::Edit(EditCommand::CopySelection));
    let transform = state.edit.transform.clone();
    let can_apply = transform.as_ref().is_some_and(|value| value.can_commit) && !busy && !dirty();
    let can_cancel = state.edit.can_cancel && !busy;
    rsx! {
        div { class: "selection-context", role: "group", aria_label: "현재 선택과 변형",
            span { class: "selection-context-status", role: "status",
                if transform.is_some() { "변형 미리보기" }
                else { "선택 영역 안에 그리는 중" }
            }
            div { class: "selection-context-actions",
                if transform.is_some() {
                    button { disabled: !can_apply,
                        title: if dirty() { "변형 입력을 미리보기로 확인한 뒤 적용하세요." } else { "변형 적용 (Enter)" },
                        onclick: move |_| {
                            let current = ui_projection.peek();
                            if current.edit.busy || *dirty.peek() {
                                error.set(Some("변형 입력을 미리보기로 확인한 뒤 적용하세요.".into()));
                                return;
                            }
                            let generation = current.edit.transform.as_ref().filter(|value| value.can_commit).map(|value| value.generation);
                            drop(current);
                            if let Some(generation) = generation {
                                dispatch(&apply_host, EditCommand::FreeTransform(TransformCommand::Commit { generation }), error);
                            }
                        },
                        UiIcon { name: "transform" } span { "적용" }
                    }
                    button { disabled: !can_cancel, title: "변형 전 그림으로 되돌리기 (Esc)",
                        onclick: move |_| dispatch(&cancel_host, EditCommand::FreeTransform(TransformCommand::Cancel), error),
                        UiIcon { name: "close" } span { "취소" }
                    }
                } else {
                    button { disabled: !can_begin, title: "선택한 그림 변형",
                        onclick: move |_| dispatch(&host, EditCommand::FreeTransform(TransformCommand::Begin), error),
                        UiIcon { name: "transform" } span { "변형" }
                    }
                    button { disabled: !can_copy, title: "선택한 그림 복사 (Ctrl+C)",
                        onclick: move |_| dispatch(&copy_host, EditCommand::CopySelection, error),
                        UiIcon { name: "copy" } span { "복사" }
                    }
                    button { disabled: busy, title: "그림은 유지하고 선택 해제 (Esc)",
                        onclick: move |_| dispatch(&clear_host, EditCommand::ClearSelection, error),
                        UiIcon { name: "deselect" } span { "해제" } kbd { "Esc" }
                    }
                }
            }
            if let Some(message) = error() { span { class: "selection-context-error", role: "alert", "{message}" } }
        }
    }
}
