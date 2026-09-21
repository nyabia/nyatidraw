use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;
use nyatidraw_web_core::{BrushSettings, LayerNode, LayerTreeNode, WebDocument, WebTool};

use crate::runtime::{Editor, EditorModal};

const STYLE: Asset = asset!("/assets/editor.css");

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
    use_future(move || async move {
        if let Err(error) = editor.initialize().await {
            editor.report(error);
        }
    });
    let _revision = (editor.revision)();
    let ready = (editor.ready)();
    let tool = editor.read(|r| r.document.tool()).unwrap_or_default();
    let settings = editor
        .read(|r| r.document.brush_settings())
        .unwrap_or_else(|| BrushSettings::for_tool(tool));
    let color = editor
        .read(|r| color_hex(r.document.foreground()))
        .unwrap_or_else(|| "#000000".into());
    let recent = editor.read(|r| r.recent_colors.clone()).unwrap_or_default();
    let layers = use_memo(move || {
        let _revision = (editor.revision)();
        editor
            .read(|r| {
                r.document
                    .layers()
                    .root()
                    .children
                    .iter()
                    .rev()
                    .filter_map(|node| match node {
                        LayerTreeNode::Raster(layer) => Some((
                            layer.clone(),
                            crate::preview::layer_thumbnail(&r.document, layer.id),
                        )),
                        LayerTreeNode::Group(_) => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    let active = editor.read(|r| r.document.active_layer());
    let can_undo = editor.read(|r| r.document.can_undo()).unwrap_or(false);
    let can_redo = editor.read(|r| r.document.can_redo()).unwrap_or(false);
    let zoom = editor
        .read(|r| format!("{:.0}%", r.viewport.zoom * 100.0))
        .unwrap_or_else(|| "—".into());
    let dimensions = editor
        .read(|r| {
            let c = r.document.canvas();
            format!("{} × {}", c.width_px, c.height_px)
        })
        .unwrap_or_default();
    let modal = (editor.modal)();
    let notice = (editor.notice)();
    rsx! {
        document::Link { rel: "stylesheet", href: STYLE }
        document::Meta { name: "theme-color", content: "#242424" }
        div { class: "editor-app", "inert": modal.map(|_| ""),
            header { class: "action-bar",
                a { class: "wordmark", href: "../", title: "NyatiDraw 홈페이지", "N" span { "NyatiDraw" } }
                span { class: "web-badge", "WEB · 실험판" }
                fieldset { disabled: !ready, class: "actions",
                    button { title: "새 그림", onclick: move |_| editor.show_modal(EditorModal::NewDocument), Icon { name: "plus" } span { "새 그림" } }
                    button { title: "PNG 또는 웹 작업 파일 열기", onclick: move |_| editor.open(), Icon { name: "open" } span { "열기" } }
                    button { title: "편집 가능한 웹 작업 파일 내려받기 (Ctrl+S)", onclick: move |_| editor.download_project(), Icon { name: "save" } span { "작업 저장" } }
                    span { class: "separator" }
                    button { class: "square", title: "되돌리기 (Ctrl+Z)", disabled: !can_undo, onclick: move |_| editor.edit(WebDocument::undo), Icon { name: "undo" } }
                    button { class: "square", title: "다시 하기 (Ctrl+Shift+Z)", disabled: !can_redo, onclick: move |_| editor.edit(WebDocument::redo), Icon { name: "redo" } }
                    span { class: "separator" }
                    button { class: "square", title: "화면에 맞춤", onclick: move |_| editor.fit(), Icon { name: "fit" } }
                    button { class: "square", title: "축소", onclick: move |_| editor.zoom(0.8), "−" }
                    output { class: "zoom", "{zoom}" }
                    button { class: "square", title: "확대", onclick: move |_| editor.zoom(1.25), "+" }
                }
                div { class: "action-spacer" }
                button { class: "export-button", disabled: !ready, onclick: move |_| editor.download_png(), Icon { name: "download" } "PNG 내보내기" }
                button { class: "square", title: "웹판 안내", onclick: move |_| editor.show_modal(EditorModal::Help), "?" }
            }
            main { class: "workspace",
                aside { class: "left-sidebar", aria_label: "브러시 도구",
                    section { class: "panel",
                        h2 { "도구" }
                        fieldset { disabled: !ready, class: "tool-list",
                            for candidate in WebTool::ALL {
                                button { class: if candidate == tool { "tool selected" } else { "tool" }, aria_pressed: candidate == tool,
                                    onclick: move |_| editor.select_tool(candidate),
                                    Icon { name: if candidate == WebTool::Eraser { "eraser" } else { "pen" } }
                                    span { "{candidate.label()}" }
                                    sub { if candidate == WebTool::Eraser { "E" } else if candidate == WebTool::Pencil2H { "B" } }
                                }
                            }
                        }
                    }
                    section { class: "panel properties",
                        h2 { "도구 속성" }
                        div { class: "brush-preview", aria_label: "브러시 크기와 색 미리보기",
                            span { style: "height:{settings.size_px.clamp(2.0, 28.0)}px;background:{color};opacity:{settings.opacity}" }
                        }
                        fieldset { disabled: !ready,
                            label { class: "number-row", span { "크기" }
                                input { aria_label: "브러시 크기", r#type: "number", min: "0.5", max: "200", step: "0.5", value: "{settings.size_px}", onchange: move |event| {
                                    if let Ok(value) = event.value().parse::<f32>() { change_brush(editor, |s| s.size_px = value.clamp(0.5, 200.0)); }
                                } } span { class: "unit", "px" }
                            }
                            input { aria_label: "브러시 크기 조절", r#type: "range", min: "0.5", max: "100", step: "0.5", value: "{settings.size_px}", oninput: move |event| {
                                if let Ok(value) = event.value().parse::<f32>() { change_brush(editor, |s| s.size_px = value); }
                            } }
                            label { class: "number-row", span { "불투명도" }
                                input { aria_label: "브러시 불투명도", r#type: "number", min: "0", max: "100", value: "{(settings.opacity * 100.0).round()}", onchange: move |event| {
                                    if let Ok(value) = event.value().parse::<f32>() { change_brush(editor, |s| s.opacity = value.clamp(0.0, 100.0) / 100.0); }
                                } } span { class: "unit", "%" }
                            }
                            input { aria_label: "브러시 불투명도 조절", r#type: "range", min: "0", max: "100", value: "{settings.opacity * 100.0}", oninput: move |event| {
                                if let Ok(value) = event.value().parse::<f32>() { change_brush(editor, |s| s.opacity = value / 100.0); }
                            } }
                            details { summary { "추가 옵션" }
                                label { class: "check-row", input { r#type: "checkbox", checked: settings.size_pressure, onchange: move |event| change_brush(editor, |s| s.size_pressure = event.checked()) } "크기에 필압 사용" }
                                label { class: "check-row", input { r#type: "checkbox", checked: settings.opacity_pressure, onchange: move |event| change_brush(editor, |s| s.opacity_pressure = event.checked()) } "불투명도에 필압 사용" }
                                label { class: "number-row", "경도" output { "{(settings.hardness * 100.0).round()}%" } }
                                input { aria_label: "경도", r#type: "range", min: "0", max: "100", value: "{settings.hardness * 100.0}", oninput: move |event| {
                                    if let Ok(value) = event.value().parse::<f32>() { change_brush(editor, |s| s.hardness = value / 100.0); }
                                } }
                            }
                        }
                    }
                    section { class: "panel quick-sizes",
                        h2 { "크기 빠른 선택" }
                        fieldset { disabled: !ready, class: "size-grid",
                            for size in [1_u16, 3, 5, 10, 20, 40, 60, 100] {
                                button { class: if (settings.size_px - f32::from(size)).abs() < 0.1 { "size selected" } else { "size" }, title: "{size} px", onclick: move |_| change_brush(editor, |s| s.size_px = f32::from(size)),
                                    span { class: "size-dot", style: "width:{size.min(24)}px;height:{size.min(24)}px" }
                                    span { "{size}" }
                                }
                            }
                        }
                    }
                }
                div { class: "canvas-host",
                    canvas { id: "drawing-canvas", tabindex: "0", aria_label: "그리기 캔버스. B 연필, E 지우개, 휠 확대축소, 가운데 버튼 또는 Space 드래그로 이동." }
                    if !ready {
                        div { class: "startup", role: "status",
                            strong { "NyatiDraw Web" }
                            p { "{editor.status}" }
                            p { class: "muted", "최신 Chrome · Edge의 WebGPU 지원 환경이 필요합니다." }
                            button { class: "recovery-download", onclick: move |_| editor.download_recovery(), "저장된 작업 내려받기" }
                            a { href: "../", "홈페이지로 돌아가기" }
                        }
                    }
                }
                aside { class: "right-sidebar", aria_label: "색상과 레이어",
                    section { class: "panel color-panel",
                        h2 { "색상" }
                        label { class: "color-control", span { "현재 색" } input { aria_label: "그리기 색상", r#type: "color", value: "{color}", disabled: !ready, oninput: move |event| {
                            if let Some(color) = parse_color(&event.value()) { editor.preferences(|d| { d.set_foreground(color); Ok(()) }); }
                        } } }
                        div { class: "recent-colors", aria_label: "그리기 시작할 때 기록되는 최근 색상",
                            for value in recent {
                                button { style: format!("background:{}", color_hex(value)), title: "최근 색상 선택", onclick: move |_| editor.preferences(|d| { d.set_foreground(value); Ok(()) }) }
                            }
                        }
                    }
                    section { class: "panel layers-panel",
                        h2 { "레이어" }
                        fieldset { disabled: !ready, class: "layer-actions",
                            button { title: "새 레이어", onclick: move |_| editor.edit(|d| d.add_layer(&format!("Layer {}", d.layers().root().children.len() + 1))), Icon { name: "plus" } }
                            button { title: "현재 레이어 비우기 (되돌릴 수 있음)", onclick: move |_| editor.edit(WebDocument::clear_active_layer), Icon { name: "clear" } }
                        }
                        div { class: "layer-list",
                            for (layer, thumbnail) in layers() {
                                LayerRow { editor, selected: active == Some(layer.id), layer, thumbnail }
                            }
                        }
                    }
                }
            }
            footer { class: "status-bar",
                span { class: "status-message", role: "status", "{editor.status}" }
                span { class: "gesture-hint", "Space / 가운데 버튼 · 이동　 휠 · 확대축소" }
                span { "{dimensions}" }
            }
            if let Some(message) = notice {
                div { class: "persistent-notice", role: "alert",
                    p { "{message}" }
                    button { title: "안내 닫기", aria_label: "안내 닫기", onclick: move |_| editor.dismiss_notice(), "×" }
                }
            }
        }
        if modal == Some(EditorModal::Help) {
            div { class: "modal-shade", onclick: move |_| editor.close_modal(),
                section { class: "dialog", role: "dialog", tabindex: "-1", aria_modal: "true", aria_label: "웹판 안내", onclick: |event| event.stop_propagation(),
                    onmounted: |_| crate::browser::focus_modal(),
                    onkeydown: move |event| { if event.key() == Key::Escape { event.prevent_default(); event.stop_propagation(); editor.close_modal(); } },
                    h1 { "웹에서 가볍게 그리기" }
                    p { "기존 Rust 브러시와 WebGPU 화면 합성을 사용하는 실험판입니다. Windows 앱의 모든 기능을 제공하지는 않습니다." }
                    ul {
                        li { "PNG 열기·내보내기, 2H/2B 연필, 펜, 브러시, 지우개와 기본 레이어를 지원합니다." }
                        li { "작업 파일은 .nyatidraw-web입니다. Windows용 .ntdr와는 다릅니다. 앱과 그림을 주고받을 때는 PNG를 사용하세요." }
                        li { "작업은 이 브라우저에 자동 복구용으로 저장됩니다. 사이트 데이터 삭제·비공개 창 종료 시 사라질 수 있으니 파일로 보관하세요. 서버에 그림을 전송하지 않습니다." }
                        li { "되돌리기는 최대 128개이며 메모리 한도에 따라 더 적을 수 있습니다. 다시 열면 현재 그림과 도구 설정을 복구하며, 되돌리기 기록은 복구하지 않습니다." }
                        li { "필압은 브라우저·기기 지원에 따라 다릅니다. 손가락 드래그는 화면을 이동합니다." }
                    }
                    button { class: "primary", onclick: move |_| editor.close_modal(), "그리기 계속" }
                }
            }
        }
        if modal == Some(EditorModal::NewDocument) {
            div { class: "modal-shade", onclick: move |_| editor.close_modal(),
                section { class: "dialog small-dialog", role: "dialog", tabindex: "-1", aria_modal: "true", aria_label: "새 그림", onclick: |event| event.stop_propagation(),
                    onmounted: |_| crate::browser::focus_modal(),
                    onkeydown: move |event| { if event.key() == Key::Escape { event.prevent_default(); event.stop_propagation(); editor.close_modal(); } },
                    h1 { "새 그림" }
                    p { "투명한 레이어 하나로 시작합니다." }
                    for (name, width, height) in [("HD · 1280 × 720", 1280, 720), ("FHD · 1920 × 1080", 1920, 1080), ("정사각형 · 512 × 512", 512, 512), ("픽셀 작업 · 64 × 64", 64, 64)] {
                        button { class: "preset", onclick: move |_| editor.new_document(width, height), "{name}" }
                    }
                    button { "data-dialog-initial": "", onclick: move |_| editor.close_modal(), "취소" }
                }
            }
        }
        if modal == Some(EditorModal::ReplaceDocument) {
            div { class: "modal-shade", onclick: move |_| editor.close_modal(),
                section { class: "dialog small-dialog", role: "dialog", tabindex: "-1", aria_modal: "true", aria_labelledby: "replace-title", aria_describedby: "replace-description", onclick: |event| event.stop_propagation(),
                    onmounted: |_| crate::browser::focus_modal(),
                    onkeydown: move |event| { if event.key() == Key::Escape { event.prevent_default(); event.stop_propagation(); editor.close_modal(); } },
                    h1 { id: "replace-title", "현재 그림을 바꿀까요?" }
                    p { id: "replace-description", "새 그림을 열면 현재 그림의 자동 복구 저장도 바뀝니다. 보관하려면 취소 후 ‘작업 저장’으로 내려받아주세요." }
                    div { class: "dialog-actions",
                        button { "data-dialog-initial": "", onclick: move |_| editor.close_modal(), "취소" }
                        button { class: "primary", onclick: move |_| editor.confirm_replacement(), "그림 바꾸기" }
                    }
                }
            }
        }
    }
}

#[component]
fn LayerRow(
    editor: Editor,
    selected: bool,
    layer: LayerNode,
    thumbnail: Option<String>,
) -> Element {
    let id = layer.id;
    let visible = layer.visible;
    rsx! {
        div { class: if selected { "layer selected" } else { "layer" },
            div { class: "layer-heading",
                button { class: "square visibility", title: if visible { "레이어 숨기기" } else { "레이어 보이기" }, aria_pressed: visible, onclick: move |_| editor.edit(|d| d.set_layer_visible(id, !visible)), Icon { name: "eye" } }
                if let Some(image) = thumbnail {
                    button { class: "layer-thumbnail", title: "레이어 선택", onclick: move |_| editor.preferences(|d| d.set_active_layer(id)), img { src: image, alt: "{layer.name} 미리보기" } }
                }
                button { class: "layer-name", title: "레이어 선택", onclick: move |_| editor.preferences(|d| d.set_active_layer(id)), "{layer.name}" }
            }
            div { class: "layer-opacity",
                input { aria_label: "{layer.name} 불투명도", r#type: "range", min: "0", max: "100", value: "{f32::from(layer.opacity_u16) / 655.35}", onchange: move |event| {
                    if let Ok(value) = event.value().parse::<f32>() { editor.edit(|d| d.set_layer_opacity(id, value / 100.0)); }
                } }
                output { "{(f32::from(layer.opacity_u16) / 655.35).round()}%" }
            }
        }
    }
}

#[component]
fn Icon(name: String) -> Element {
    let path = match name.as_str() {
        "open" => "M3 7V4h6l2 3h10v13H3z M3 10h18",
        "save" => "M4 3h13l4 4v14H3V3z M7 3v6h9V3 M7 21v-8h10v8",
        "undo" => "M9 5 3 11l6 6 M3 11h10c6 0 8 4 8 9",
        "redo" => "M15 5 21 11l-6 6 M21 11H11c-6 0-8 4-8 9",
        "fit" => "M3 9V3h6 M15 3h6v6 M21 15v6h-6 M9 21H3v-6",
        "download" => "M12 3v12 M7 10l5 5 5-5 M4 17v4h16v-4",
        "plus" => "M12 4v16 M4 12h16",
        "eraser" => "m4 14 9-11 8 7-9 11H8z M10 7l8 7 M12 21h9",
        "pen" => "m3 21 4-1L21 6l-4-4L3 16z M14 5l4 4 M3 16l4 4",
        "eye" => "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12 M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
        "clear" => "M4 7h16 M9 7V3h6v4 M6 7l1 14h10l1-14 M10 10v7 M14 10v7",
        _ => "M4 4h16v16H4z",
    };
    rsx! { svg { width: "20", height: "20", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "1.6", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true", path { d: path } } }
}

fn change_brush(editor: Editor, change: impl FnOnce(&mut BrushSettings)) {
    editor.preferences(|document| {
        let mut settings = document.brush_settings();
        change(&mut settings);
        document.set_brush_settings(settings)
    });
}

fn color_hex(color: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn parse_color(hex: &str) -> Option<[u8; 4]> {
    if hex.len() != 7 || !hex.starts_with('#') {
        return None;
    }
    let value = u32::from_str_radix(&hex[1..], 16).ok()?;
    let [_, red, green, blue] = value.to_be_bytes();
    Some([red, green, blue, 255])
}
