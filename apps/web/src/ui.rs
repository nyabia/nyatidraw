use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;
use nyatidraw_api::UiProjection;
use nyatidraw_editor_ui::{EditorChrome, ExportStatus, UiBackend, UiHost, UiIcon, UiSlots};

use crate::{
    adapter::WebUiBackend,
    runtime::{Editor, EditorModal},
};

const HOST_STYLE: Asset = asset!("/assets/host.css");

#[component]
pub fn App() -> Element {
    let editor = Editor {
        runtime: use_signal(|| Rc::new(RefCell::new(None))),
        revision: use_signal(|| 0),
        status: use_signal(|| "WebGPU와 이전 작업을 준비하는 중…".to_owned()),
        ready: use_signal(|| false),
        modal: use_signal(|| None),
        notice: use_signal(|| None),
    };
    use_context_provider(|| editor);
    let backend = use_hook(|| WebUiBackend::new(editor));
    use_context_provider(|| UiHost(backend.clone()));
    use_context_provider(|| UiSlots {
        canvas: || rsx! { BrowserCanvas {} },
        file_buttons: |extended| rsx! { BrowserFileButtons { extended } },
        update_control: || rsx! { BrowserHelp {} },
    });
    let mut projection = use_signal(UiProjection::empty);
    let appearance_backend = backend.clone();
    use_effect(move || {
        if (editor.ready)() {
            appearance_backend.set_workspace_appearance(appearance_backend.workspace_appearance());
        }
    });
    use_effect(move || {
        let _revision = (editor.revision)();
        let _ready = (editor.ready)();
        projection.set(backend.protocol_snapshot().0);
    });
    use_future(move || async move {
        if let Err(error) = editor.initialize().await {
            editor.report(error);
        }
    });
    let modal = (editor.modal)();
    rsx! {
        document::Link { rel: "stylesheet", href: HOST_STYLE }
        document::Meta { name: "theme-color", content: "#242424" }
        div { class: "browser-editor-root", "inert": modal.map(|_| ""),
            EditorChrome { ui_projection: projection, export_status: ExportStatus::Idle, overlays: rsx! { BrowserNotice {} } }
        }
        BrowserDialog {}
    }
}

#[component]
fn BrowserCanvas() -> Element {
    let editor = use_context::<Editor>();
    let ready = (editor.ready)();
    let save_status = (editor.status)();
    rsx! {
        div { class: "browser-canvas-host",
            canvas { id: "drawing-canvas", tabindex: "0", aria_label: "그리기 캔버스. B 브러시 계열, E 지우개, C 스포이트, Space 또는 가운데 버튼으로 이동." }
            if ready {
                div { class: "browser-save-status", role: "status", "{save_status}" }
            }
            if !ready {
                section { class: "browser-startup", role: "status",
                    strong { "NyatiDraw Web" }
                    p { "{editor.status}" }
                    p { "WebGPU를 지원하는 최신 Chrome 또는 Edge를 이용해주세요." }
                    button { onclick: move |_| editor.download_recovery(), "저장된 작업 내려받기" }
                    a { href: "../", "홈페이지" }
                }
            }
        }
    }
}

#[component]
fn BrowserFileButtons(extended: bool) -> Element {
    let editor = use_context::<Editor>();
    let ready = (editor.ready)();
    rsx! {
        button { class: "command", title: "NTDR 작업 파일 또는 PNG 열기", disabled: !ready, onclick: move |_| editor.open(),
            UiIcon { name: "folder" } span { class: "command-label", "열기" }
        }
        if extended {
            button { class: "command", disabled: !ready, onclick: move |_| editor.show_modal(EditorModal::NewDocument), "새 그림" }
            button { class: "command", disabled: !ready, onclick: move |_| editor.download_project(), "작업 파일 저장" }
            button { class: "command", disabled: !ready, onclick: move |_| editor.download_png(), "PNG 내보내기" }
        }
    }
}

#[component]
fn BrowserHelp() -> Element {
    let editor = use_context::<Editor>();
    let save_status = (editor.status)();
    rsx! { button { class: "command", title: "{save_status}", onclick: move |_| editor.show_modal(EditorModal::Help), "웹판 안내" } }
}

#[component]
fn BrowserNotice() -> Element {
    let editor = use_context::<Editor>();
    rsx! {
        if let Some(message) = (editor.notice)() {
            aside { class: "browser-notice", role: "alert", p { "{message}" }
                button { title: "안내 닫기", aria_label: "안내 닫기", onclick: move |_| editor.dismiss_notice(), "×" }
            }
        }
    }
}

#[component]
fn BrowserDialog() -> Element {
    let editor = use_context::<Editor>();
    let modal = (editor.modal)();
    let title = match modal {
        Some(EditorModal::Help) => "NyatiDraw Web",
        Some(EditorModal::NewDocument) => "새 그림",
        _ => "현재 그림을 바꿀까요?",
    };
    rsx! {
        if modal.is_some() {
            div { class: "modal-shade browser-modal", onclick: move |_| editor.close_modal(),
                section { class: "browser-dialog", role: "dialog", tabindex: "-1", aria_modal: "true", aria_label: title,
                    onmounted: |_| crate::browser::focus_modal(),
                    onclick: |event| event.stop_propagation(),
                    onkeydown: move |event| { if event.key() == Key::Escape { event.prevent_default(); event.stop_propagation(); editor.close_modal(); } },
                    h1 { "{title}" }
                    if modal == Some(EditorModal::Help) {
                        p { "데스크톱과 같은 공용 UI를 사용하는 웹 실험판입니다. 아직 웹에 연결되지 않은 기능은 비활성화됩니다." }
                        p { role: "status", "{editor.status}" }
                        ul {
                            li { "그림은 서버로 전송하지 않고 이 브라우저에 자동 복구용으로 저장합니다. 중요한 작업은 파일로 내려받아 보관하세요." }
                            li { "작업 파일은 데스크톱과 같은 .ntdr 형식입니다. 웹에서 지원하지 않는 그룹이나 자원 한도를 넘는 파일은 내용 손실을 막기 위해 열지 않습니다. 이전 .nyatidraw-web 파일도 가져올 수 있습니다." }
                            li { "최대 4096 × 4096 캔버스, 32개 레이어. 되돌리기는 메모리 한도 내 최대 128개이며 다시 열 때 복원되지 않습니다." }
                            li { "도킹 위치 변경은 캔버스 수명 보장을 위해 현재 웹판에서 비활성화됩니다. 패널 크기 조절과 탭은 사용할 수 있습니다." }
                            li { "필압은 기기와 브라우저 지원에 따라 달라집니다. Windows 앱과 같은 성능을 검증한 상태는 아닙니다." }
                        }
                        button { "data-dialog-initial": "", onclick: move |_| editor.close_modal(), "닫기" }
                    } else if modal == Some(EditorModal::NewDocument) {
                        p { "투명한 레이어 하나로 시작합니다." }
                        for (name, width, height) in [("HD · 1280 × 720", 1280, 720), ("FHD · 1920 × 1080", 1920, 1080), ("정사각형 · 512 × 512", 512, 512), ("픽셀 작업 · 64 × 64", 64, 64)] {
                            button { class: "browser-preset", onclick: move |_| editor.new_document(width, height), "{name}" }
                        }
                        button { "data-dialog-initial": "", onclick: move |_| editor.close_modal(), "취소" }
                    } else {
                        p { "새 그림을 열면 현재 그림의 자동 복구 저장도 바뀝니다. 보관하려면 취소 후 작업 파일로 내려받아주세요." }
                        div { class: "browser-dialog-actions",
                            button { "data-dialog-initial": "", onclick: move |_| editor.close_modal(), "취소" }
                            button { onclick: move |_| editor.confirm_replacement(), "그림 바꾸기" }
                        }
                    }
                }
            }
        }
    }
}
