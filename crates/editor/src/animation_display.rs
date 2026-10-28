use nyatidraw_api::{CanvasSpec, ContentRootId, FrameId, LayerBlendMode, LayerId};
use nyatidraw_document::{AnimationTimeline, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

pub struct FrameDisplay {
    pub tree: LayerTree,
    pub tiles: TileSnapshot,
}

/// Disposable page-cropped onion rasters never enter the authoritative artwork.
/// # Errors
/// Rejects unknown frames and bounded scratch/overlay allocations before compositing.
#[allow(clippy::too_many_arguments)]
pub fn build_frame_display(
    tree: &LayerTree,
    tiles: &TileSnapshot,
    timeline: Option<&AnimationTimeline>,
    frame: Option<FrameId>,
    canvas: CanvasSpec,
    onion_skin: bool,
    max_overlay_bytes: u64,
) -> Result<FrameDisplay, String> {
    let Some(timeline) = timeline else {
        return Ok(FrameDisplay {
            tree: tree.clone(),
            tiles: tiles.clone(),
        });
    };
    let frame = frame.unwrap_or(timeline.frames()[0].id);
    let active_tree = timeline
        .resolve_frame_tree(tree, frame)
        .map_err(|error| format!("frame: {error:?}"))?;
    let ids = crate::layer_edit::raster_layer_ids(&active_tree);
    let mut objects: Vec<_> = tiles
        .iter()
        .filter(|(key, _)| ids.contains(&key.layer))
        .map(|(key, tile)| (key, tile.clone()))
        .collect();
    let mut root = active_tree.root().clone();
    if onion_skin {
        let index = timeline
            .frames()
            .iter()
            .position(|entry| entry.id == frame)
            .ok_or("없는 프레임입니다.")?;
        let neighbors = [
            index.checked_sub(1),
            (index + 1 < timeline.frames().len()).then_some(index + 1),
        ];
        validate_onion_budget(canvas, max_overlay_bytes)?;
        let first_id = crate::layer_edit::next_layer_node_id(tree)?;
        let active_cel = timeline.frames()[index].cel;
        for (direction, neighbor) in neighbors.into_iter().enumerate() {
            let Some(neighbor) = neighbor else {
                continue;
            };
            let neighbor = timeline.frames()[neighbor];
            if neighbor.cel == active_cel {
                continue;
            }
            let layer = LayerId(
                first_id
                    .checked_add(direction as u128)
                    .ok_or("ID가 소진되었습니다.")?,
            );
            let neighbor_tree = timeline
                .resolve_frame_tree(tree, neighbor.id)
                .map_err(|error| format!("frame: {error:?}"))?;
            let surface =
                nyatidraw_paint_cpu::flatten_layer_tree_rgba8(tiles, &neighbor_tree, canvas)
                    .map_err(|error| format!("어니언 합성: {error:?}"))?;
            let tinted = onion_tiles(&surface, layer, direction == 0)?;
            objects.extend(tinted.iter().map(|(key, tile)| (key, tile.clone())));
            root.children
                .insert(0, LayerTreeNode::Raster(onion_layer(layer, direction == 0)));
        }
    }
    Ok(FrameDisplay {
        tree: LayerTree::new(root).map_err(|error| format!("frame: {error:?}"))?,
        tiles: TileSnapshot::from_planes(
            objects,
            tiles
                .coverage_iter()
                .filter(|(key, _)| active_tree.mask_ids().contains(&key.mask))
                .map(|(key, tile)| (key, tile.clone())),
        )
        .map_err(|error| format!("frame tiles: {error:?}"))?,
    })
}

fn validate_onion_budget(canvas: CanvasSpec, limit: u64) -> Result<(), String> {
    let canvas_bytes = u64::from(canvas.width_px)
        .checked_mul(u64::from(canvas.height_px))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("어니언 스킨의 이미지가 너무 큽니다.")?;
    let padded = u64::from(canvas.width_px.div_ceil(TILE_EDGE))
        .checked_mul(u64::from(canvas.height_px.div_ceil(TILE_EDGE)))
        .and_then(|tiles| tiles.checked_mul(TILE_BYTE_LEN as u64))
        .and_then(|bytes| bytes.checked_mul(3))
        .and_then(|bytes| bytes.checked_add(canvas_bytes));
    if canvas.width_px == 0 || canvas.height_px == 0 || padded.is_none_or(|bytes| bytes > limit) {
        return Err("어니언 스킨의 메모리 한도를 넘었습니다.".into());
    }
    Ok(())
}

fn onion_layer(id: LayerId, previous: bool) -> LayerNode {
    LayerNode {
        mask: None,
        id,
        name: if previous {
            "이전 프레임"
        } else {
            "다음 프레임"
        }
        .into(),
        visible: true,
        locked: true,
        alpha_locked: true,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        opacity_u16: u16::MAX,
        reference: false,
        content_root: ContentRootId(0),
        color_effect: None,
    }
}

fn onion_tiles(
    surface: &nyatidraw_tiles::FlattenedRgba8,
    layer: LayerId,
    previous: bool,
) -> Result<TileSnapshot, String> {
    let mut tiles = Vec::new();
    for y in 0..surface.height.div_ceil(TILE_EDGE) {
        for x in 0..surface.width.div_ceil(TILE_EDGE) {
            let mut pixels = vec![0; TILE_BYTE_LEN];
            let mut occupied = false;
            for row in 0..TILE_EDGE.min(surface.height - y * TILE_EDGE) {
                for col in 0..TILE_EDGE.min(surface.width - x * TILE_EDGE) {
                    let source = (((y * TILE_EDGE + row) as usize * surface.width as usize
                        + (x * TILE_EDGE + col) as usize)
                        * 4)
                        + 3;
                    let alpha = (u16::from(surface.pixels[source]) + 2) / 4;
                    let alpha = u8::try_from(alpha).expect("quarter alpha fits byte");
                    occupied |= alpha != 0;
                    let destination = ((row * TILE_EDGE + col) * 4) as usize;
                    let color = if previous {
                        [alpha, alpha / 4, alpha / 4, alpha]
                    } else {
                        [alpha / 4, alpha / 2, alpha, alpha]
                    };
                    pixels[destination..destination + 4].copy_from_slice(&color);
                }
            }
            if occupied {
                tiles.push((
                    TileKey {
                        layer,
                        mip: 0,
                        x: i32::try_from(x).map_err(|_| "어니언 너비 초과")?,
                        y: i32::try_from(y).map_err(|_| "어니언 높이 초과")?,
                    },
                    pixels,
                ));
            }
        }
    }
    TileSnapshot::from_tiles(tiles).map_err(|error| format!("onion tiles: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{AnimationCommand, GroupId};
    use nyatidraw_document::GroupNode;

    #[test]
    fn onion_display_never_changes_stored_pixels_or_allocates_inactive_layer_scenes() {
        let canvas = CanvasSpec {
            width_px: 16,
            height_px: 16,
            pixels_per_inch: 96,
        };
        let tree = LayerTree::new(GroupNode {
            id: GroupId(0),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![LayerTreeNode::Raster(crate::layer_edit::empty_raster(
                LayerId(1),
                "Ink".into(),
            ))],
        })
        .unwrap();
        let key = TileKey {
            layer: LayerId(1),
            mip: 0,
            x: 0,
            y: 0,
        };
        let original =
            TileSnapshot::from_tiles([(key, [0, 0, 0, 255].repeat(TILE_BYTE_LEN / 4))]).unwrap();
        let enabled = crate::animation::prepare_animation_edit(
            AnimationCommand::Enable,
            &tree,
            &original,
            None,
            None,
            LayerId(1),
            canvas,
            2,
        )
        .unwrap();
        let second = crate::animation::prepare_animation_edit(
            AnimationCommand::InsertBlank {
                after: enabled.active_frame.unwrap(),
            },
            &enabled.tree,
            &enabled.tiles,
            enabled.timeline.as_ref(),
            enabled.active_frame,
            enabled.active_layer,
            canvas,
            1,
        )
        .unwrap();
        let display = build_frame_display(
            &second.tree,
            &second.tiles,
            second.timeline.as_ref(),
            second.active_frame,
            canvas,
            true,
            1_048_576,
        )
        .unwrap();
        assert_eq!(second.tiles, original);
        assert!(display.tree.raster(key.layer).is_none());
        assert_eq!(display.tree.root().children.len(), 2);
        assert_eq!(display.tiles.len(), 1);
        assert!(display.tiles.iter().all(|(key, _)| key.layer != LayerId(1)));
        let pixel = display.tiles.iter().next().unwrap().1.pixels();
        assert_eq!(&pixel[..4], &[64, 16, 16, 64]);
        assert_eq!(&pixel[16 * 4..17 * 4], &[0; 4]);
        assert!(
            build_frame_display(
                &second.tree,
                &second.tiles,
                second.timeline.as_ref(),
                second.active_frame,
                canvas,
                true,
                1024
            )
            .is_err()
        );
        let clean = build_frame_display(
            &second.tree,
            &second.tiles,
            second.timeline.as_ref(),
            second.active_frame,
            canvas,
            false,
            0,
        )
        .unwrap();
        assert!(clean.tiles.is_empty());
        let export =
            nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&second.tiles, &clean.tree, canvas)
                .unwrap();
        assert!(export.pixels.iter().all(|pixel| *pixel == 0));
    }
}
