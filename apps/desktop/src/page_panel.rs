use std::collections::VecDeque;

use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{CanvasSpec, EditCommand, EditorCommand, UiProjection};

const COMMON_PRESETS: [CanvasPreset; 3] = [
    CanvasPreset {
        name: "FHD",
        size: [1920, 1080],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "UXGA",
        size: [1600, 1200],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "64 × 64",
        size: [64, 64],
        ratio: "1:1",
    },
];
const MORE_PRESETS: [CanvasPreset; 10] = [
    CanvasPreset {
        name: "HD",
        size: [1280, 720],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "QHD",
        size: [2560, 1440],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "4K UHD",
        size: [3840, 2160],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "XGA",
        size: [1024, 768],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "SVGA",
        size: [800, 600],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "VGA",
        size: [640, 480],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "128 × 128",
        size: [128, 128],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "256 × 256",
        size: [256, 256],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "512 × 512",
        size: [512, 512],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "1024 × 1024",
        size: [1024, 1024],
        ratio: "1:1",
    },
];
const SIZE_UNDO_LIMIT: usize = 32;

#[derive(Clone, Copy, PartialEq)]
struct CanvasPreset {
    name: &'static str,
    size: [u32; 2],
    ratio: &'static str,
}

fn within_output_limit([width, height]: [u32; 2]) -> bool {
    width > 0
        && height > 0
        && u64::from(width) * u64::from(height) <= nyatidraw_tiles::MAX_FLATTENED_PIXELS
}

#[derive(Clone, PartialEq)]
struct CanvasSizeInput {
    width: String,
    height: String,
    source: Option<(CanvasPreset, u32)>,
}

impl CanvasSizeInput {
    fn new(canvas: CanvasSpec) -> Self {
        let landscape = [
            canvas.width_px.max(canvas.height_px),
            canvas.width_px.min(canvas.height_px),
        ];
        let source = COMMON_PRESETS
            .iter()
            .chain(&MORE_PRESETS)
            .find(|preset| preset.size == landscape)
            .map(|&preset| (preset, 1));
        Self {
            width: canvas.width_px.to_string(),
            height: canvas.height_px.to_string(),
            source,
        }
    }

    fn size(&self) -> Result<[u32; 2], &'static str> {
        let (Ok(width), Ok(height)) = (self.width.parse::<u32>(), self.height.parse::<u32>())
        else {
            return Err("가로와 세로를 양의 정수로 입력하세요.");
        };
        if width == 0 || height == 0 {
            return Err("가로와 세로는 1 이상이어야 합니다.");
        }
        within_output_limit([width, height])
            .then_some([width, height])
            .ok_or("현재 출력 한도를 초과합니다. 크기를 줄여주세요.")
    }

    fn multiplied(&self, factor: u32) -> Option<Self> {
        let [width, height] = self.size().ok()?;
        let size = [width.checked_mul(factor)?, height.checked_mul(factor)?];
        if !within_output_limit(size) {
            return None;
        }
        let source = match self.source {
            Some((preset, multiplier)) => Some((preset, multiplier.checked_mul(factor)?)),
            None => None,
        };
        Some(Self {
            width: size[0].to_string(),
            height: size[1].to_string(),
            source,
        })
    }

    fn label(&self) -> String {
        match self.source {
            Some((preset, 1)) => preset.name.into(),
            Some((preset, multiplier)) => format!("{} ×{multiplier}", preset.name),
            None => "커스텀".into(),
        }
    }
}

struct CanvasDraft {
    current: CanvasSizeInput,
    undo: VecDeque<CanvasSizeInput>,
    typing_start: Option<CanvasSizeInput>,
}

impl CanvasDraft {
    fn new(canvas: CanvasSpec) -> Self {
        Self {
            current: CanvasSizeInput::new(canvas),
            undo: VecDeque::new(),
            typing_start: None,
        }
    }

    fn remember(&mut self, previous: CanvasSizeInput) {
        if previous == self.current {
            return;
        }
        if self.undo.len() == SIZE_UNDO_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(previous);
    }

    fn finish_typing(&mut self) {
        if let Some(previous) = self.typing_start.take() {
            self.remember(previous);
        }
    }

    fn type_dimension(&mut self, horizontal: bool, text: String) {
        if self.typing_start.is_none() {
            self.typing_start = Some(self.current.clone());
        }
        if horizontal {
            self.current.width = text;
        } else {
            self.current.height = text;
        }
        self.current.source = None;
    }

    fn replace(&mut self, next: CanvasSizeInput) {
        self.finish_typing();
        let previous = std::mem::replace(&mut self.current, next);
        self.remember(previous);
    }

    fn choose_preset(&mut self, preset: CanvasPreset) {
        let mut size = preset.size;
        if let Ok([width, height]) = self.current.size()
            && width < height
        {
            size.swap(0, 1);
        }
        self.replace(CanvasSizeInput {
            width: size[0].to_string(),
            height: size[1].to_string(),
            source: Some((preset, 1)),
        });
    }

    fn multiply(&mut self, factor: u32) {
        if let Some(next) = self.current.multiplied(factor) {
            self.replace(next);
        }
    }

    fn swap_axes(&mut self) {
        let mut next = self.current.clone();
        std::mem::swap(&mut next.width, &mut next.height);
        self.replace(next);
    }

    fn can_undo(&self) -> bool {
        !self.undo.is_empty()
            || self
                .typing_start
                .as_ref()
                .is_some_and(|start| start != &self.current)
    }

    fn undo(&mut self) {
        self.finish_typing();
        if let Some(previous) = self.undo.pop_back() {
            self.current = previous;
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PagePanelOpen(pub Signal<bool>);

#[component]
pub(crate) fn PagePanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let crop_ink = live_ink.clone();
    let PagePanelOpen(mut opened) = use_context();
    let mut draft = use_signal(|| CanvasDraft::new(ui_projection.read().canvas));
    let mut appearance = use_signal(|| live_ink.workspace_appearance());
    let mut error = use_signal(|| None::<String>);
    let current = ui_projection.read();
    let busy = current.edit.busy;
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
            fieldset { class: "canvas-background-settings",
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
                    let command = EditCommand::ResizePage { size: [w, h] };
                    let result = live_ink.push_editor_command(ui_projection.read().revision, EditorCommand::Edit(command));
                    println!("desktop-page event=resize size={w}x{h} admission={result:?}");
                    if result.is_ok() {
                        live_ink.set_workspace_appearance(appearance());
                        opened.set(false);
                    }
                    else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
                }, "적용" }
                button { disabled: !can_crop, title: "선택 경계에 캔버스를 맞춥니다. 바깥 그림도 보존하며 함께 이동합니다.", onclick: move |_| {
                    let result = crop_ink.push_editor_command(ui_projection.read().revision, EditorCommand::Edit(EditCommand::CropPageToSelection));
                    println!("desktop-page event=crop-selection admission={result:?}");
                    if result.is_ok() {
                        crop_ink.set_workspace_appearance(appearance());
                        opened.set(false);
                    }
                    else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
                }, "선택에 맞추기" }
                button { onclick: move |_| opened.set(false), "닫기" }
            }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
        }
        }
    }
}
