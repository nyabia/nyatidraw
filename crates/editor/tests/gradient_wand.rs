use nyatidraw_api::{
    CanvasSpec, ContentRootId, EditCommand, EditSource, GroupId, LayerId, SelectionMode,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::pixel_edit::{MemoryArtworkClipboard, execute_pixel_edit};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask, selection_all};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{collections::BTreeMap, sync::Arc};

const LAYER: LayerId = LayerId(1);
const PAGE: CanvasSpec = CanvasSpec {
    width_px: 4,
    height_px: 4,
    pixels_per_inch: 96,
};

fn fixture() -> (LayerTree, TileSnapshot) {
    let tree = LayerTree::new(GroupNode {
        id: GroupId(1),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::default(),
        children: vec![LayerTreeNode::Raster(LayerNode {
            mask: None,
            id: LAYER,
            name: "Paint".into(),
            visible: true,
            locked: false,
            reference: true,
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::default(),
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
            color_effect: None,
        })],
    })
    .unwrap();
    let mut tiles = BTreeMap::new();
    for (x, y, color) in [(-3_i32, 1_i32, [0, 128, 0, 128]), (-1, 1, [0, 128, 0, 128])] {
        let data = tiles
            .entry(TileKey::from_pixel(LAYER, 128, x, y))
            .or_insert_with(|| vec![0; TILE_BYTE_LEN]);
        let offset = usize::try_from((y.rem_euclid(128) * 128 + x.rem_euclid(128)) * 4).unwrap();
        data[offset..offset + 4].copy_from_slice(&color);
    }
    (tree, TileSnapshot::from_tiles(tiles).unwrap())
}

fn pixel(tiles: &TileSnapshot, x: i32, y: i32) -> [u8; 4] {
    let offset = usize::try_from((y.rem_euclid(128) * 128 + x.rem_euclid(128)) * 4).unwrap();
    tiles
        .get(TileKey::from_pixel(LAYER, 128, x, y))
        .map_or([0; 4], |tile| {
            tile.pixels()[offset..offset + 4].try_into().unwrap()
        })
}

#[test]
fn page_gradient_preserves_off_page_ink_and_signed_selected_gradient_preserves_alpha() {
    let (mut tree, before) = fixture();
    let mut clipboard = MemoryArtworkClipboard::default();
    for end_color in [[0; 4], [0, 0, 255, 255]] {
        let command = EditCommand::GradientSelection {
            start: [0, 0],
            end: [3, 0],
            start_color: [255, 0, 0, 255],
            end_color,
        };
        let result = execute_pixel_edit(
            command,
            LAYER,
            PAGE,
            &tree,
            &None,
            &before,
            2,
            &mut clipboard,
            EditLimits::default(),
        )
        .unwrap();
        assert!(
            result.selection.is_none(),
            "implicit page mask must not become a persistent selection"
        );
        let after = result.tiles.unwrap();
        assert_eq!(
            pixel(&after, 0, 0),
            if end_color[3] == 0 {
                [213, 0, 0, 213]
            } else {
                [213, 0, 43, 255]
            }
        );
        assert_eq!(pixel(&after, 3, 0), end_color);
        assert_eq!(pixel(&after, -3, 1), pixel(&before, -3, 1));
        assert_eq!(pixel(&after, 4, 0), [0; 4]);
    }
    tree.set_alpha_locked(LAYER, true).unwrap();
    let mask = Arc::new(selection_all([-3, 1], [1, 1], EditLimits::default()).unwrap());
    let selection = Some(mask.clone());
    let result = execute_pixel_edit(
        EditCommand::GradientSelection {
            start: [-2, 1],
            end: [-1, 1],
            start_color: [255, 0, 0, 255],
            end_color: [0, 0, 255, 255],
        },
        LAYER,
        PAGE,
        &tree,
        &selection,
        &before,
        2,
        &mut clipboard,
        EditLimits::default(),
    )
    .unwrap();
    let after = result.tiles.unwrap();
    assert_eq!(pixel(&after, -3, 1), [128, 0, 0, 128]);
    assert_eq!(pixel(&after, -1, 1), pixel(&before, -1, 1));
    assert_eq!(pixel(&after, 0, 0), [0; 4]);
    assert_eq!(result.selection.unwrap().packed_bits(), mask.packed_bits());
}

#[test]
fn signed_wand_combination_preserves_artwork_and_limit_rejection_preserves_selection() {
    let (tree, tiles) = fixture();
    let mut clipboard = MemoryArtworkClipboard::default();
    for source in [
        EditSource::ActiveLayer,
        EditSource::ReferenceLayers,
        EditSource::AllVisible,
    ] {
        let mut selection: Option<Arc<SelectionMask>> = None;
        for (seed, mode, expected) in [
            ([-3, 1], SelectionMode::Replace, 1),
            ([-1, 1], SelectionMode::Add, 2),
            ([-3, 1], SelectionMode::Subtract, 1),
        ] {
            let result = execute_pixel_edit(
                EditCommand::CombineWand {
                    seed,
                    tolerance: 0,
                    source,
                    mode,
                },
                LAYER,
                PAGE,
                &tree,
                &selection,
                &tiles,
                2,
                &mut clipboard,
                EditLimits::default(),
            )
            .unwrap();
            assert!(result.tiles.is_none());
            selection = result.selection;
            assert_eq!(selection.as_ref().unwrap().selected_pixels(), expected);
        }
        let original = selection.as_ref().unwrap().packed_bits();
        assert!(selection.as_ref().unwrap().contains_signed(-1, 1));
        assert!(!selection.as_ref().unwrap().contains_signed(-3, 1));
        let limits = EditLimits {
            max_pixels: 4,
            ..EditLimits::default()
        };
        assert!(
            execute_pixel_edit(
                EditCommand::CombineWand {
                    seed: [-3, 1],
                    tolerance: 0,
                    source,
                    mode: SelectionMode::Replace
                },
                LAYER,
                PAGE,
                &tree,
                &selection,
                &tiles,
                2,
                &mut clipboard,
                limits
            )
            .is_err()
        );
        assert_eq!(
            selection.unwrap().packed_bits(),
            original,
            "domain rejection must not partially replace selection"
        );
    }
}

#[test]
fn completed_gradient_and_wand_use_the_colors_and_mode_captured_at_begin() {
    use nyatidraw_api::{DrawingTool, EditSettings, GradientMode};
    use nyatidraw_editor::gesture::EditGesture;
    use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
    let sample = |x, phase| StylusSample {
        sequence: 1,
        timestamp_ns: 1,
        device_id: 1,
        phase,
        position_document: Point { x, y: 0.0 },
        pressure: 1.0,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons(0),
        eraser: false,
        viewport_revision: 1,
    };
    for mode in [
        GradientMode::ForegroundToTransparent,
        GradientMode::ForegroundToBackground,
    ] {
        let mut colors = [[255, 0, 0, 255], [0, 0, 255, 255]];
        let mut settings = EditSettings {
            gradient_mode: mode,
            ..Default::default()
        };
        let gesture = EditGesture::begin_with_colors(
            DrawingTool::Gradient,
            settings,
            colors,
            false,
            sample(0.0, PointerPhase::Begin),
        );
        settings.gradient_mode = GradientMode::ForegroundToTransparent;
        colors.fill([0; 4]);
        assert_eq!(
            gesture.finish(sample(3.0, PointerPhase::End)).unwrap(),
            EditCommand::GradientSelection {
                start: [0, 0],
                end: [3, 0],
                start_color: [255, 0, 0, 255],
                end_color: if mode == GradientMode::ForegroundToBackground {
                    [0, 0, 255, 255]
                } else {
                    [0; 4]
                },
            }
        );
        assert_eq!(colors[0], [0; 4]);
        assert_eq!(
            settings.gradient_mode,
            GradientMode::ForegroundToTransparent
        );
    }
    for mode in [SelectionMode::Add, SelectionMode::Subtract] {
        let gesture = EditGesture::begin(
            DrawingTool::Wand,
            EditSettings {
                selection_mode: mode,
                ..Default::default()
            },
            [0; 4],
            true,
            sample(-3.0, PointerPhase::Begin),
        );
        assert!(
            matches!(gesture.finish(sample(-1.0, PointerPhase::End)).unwrap(), EditCommand::CombineWand { seed: [-3, 0], mode: captured, .. } if captured == mode)
        );
    }
}
