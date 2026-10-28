use std::collections::BTreeSet;

use nyatidraw_api::{CanvasSpec, FrameId};
use nyatidraw_document::{AnimationTimeline, LayerTree};
use nyatidraw_tiles::{FlattenedRgba8, ObjectHash, TileSnapshot};

pub const FRAME_PREVIEW_WIDTH: u32 = 48;
pub const FRAME_PREVIEW_HEIGHT: u32 = 36;
const MAX_FINGERPRINT_BYTES: usize = 8 * 1024 * 1024;

/// One disposable cel description; never retains source pixel objects.
pub struct FramePreviewSource {
    pub fingerprint: ObjectHash,
    tree: LayerTree,
    canvas: CanvasSpec,
}

impl FramePreviewSource {
    /// # Errors
    /// Rejects missing cels, invalid trees and oversized fingerprint metadata.
    pub fn prepare(
        tree: &LayerTree,
        tiles: &TileSnapshot,
        timeline: &AnimationTimeline,
        frame: FrameId,
        canvas: CanvasSpec,
    ) -> Result<Self, String> {
        let tree = timeline
            .resolve_frame_tree(tree, frame)
            .map_err(|error| error.to_string())?;
        let layers: BTreeSet<_> = crate::layer_edit::raster_layer_ids(&tree)
            .into_iter()
            .collect();
        let masks: BTreeSet<_> = tree.mask_ids().into_iter().collect();
        let mut bytes =
            nyatidraw_project::encode_layer_tree(&tree).map_err(|error| format!("{error:?}"))?;
        bytes.extend_from_slice(&canvas.width_px.to_le_bytes());
        bytes.extend_from_slice(&canvas.height_px.to_le_bytes());
        bytes.extend_from_slice(&canvas.pixels_per_inch.to_le_bytes());
        for (key, tile) in tiles.iter().filter(|(key, _)| layers.contains(&key.layer)) {
            append_tile(
                &mut bytes,
                0,
                key.layer.0,
                key.mip,
                key.x,
                key.y,
                tile.hash(),
            )?;
        }
        for (key, tile) in tiles
            .coverage_iter()
            .filter(|(key, _)| masks.contains(&key.mask))
        {
            append_tile(
                &mut bytes,
                1,
                key.mask.0,
                key.mip,
                key.x,
                key.y,
                tile.hash(),
            )?;
        }
        let fingerprint = ObjectHash::digest_tagged(b"frame-preview-v1", &bytes);
        Ok(Self {
            fingerprint,
            tree,
            canvas,
        })
    }

    /// # Errors
    /// Rejects invalid preview source dimensions or unsupported tile levels.
    pub fn render(&self, tiles: &TileSnapshot) -> Result<FlattenedRgba8, String> {
        nyatidraw_paint_cpu::render_page_preview_rgba8(
            tiles,
            &self.tree,
            self.canvas,
            FRAME_PREVIEW_WIDTH,
            FRAME_PREVIEW_HEIGHT,
        )
        .map_err(|error| format!("Frame preview: {error:?}"))
    }
}

fn append_tile(
    bytes: &mut Vec<u8>,
    plane: u8,
    owner: u128,
    mip: u8,
    x: i32,
    y: i32,
    hash: ObjectHash,
) -> Result<(), String> {
    if bytes.len().saturating_add(58) > MAX_FINGERPRINT_BYTES {
        return Err("Frame preview metadata limit".into());
    }
    bytes.push(plane);
    bytes.extend_from_slice(&owner.to_le_bytes());
    bytes.push(mip);
    bytes.extend_from_slice(&x.to_le_bytes());
    bytes.extend_from_slice(&y.to_le_bytes());
    bytes.extend_from_slice(&hash.0);
    Ok(())
}
