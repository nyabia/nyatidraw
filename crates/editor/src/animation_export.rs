use nyatidraw_api::{CanvasSpec, CelId, FrameId};
use nyatidraw_document::{AnimationError, AnimationTimeline, LayerTree};
use nyatidraw_tiles::{FlattenedRgba8, TileSnapshot};

pub const MAX_SPRITESHEET_EDGE: u32 = 16_384;
pub const MAX_SPRITESHEET_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_SPRITESHEET_WORKING_BYTES: u64 = 320 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteFrame {
    pub frame_id: FrameId,
    pub cel_id: CelId,
    pub duration_ms: u32,
    pub rect: [u32; 4],
}

#[derive(Debug)]
pub struct SpriteSheet {
    pub surface: FlattenedRgba8,
    pub frames: Vec<SpriteFrame>,
    pub frame_size: [u32; 2],
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug)]
pub enum AnimationExportError {
    Timeline(AnimationError),
    InvalidCanvas,
    InvalidColumns,
    SizeLimit,
    Composite(nyatidraw_paint_cpu::CpuCompositeError),
}

impl std::fmt::Display for AnimationExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SizeLimit => formatter.write_str("spritesheet exceeds 16384 px per edge, 64 Mi pixels, or 320 MiB sheet plus frame working memory"),
            _ => write!(formatter, "{self:?}"),
        }
    }
}

impl std::error::Error for AnimationExportError {}

/// Builds a row-major sheet of complete, untrimmed pages in timeline order.
/// Only one frame composite is staged alongside the bounded output image.
/// # Errors
/// Rejects invalid timelines, columns and overflow/resource limits before allocation.
pub fn export_sprite_sheet(
    tiles: &TileSnapshot,
    tree: &LayerTree,
    timeline: &AnimationTimeline,
    canvas: CanvasSpec,
    columns: Option<u32>,
) -> Result<SpriteSheet, AnimationExportError> {
    timeline
        .validate_against(tree)
        .map_err(AnimationExportError::Timeline)?;
    canvas
        .validate()
        .map_err(|_| AnimationExportError::InvalidCanvas)?;
    let count =
        u32::try_from(timeline.frames().len()).map_err(|_| AnimationExportError::SizeLimit)?;
    let columns = columns.unwrap_or_else(|| {
        (1..=count)
            .find(|width| width * width >= count)
            .unwrap_or(1)
    });
    if columns == 0 || columns > count {
        return Err(AnimationExportError::InvalidColumns);
    }
    let rows = count.div_ceil(columns);
    let width = canvas
        .width_px
        .checked_mul(columns)
        .ok_or(AnimationExportError::SizeLimit)?;
    let height = canvas
        .height_px
        .checked_mul(rows)
        .ok_or(AnimationExportError::SizeLimit)?;
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_SPRITESHEET_EDGE
        || height > MAX_SPRITESHEET_EDGE
        || pixels > MAX_SPRITESHEET_PIXELS
    {
        return Err(AnimationExportError::SizeLimit);
    }
    let frame_pixels = u64::from(canvas.width_px) * u64::from(canvas.height_px);
    if (pixels + frame_pixels) * 4 > MAX_SPRITESHEET_WORKING_BYTES {
        return Err(AnimationExportError::SizeLimit);
    }
    let bytes = usize::try_from(pixels * 4).map_err(|_| AnimationExportError::SizeLimit)?;
    let mut surface = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width,
        height,
        pixels: vec![0; bytes],
    };
    let row_bytes = canvas.width_px as usize * 4;
    let mut frames = Vec::with_capacity(timeline.frames().len());
    for (index, frame) in timeline.frames().iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| AnimationExportError::SizeLimit)?;
        let x = index % columns * canvas.width_px;
        let y = index / columns * canvas.height_px;
        let frame_tree = timeline
            .resolve_frame_tree(tree, frame.id)
            .map_err(AnimationExportError::Timeline)?;
        let rendered = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(tiles, &frame_tree, canvas)
            .map_err(AnimationExportError::Composite)?;
        for (row, pixels) in rendered.pixels.chunks_exact(row_bytes).enumerate() {
            let destination = ((y as usize + row) * width as usize + x as usize) * 4;
            surface.pixels[destination..destination + row_bytes].copy_from_slice(pixels);
        }
        frames.push(SpriteFrame {
            frame_id: frame.id,
            cel_id: frame.cel,
            duration_ms: frame.duration_ms,
            rect: [x, y, canvas.width_px, canvas.height_px],
        });
    }
    Ok(SpriteSheet {
        surface,
        frames,
        frame_size: [canvas.width_px, canvas.height_px],
        columns,
        rows,
    })
}

/// Serializes ordered frame rectangles and durations without truncating 128-bit IDs.
#[must_use]
pub fn sprite_sheet_manifest(sheet: &SpriteSheet) -> String {
    use std::fmt::Write as _;
    let mut text = format!(
        "{{\n  \"format\": \"nyatidraw-spritesheet-v1\",\n  \"image\": \"sheet.png\",\n  \"width\": {}, \"height\": {},\n  \"frame_width\": {}, \"frame_height\": {},\n  \"columns\": {}, \"rows\": {},\n  \"frames\": [\n",
        sheet.surface.width,
        sheet.surface.height,
        sheet.frame_size[0],
        sheet.frame_size[1],
        sheet.columns,
        sheet.rows
    );
    for (index, frame) in sheet.frames.iter().enumerate() {
        if index > 0 {
            text.push_str(",\n");
        }
        let [x, y, width, height] = frame.rect;
        let _ = write!(
            text,
            "    {{\"frame_id\":\"{}\",\"cel_id\":\"{}\",\"x\":{x},\"y\":{y},\"width\":{width},\"height\":{height},\"duration_ms\":{}}}",
            frame.frame_id.0, frame.cel_id.0, frame.duration_ms
        );
    }
    text.push_str("\n  ]\n}\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{GroupId, LayerBlendMode, LayerId};
    use nyatidraw_document::{AnimationCel, AnimationFrame, GroupNode, LayerTreeNode};
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    fn fixture() -> (LayerTree, AnimationTimeline, TileSnapshot) {
        let group = |id, layer| GroupNode {
            id: GroupId(id),
            name: format!("Cel {id}"),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![LayerTreeNode::Raster(crate::layer_edit::empty_raster(
                LayerId(layer),
                "Paint".into(),
            ))],
        };
        let tree = LayerTree::new(GroupNode {
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![
                LayerTreeNode::Group(group(2, 4)),
                LayerTreeNode::Group(group(3, 5)),
            ],
        })
        .unwrap();
        let timeline = AnimationTimeline::new(
            vec![
                AnimationFrame {
                    id: FrameId(3),
                    cel: CelId(2),
                    duration_ms: 120,
                },
                AnimationFrame {
                    id: FrameId(1),
                    cel: CelId(1),
                    duration_ms: 80,
                },
                AnimationFrame {
                    id: FrameId(2),
                    cel: CelId(2),
                    duration_ms: 200,
                },
            ],
            vec![
                AnimationCel {
                    id: CelId(1),
                    root_group: GroupId(2),
                },
                AnimationCel {
                    id: CelId(2),
                    root_group: GroupId(3),
                },
            ],
            &tree,
        )
        .unwrap();
        let tiles = TileSnapshot::from_tiles([(4, [128, 0, 0, 128]), (5, [0, 0, 255, 255])].map(
            |(layer, color)| {
                let mut pixels = vec![0; TILE_BYTE_LEN];
                pixels[..4].copy_from_slice(&color);
                (
                    TileKey {
                        layer: LayerId(layer),
                        mip: 0,
                        x: 0,
                        y: 0,
                    },
                    pixels,
                )
            },
        ))
        .unwrap();
        (tree, timeline, tiles)
    }

    #[test]
    fn sheet_keeps_frame_order_duration_transparency_and_source_artwork() {
        let (tree, timeline, tiles) = fixture();
        let source = tiles.clone();
        let sheet = export_sprite_sheet(
            &tiles,
            &tree,
            &timeline,
            CanvasSpec {
                width_px: 1,
                height_px: 1,
                pixels_per_inch: 96,
            },
            Some(2),
        )
        .unwrap();
        assert_eq!([sheet.surface.width, sheet.surface.height], [2, 2]);
        assert_eq!(
            sheet.surface.pixels,
            [0, 0, 255, 255, 128, 0, 0, 128, 0, 0, 255, 255, 0, 0, 0, 0]
        );
        assert_eq!(
            sheet
                .frames
                .iter()
                .map(|frame| (frame.frame_id.0, frame.duration_ms, frame.rect))
                .collect::<Vec<_>>(),
            vec![
                (3, 120, [0, 0, 1, 1]),
                (1, 80, [1, 0, 1, 1]),
                (2, 200, [0, 1, 1, 1])
            ]
        );
        assert_eq!(tiles, source);
        assert_eq!(timeline.frames()[0].id, FrameId(3));
    }

    #[test]
    fn oversized_or_invalid_sheet_requests_fail_before_allocating_pixels() {
        let (tree, timeline, tiles) = fixture();
        for (width_px, height_px, columns) in [
            (u32::MAX, 1, Some(2)),
            (16_384, 16_384, Some(1)),
            (4096, 5461, Some(1)),
            (1, 1, Some(0)),
            (1, 1, Some(4)),
            (0, 1, None),
        ] {
            assert!(
                export_sprite_sheet(
                    &tiles,
                    &tree,
                    &timeline,
                    CanvasSpec {
                        width_px,
                        height_px,
                        pixels_per_inch: 96
                    },
                    columns
                )
                .is_err()
            );
        }
    }
}
