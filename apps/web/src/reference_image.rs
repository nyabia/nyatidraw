use dioxus::prelude::*;
use js_sys::{Array, Uint8Array};
use nyatidraw_editor_ui::reference_panel::{ReferenceState, decode_reference_png};
use wasm_bindgen_futures::JsFuture;

use crate::browser;

#[component]
pub(crate) fn ReferenceImageButton(reference: ReferenceState) -> Element {
    rsx! {
        button { class: "command compact", disabled: reference.loading(), title: "참조용 PNG · 현재 세션에서만 표시",
            onclick: move |_| {
                let Some(request) = reference.begin_load() else { return };
                dioxus::core::spawn_forever(async move {
                    let result = match JsFuture::from(browser::pick_file(true)).await {
                        Ok(file) if file.is_null() || file.is_undefined() => Ok(None),
                        Ok(file) => {
                            let file = Array::from(&file);
                            let name = file.get(0).as_string().unwrap_or_else(|| "PNG".into());
                            let bytes = Uint8Array::new(&file.get(1)).to_vec();
                            let _ = JsFuture::from(browser::background_turn(0.0)).await;
                            decode_reference_png(&bytes, name).map(Some)
                        }
                        Err(error) => Err(browser::error_text(&error)),
                    };
                    reference.finish_load(request, result);
                });
            }, "PNG 열기"
        }
    }
}
