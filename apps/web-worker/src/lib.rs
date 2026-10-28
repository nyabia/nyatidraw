#![forbid(unsafe_code)]

use nyatidraw_project_web::RecoveryWriter;
use wasm_bindgen::prelude::*;

/// Encodes a bounded immutable page copy; no recovery or project state is touched.
/// # Errors
/// Rejects invalid source buffers, enlargement, or PNG encoding failure.
#[wasm_bindgen]
pub fn export_reduced_png(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
) -> Result<Vec<u8>, JsValue> {
    let surface = nyatidraw_tiles::FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width,
        height,
        pixels,
    };
    nyatidraw_png_io::encode_png_reduced_bytes(&surface, [output_width, output_height])
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

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

#[wasm_bindgen]
pub struct SpriteExport {
    png: Vec<u8>,
    manifest: String,
}

#[wasm_bindgen]
impl SpriteExport {
    pub fn take_png(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.png)
    }
    pub fn take_manifest(&mut self) -> String {
        std::mem::take(&mut self.manifest)
    }
}

/// Renders every frame of a frozen complete document in the export worker.
/// # Errors
/// Rejects missing animation, malformed checkpoints and shared output resource limits.
#[wasm_bindgen]
pub fn export_sprite_sheet(packet: &[u8], columns: Option<u32>) -> Result<SpriteExport, JsValue> {
    let project = nyatidraw_project_web::decode_export_checkpoint(packet)
        .map_err(|error| JsValue::from_str(&error))?;
    let timeline = project
        .animation_timeline()
        .ok_or_else(|| JsValue::from_str("그림에 애니메이션 프레임이 없습니다."))?;
    let sheet = nyatidraw_editor::animation_export::export_sprite_sheet(
        project.snapshot(),
        project.layers(),
        timeline,
        project.canvas(),
        columns,
    )
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let manifest = nyatidraw_editor::animation_export::sprite_sheet_manifest(&sheet);
    let png = nyatidraw_png_io::encode_png_reduced_bytes(
        &sheet.surface,
        [sheet.surface.width, sheet.surface.height],
    )
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    Ok(SpriteExport { png, manifest })
}
