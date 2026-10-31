use dioxus::prelude::*;
use nyatidraw_api::{AnimationCommand, EditorCommand, EditorEvent, FrameId, UiProjection};
use std::sync::Arc;

use crate::{UiHost, UiIcon, preview::FrameThumbnailFrame};

#[derive(Clone, Copy)]
pub(crate) struct AnimationPanelVisible(pub Signal<bool>);

#[component]
pub fn StartAnimation(ui_projection: Signal<UiProjection>, onstarted: EventHandler<()>) -> Element {
    let host = use_context::<UiHost>();
    let AnimationPanelVisible(mut visible) = use_context();
    let mut error = use_signal(|| None::<String>);
    let state = ui_projection.read();
    if !state.animation.frames.is_empty() {
        return rsx! {
            button { class: "command", title: "애니메이션 타임라인 열기", onclick: move |_| {
                visible.set(true); onstarted.call(()); crate::focus_editor_shortcuts();
            }, UiIcon { name: "timeline" } "타임라인 열기" }
        };
    }
    let disabled = state.edit.busy
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some()
        || !host.supports_command(&EditorCommand::Animation(AnimationCommand::Enable));
    rsx! {
        button { class: "command", disabled, onclick: move |_| {
            match host.push_ui_editor_command(EditorCommand::Animation(AnimationCommand::Enable)) {
                Ok(()) => { visible.set(true); onstarted.call(()); crate::focus_editor_shortcuts(); }, Err(message) => error.set(Some(message)),
            }
        }, "애니메이션 시작" }
        if let Some(message) = error.read().as_ref() { span { role: "alert", "{message}" } }
    }
}

#[component]
pub fn AnimationPanel(ui_projection: Signal<UiProjection>, host_update: u64) -> Element {
    let mut collapsed = use_signal(|| false);
    let state = ui_projection.read();
    let animation = state.animation.clone();
    let Some(active) = animation.active_frame else {
        return rsx! {};
    };
    if animation.frames.is_empty() {
        return rsx! {};
    }
    let busy =
        state.edit.busy || state.edit.transform.is_some() || state.edit.color_adjustment.is_some();
    let editing_disabled = busy || animation.playing;
    let index = animation
        .frames
        .iter()
        .position(|frame| frame.id == active)
        .unwrap_or(0);
    let duration = animation.frames[index].duration_ms;
    let length = animation.frames.len();
    let play_icon = if animation.playing { "stop" } else { "play" };
    let play_label = if animation.playing {
        "재생 정지"
    } else {
        "애니메이션 재생"
    };
    let collapse_icon = if collapsed() {
        "chevron-right"
    } else {
        "chevron-down"
    };
    rsx! {
        section { id: "animation-timeline", class: "animation-panel", aria_label: "애니메이션 타임라인", "data-collapsed": "{collapsed}",
            div { class: "animation-controls",
                div { class: "animation-actions",
                button { class: "command compact", title: "프레임 목록 접기/펼치기", aria_label: "프레임 목록 접기/펼치기", aria_expanded: !collapsed(),
                    onclick: move |_| collapsed.toggle(), UiIcon { name: collapse_icon } }
                span { class: "animation-title", "프레임" }
                AnimationButton { command: if animation.playing { AnimationCommand::Stop } else { AnimationCommand::Play },
                    icon: play_icon, label: play_label, disabled: busy }
                AnimationButton { command: AnimationCommand::SetLoop(!animation.looping), icon: "repeat", label: "반복 재생", pressed: animation.looping, disabled: busy }
                AnimationButton { command: AnimationCommand::SetOnionSkin(!animation.onion_skin), icon: "layers", label: "이전·다음 프레임 겹쳐 보기", pressed: animation.onion_skin, disabled: editing_disabled }
                span { class: "mini-divider" }
                AnimationButton { command: AnimationCommand::InsertBlank { after: active }, icon: "plus", label: "빈 프레임 추가", disabled: editing_disabled }
                AnimationButton { command: AnimationCommand::Duplicate(active), icon: "copy", label: "현재 프레임 복제", disabled: editing_disabled }
                AnimationButton { command: AnimationCommand::Delete(active), icon: "trash", label: "현재 프레임 삭제", disabled: editing_disabled || length <= 1 }
                span { class: "mini-divider" }
                AnimationButton { command: AnimationCommand::Move { frame: active, index: u16::try_from(index.saturating_sub(1)).unwrap_or(0) }, icon: "arrow-left", label: "프레임 순서 앞으로", disabled: editing_disabled || index == 0 }
                AnimationButton { command: AnimationCommand::Move { frame: active, index: u16::try_from(index + 1).unwrap_or(u16::MAX) }, icon: "arrow-right", label: "프레임 순서 뒤로", disabled: editing_disabled || index + 1 >= length }
                FrameDuration { frame: active, duration, disabled: editing_disabled }
                span { class: "animation-counter", "{index + 1} / {length}" }
                }
                TimelineVisibilityButton { ui_projection, host_update, close_only: true }
            }
            if !collapsed() {
                FrameList { frames: animation.frames, active, disabled: editing_disabled, host_update }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PendingTimelineClose {
    project_epoch: u64,
    event_sequence: u64,
}

#[component]
pub(super) fn TimelineVisibilityButton(
    ui_projection: Signal<UiProjection>,
    host_update: u64,
    #[props(default)] close_only: bool,
) -> Element {
    let host = use_context::<UiHost>();
    let AnimationPanelVisible(mut visible) = use_context();
    let mut pending = use_signal(|| None::<PendingTimelineClose>);
    let mut error = use_signal(|| None::<String>);
    let monitor = host.clone();
    use_effect(use_reactive((&host_update,), move |_| {
        let _ = ui_projection.read().revision;
        let Some(request) = pending() else {
            return;
        };
        if monitor.project_epoch() != request.project_epoch {
            pending.set(None);
            error.set(None);
            return;
        }
        let (projection, event) = monitor.protocol_snapshot();
        if let Some(event) = event.filter(|event| event.sequence > request.event_sequence)
            && let EditorEvent::CommandRejected { reason, .. } = event.event
        {
            pending.set(None);
            error.set(Some(crate::reject_label(reason).to_owned()));
        } else if !projection.animation.playing {
            pending.set(None);
            crate::focus_editor_shortcuts();
            visible.set(false);
        }
    }));
    let waiting = pending().is_some();
    let showing = visible();
    let icon = if waiting {
        "stop"
    } else if close_only {
        "close"
    } else {
        "timeline"
    };
    let label = if waiting {
        "재생 정지 확인 중"
    } else if showing {
        "타임라인 닫기"
    } else {
        "타임라인 열기"
    };
    let class = if close_only {
        "command compact animation-close"
    } else {
        "command compact"
    };
    rsx! {
        button { class, disabled: waiting,
            aria_label: label, aria_expanded: showing, aria_controls: "animation-timeline",
            title: if showing { "재생을 정지하고 타임라인 닫기 · 프레임 유지" } else { "타임라인 열기" },
            onclick: move |_| {
                crate::focus_editor_shortcuts();
                error.set(None);
                if !visible() {
                    visible.set(true);
                    return;
                }
                let (projection, event) = host.protocol_snapshot();
                if !projection.animation.playing {
                    visible.set(false);
                    return;
                }
                let request = PendingTimelineClose {
                    project_epoch: host.project_epoch(),
                    event_sequence: event.map_or(0, |event| event.sequence),
                };
                match host.push_ui_editor_command(EditorCommand::Animation(AnimationCommand::Stop)) {
                    Ok(()) => {
                        pending.set(Some(request));
                        expire_pending_close(request, pending, error);
                    }
                    Err(message) => error.set(Some(message)),
                }
            },
            UiIcon { name: icon }
        }
        if let Some(message) = error() {
            aside { class: "status-toast error", role: "alert",
                span { "타임라인을 닫지 못했습니다. {message}" }
                button { aria_label: "타임라인 오류 알림 닫기", onclick: move |_| error.set(None), "×" }
            }
        }
    }
}

fn expire_pending_close(
    request: PendingTimelineClose,
    mut pending: Signal<Option<PendingTimelineClose>>,
    mut error: Signal<Option<String>>,
) {
    spawn(async move {
        let mut timer =
            document::eval("await new Promise(r => setTimeout(r, 5000)); dioxus.send(true);");
        if timer.recv::<bool>().await.is_ok() && *pending.peek() == Some(request) {
            pending.set(None);
            error.set(Some(
                "재생 정지를 확인하지 못했습니다. 다시 시도하세요.".to_owned(),
            ));
        }
    });
}

#[component]
fn FrameList(
    frames: Vec<nyatidraw_api::AnimationFrameProjection>,
    active: FrameId,
    disabled: bool,
    host_update: u64,
) -> Element {
    let _ = host_update;
    let host = use_context::<UiHost>();
    use_frame_thumbnail_requests(host.clone());
    let epoch = host.project_epoch();
    let previews = host.frame_thumbnail_snapshot();
    let mut navigation_error = use_signal(|| None::<String>);
    let index = frames
        .iter()
        .position(|frame| frame.id == active)
        .unwrap_or(0);
    let length = frames.len();
    let navigation_frames = frames.clone();
    rsx! {
                div { class: "animation-frames", role: "group", aria_label: "프레임 목록",
                    "data-frame-preview-epoch": "{epoch}",
                    onkeydown: move |event| {
                        if disabled { return; }
                        let target = match event.key() {
                            Key::ArrowLeft => index.saturating_sub(1),
                            Key::ArrowRight => (index + 1).min(length - 1),
                            Key::Home => 0,
                            Key::End => length - 1,
                            _ => return,
                        };
                        event.prevent_default();
                        event.stop_propagation();
                        let frame = navigation_frames[target].id;
                        match host.push_ui_editor_command(EditorCommand::Animation(AnimationCommand::Select(frame))) {
                            Ok(()) => focus_frame(frame),
                            Err(message) => navigation_error.set(Some(message)),
                        }
                    },
                    for (index, frame) in frames.into_iter().enumerate() {
                        FrameButton { key: "{frame.id.0}", frame: frame.id, number: index + 1, duration: frame.duration_ms,
                            selected: frame.id == active, disabled: disabled,
                            preview: if previews.project_epoch == epoch { previews.frame(frame.id) } else { None } }
                    }
                }
        if let Some(message) = navigation_error.read().as_ref() { crate::TransientNotice { message: message.clone() } }
    }
}

fn use_frame_thumbnail_requests(host: UiHost) {
    let cleanup_host = host.clone();
    use_drop(move || cleanup_host.request_frame_thumbnails(&[]));
    use_effect(move || {
        let host = host.clone();
        spawn(async move {
            let mut events = document::eval(include_str!("frame_preview_requests.js"));
            while let Ok((epoch, frames)) = events.recv::<(String, Vec<String>)>().await {
                if epoch.parse::<u64>().ok() != Some(host.project_epoch()) {
                    continue;
                }
                let frames: Vec<_> = frames
                    .into_iter()
                    .take(24)
                    .filter_map(|frame| frame.parse::<u128>().ok().map(FrameId))
                    .collect();
                host.request_frame_thumbnails(&frames);
            }
        });
    });
}

fn focus_frame(frame: FrameId) {
    let _ = document::eval(&format!(
        "document.querySelector('[data-frame-id=\"{}\"]')?.focus({{ preventScroll: true }});",
        frame.0
    ));
}

#[component]
fn AnimationButton(
    command: AnimationCommand,
    icon: &'static str,
    label: &'static str,
    disabled: bool,
    #[props(default)] pressed: bool,
) -> Element {
    let host = use_context::<UiHost>();
    let mut error = use_signal(|| None::<String>);
    let disabled = disabled || !host.supports_command(&EditorCommand::Animation(command));
    rsx! {
        button { class: "command compact", title: label, aria_label: label, aria_pressed: pressed, disabled,
            onclick: move |_| {
                if let Err(message) = host.push_ui_editor_command(EditorCommand::Animation(command)) { error.set(Some(message)); }
            }, UiIcon { name: icon } }
        if let Some(message) = error.read().as_ref() { crate::TransientNotice { message: message.clone() } }
    }
}

#[component]
fn FrameButton(
    frame: FrameId,
    number: usize,
    duration: u32,
    selected: bool,
    disabled: bool,
    preview: Option<Arc<FrameThumbnailFrame>>,
) -> Element {
    let host = use_context::<UiHost>();
    let mut error = use_signal(|| None::<String>);
    use_effect(use_reactive((&selected,), move |(selected,)| {
        if selected {
            let _ = document::eval(&format!(
                "document.querySelector('[data-frame-id=\"{}\"]')?.scrollIntoView({{ block: 'nearest', inline: 'nearest' }});",
                frame.0
            ));
        }
    }));
    rsx! {
        button { class: "animation-frame", aria_label: "프레임 {number}", aria_pressed: selected, disabled,
            "data-frame-id": "{frame.0}", tabindex: if selected { "0" } else { "-1" },
            title: "프레임 {number} · {duration} ms · ←/→ 이동 · Home/End 처음/끝",
            onclick: move |_| {
                if let Err(message) = host.push_ui_editor_command(EditorCommand::Animation(AnimationCommand::Select(frame))) { error.set(Some(message)); }
            },
            span { class: "animation-frame-preview", "data-ready": "{preview.is_some()}", aria_hidden: "true",
                if let Some(preview) = &preview {
                    img { src: preview.data_uri.to_string(), width: "{preview.width}", height: "{preview.height}",
                        alt: "", draggable: "false" }
                } else { span { class: "animation-frame-pending", "···" } }
            }
            span { class: "animation-frame-label", strong { "{number}" } span { "{duration} ms" } }
        }
        if let Some(message) = error.read().as_ref() { crate::TransientNotice { message: message.clone() } }
    }
}

#[component]
fn FrameDuration(frame: FrameId, duration: u32, disabled: bool) -> Element {
    let host = use_context::<UiHost>();
    let mut value = use_signal(|| duration.to_string());
    let mut editing = use_signal(|| None::<FrameId>);
    let mut error = use_signal(|| None::<String>);
    use_effect(use_reactive(
        (&frame, &duration),
        move |(frame, duration)| {
            if editing.peek().is_none_or(|target| target != frame) {
                value.set(duration.to_string());
                editing.set(None);
            }
        },
    ));
    let commit = use_callback(move |()| {
        let target = editing();
        editing.set(None);
        if disabled || target != Some(frame) {
            value.set(duration.to_string());
            return;
        }
        let Ok(duration_ms) = value().trim().parse::<u32>() else {
            value.set(duration.to_string());
            return;
        };
        if !(1..=60000).contains(&duration_ms) {
            value.set(duration.to_string());
            return;
        }
        value.set(duration_ms.to_string());
        error.set(None);
        if duration_ms != duration
            && let Err(message) = host.push_ui_editor_command(EditorCommand::Animation(
                AnimationCommand::SetDuration { frame, duration_ms },
            ))
        {
            error.set(Some(message));
            value.set(duration.to_string());
        }
    });
    rsx! {
        label { class: "animation-duration", "시간"
            input { r#type: "number", min: "1", max: "60000", step: "10", value: "{value}", disabled,
                aria_label: "현재 프레임 시간", title: "1–60000 ms · Enter 확정 · Esc 취소",
                onkeydown: move |event| {
                    event.stop_propagation();
                    match event.key() {
                        Key::Enter => { event.prevent_default(); commit.call(()); crate::focus_editor_shortcuts(); }
                        Key::Escape => {
                            event.prevent_default(); editing.set(None); value.set(duration.to_string());
                            error.set(None); crate::focus_editor_shortcuts();
                        }
                        _ => {}
                    }
                },
                onfocus: move |_| editing.set(Some(frame)),
                oninput: move |event| value.set(event.value()),
                onblur: move |_| commit.call(()) }
            span { "ms" }
        }
        if let Some(message) = error.read().as_ref() { crate::TransientNotice { message: message.clone() } }
    }
}
