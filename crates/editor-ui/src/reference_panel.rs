use std::sync::Arc;

use dioxus::prelude::*;
use nyatidraw_api::LayerId;

pub const MAX_REFERENCE_BYTES: usize = 72 * 1024 * 1024;
const MAX_REFERENCE_EDGE: u32 = 4096;

#[derive(Clone)]
pub struct ReferenceFrame {
    name: String,
    data_uri: Arc<str>,
}

#[derive(Default)]
struct ReferenceSession {
    frame: Option<Arc<ReferenceFrame>>,
    generation: u64,
    active_load: Option<u64>,
    error: Option<String>,
    show_reference: bool,
    zoom: f64,
    pan: [f64; 2],
}

#[derive(Clone, Copy, PartialEq)]
pub struct ReferenceState(Signal<ReferenceSession>);

pub(crate) fn provide_reference() {
    use_context_provider(|| {
        ReferenceState(Signal::new(ReferenceSession {
            zoom: 1.0,
            ..ReferenceSession::default()
        }))
    });
}

impl ReferenceState {
    #[must_use]
    pub fn begin_load(mut self) -> Option<u64> {
        let mut state = self.0.write();
        if state.active_load.is_some() {
            return None;
        }
        state.generation = state.generation.wrapping_add(1);
        let request = state.generation;
        state.active_load = Some(request);
        state.error = None;
        Some(request)
    }

    pub fn finish_load(
        mut self,
        request: u64,
        result: Result<Option<Arc<ReferenceFrame>>, String>,
    ) {
        let Ok(mut state) = self.0.try_write() else {
            return;
        };
        if state.active_load != Some(request) {
            return;
        }
        state.active_load = None;
        if state.generation != request {
            return;
        }
        match result {
            Ok(Some(frame)) => {
                state.frame = Some(frame);
                state.zoom = 1.0;
                state.pan = [0.0; 2];
            }
            Ok(None) => {}
            Err(error) => state.error = Some(error),
        }
    }

    #[must_use]
    pub fn loading(self) -> bool {
        self.0.read().active_load.is_some()
    }

    pub(crate) fn show_reference(self) -> bool {
        self.0.read().show_reference
    }

    pub(crate) fn select(mut self, reference: bool) {
        self.0.write().show_reference = reference;
    }

    fn clear(mut self) {
        let mut state = self.0.write();
        state.generation = state.generation.wrapping_add(1);
        state.frame = None;
        state.error = None;
        state.zoom = 1.0;
        state.pan = [0.0; 2];
    }

    fn zoom_by(mut self, factor: f64) {
        let mut state = self.0.write();
        state.zoom = (state.zoom * factor).clamp(1.0, 16.0);
    }

    fn fit(mut self) {
        let mut state = self.0.write();
        state.zoom = 1.0;
        state.pan = [0.0; 2];
    }
}

/// # Errors
/// Rejects oversized, malformed or unsupported PNGs before replacing the displayed reference.
pub fn decode_reference_png(bytes: &[u8], name: String) -> Result<Arc<ReferenceFrame>, String> {
    if bytes.len() > MAX_REFERENCE_BYTES || bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        return Err("72 MiB 이하의 PNG 이미지를 선택하세요.".into());
    }
    let dimensions = bytes.get(16..24).ok_or("잘린 PNG 이미지입니다.")?;
    let width = u32::from_be_bytes(
        dimensions[..4]
            .try_into()
            .map_err(|_| "잘린 PNG 너비입니다.")?,
    );
    let height = u32::from_be_bytes(
        dimensions[4..]
            .try_into()
            .map_err(|_| "잘린 PNG 높이입니다.")?,
    );
    if width > MAX_REFERENCE_EDGE || height > MAX_REFERENCE_EDGE {
        return Err("참조 이미지는 가로·세로 4096 px까지 열 수 있습니다.".into());
    }
    let imported =
        nyatidraw_png_io::decode_png_bytes(bytes, LayerId(1), u64::from(MAX_REFERENCE_EDGE).pow(2))
            .map_err(|error| error.to_string())?;
    let surface = imported
        .tiles
        .crop_base_layer_rgba8_to_canvas(LayerId(1), imported.canvas)
        .map_err(|error| format!("{error:?}"))?;
    let png = nyatidraw_png_io::encode_png_bytes(&surface).map_err(|error| error.to_string())?;
    let data_uri = crate::preview::png_data_uri(&png, 96 * 1024 * 1024, "reference")?;
    Ok(Arc::new(ReferenceFrame { name, data_uri }))
}

#[component]
pub(crate) fn ReferencePanel() -> Element {
    let mut reference = use_context::<ReferenceState>();
    let loader = use_context::<crate::UiSlots>().reference_loader;
    let mut drag = use_signal(|| None::<(i32, [f64; 2], [f64; 2])>);
    let state = reference.0.read();
    let frame = state.frame.clone();
    let error = state.error.clone();
    let image_style = format!(
        "transform:translate({}px,{}px) scale({});",
        state.pan[0], state.pan[1], state.zoom
    );
    let percent = format!("{:.0}%", state.zoom * 100.0);
    drop(state);
    rsx! {
        div { class: "reference-panel",
            div { class: "reference-actions",
                {loader(reference)}
                button { class: "command compact", title: "참조 이미지 비우기", onclick: move |_| reference.clear(), "×" }
            }
            div { class: "reference-view", aria_label: "참조 이미지 보기",
                onpointerdown: move |event| {
                    if event.data().trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) { return; }
                    event.prevent_default();
                    event.stop_propagation();
                    let point = event.data().client_coordinates();
                    drag.set(Some((event.data().pointer_id(), [point.x, point.y], reference.0.read().pan)));
                },
                onpointermove: move |event| {
                    if let Some((pointer, start, pan)) = *drag.read() {
                        if pointer != event.data().pointer_id() { return; }
                        event.prevent_default();
                        event.stop_propagation();
                        let point = event.data().client_coordinates();
                        reference.0.write().pan = [pan[0] + point.x - start[0], pan[1] + point.y - start[1]];
                    }
                },
                onpointerup: move |_| drag.set(None),
                onpointercancel: move |_| drag.set(None),
                onpointerleave: move |_| drag.set(None),
                if let Some(frame) = frame {
                    img { src: "{frame.data_uri}", alt: "{frame.name}", draggable: "false", style: image_style }
                } else {
                    span { class: "reference-empty", "PNG 참조 이미지" }
                }
            }
            div { class: "reference-actions",
                button { class: "command compact", title: "참조 이미지 맞춤", onclick: move |_| reference.fit(), crate::UiIcon { name: "fit" } }
                button { class: "command compact", title: "참조 이미지 축소", onclick: move |_| reference.zoom_by(0.8), crate::UiIcon { name: "minus" } }
                span { class: "toolbar-value", "{percent}" }
                button { class: "command compact", title: "참조 이미지 확대", onclick: move |_| reference.zoom_by(1.25), crate::UiIcon { name: "plus" } }
            }
            if let Some(error) = error { p { class: "reference-error", role: "alert", "{error}" } }
        }
    }
}
