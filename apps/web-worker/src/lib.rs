#![forbid(unsafe_code)]

use nyatidraw_project_web::RecoveryWriter;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
#[derive(Default)]
pub struct RecoveryEngine(RecoveryWriter);

#[wasm_bindgen]
impl RecoveryEngine {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// # Errors
    /// Rejects invalid or stale recovery packets without changing accepted state.
    pub fn stage(&mut self, packet: &[u8]) -> Result<Vec<u8>, JsValue> {
        self.0
            .stage(packet)
            .map_err(|error| JsValue::from_str(&error))
    }

    /// # Errors
    /// Requires a staged file that has completed its `IndexedDB` transaction.
    pub fn accept(&mut self) -> Result<(), JsValue> {
        self.0.accept().map_err(|error| JsValue::from_str(&error))
    }

    pub fn abort(&mut self) {
        self.0.abort();
    }
}
