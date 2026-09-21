use js_sys::{Function, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

#[wasm_bindgen(module = "/src/browser.js")]
extern "C" {
    #[wasm_bindgen(js_name = bindCanvas)]
    pub fn bind_canvas(canvas: &HtmlCanvasElement, callback: &Function);
    #[wasm_bindgen(js_name = claimWorkspace)]
    pub fn claim_workspace() -> Promise;
    #[wasm_bindgen(js_name = loadWorkspace)]
    pub fn load_workspace() -> Promise;
    #[wasm_bindgen(js_name = storeWorkspace)]
    pub fn store_workspace(bytes: &Uint8Array) -> Promise;
    #[wasm_bindgen(js_name = setUnsaved)]
    pub fn set_unsaved(unsaved: bool);
    #[wasm_bindgen(js_name = downloadBytes)]
    pub fn download_bytes(bytes: &Uint8Array, filename: &str, mime: &str);
    #[wasm_bindgen(js_name = pickFile)]
    pub fn pick_file() -> Promise;
    #[wasm_bindgen(js_name = setModalOpen)]
    pub fn set_modal_open(open: bool);
    #[wasm_bindgen(js_name = focusModal)]
    pub fn focus_modal();
}

pub fn error_text(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
