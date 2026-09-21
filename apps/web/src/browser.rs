use js_sys::{Function, Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

#[wasm_bindgen(module = "/src/browser.js")]
extern "C" {
    #[wasm_bindgen(js_name = monotonicNow)]
    pub fn monotonic_now() -> f64;
    #[wasm_bindgen(js_name = recordWork)]
    pub fn record_work(kind: &str, started: f64);
    #[wasm_bindgen(js_name = backgroundTurn)]
    pub fn background_turn(delay_ms: f64) -> Promise;
    #[wasm_bindgen(js_name = bindCanvas)]
    pub fn bind_canvas(canvas: &HtmlCanvasElement, callback: &Function);
    #[wasm_bindgen(js_name = claimWorkspace)]
    pub fn claim_workspace() -> Promise;
    #[wasm_bindgen(js_name = loadWorkspace)]
    pub fn load_workspace() -> Promise;
    #[wasm_bindgen(js_name = saveInWorker)]
    pub fn save_in_worker(bytes: &Uint8Array, download: bool) -> Promise;
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
    #[wasm_bindgen(js_name = setCanvasTool)]
    pub fn set_canvas_tool(tool: &str);
    #[wasm_bindgen(catch, js_name = loadPreference)]
    pub fn load_preference(key: &str) -> Result<Option<String>, JsValue>;
    #[wasm_bindgen(catch, js_name = storePreference)]
    pub fn store_preference(key: &str, value: &str) -> Result<(), JsValue>;
}

pub fn error_text(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
