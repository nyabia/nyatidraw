use super::*;

fn point(x: f64, y: f64, pressure: f32, time_ms: f64) -> StrokePoint {
    StrokePoint {
        x,
        y,
        pressure,
        time_ms,
    }
}

fn stroke(document: &mut WebDocument) {
    document.begin_stroke(point(-2.0, 8.0, 0.5, 1.0)).unwrap();
    document.move_stroke(point(20.0, 12.0, 0.8, 10.0)).unwrap();
    assert!(
        document
            .end_stroke(point(40.0, 18.0, 1.0, 20.0))
            .unwrap()
            .committed
    );
}

fn mutate_record(bytes: &[u8], offset: usize, replacement: &[u8]) -> Vec<u8> {
    let mut changed = bytes.to_vec();
    changed[offset..offset + replacement.len()].copy_from_slice(replacement);
    let end = changed.len() - 32;
    let hash = blake3::hash(&changed[..end]);
    changed[end..].copy_from_slice(hash.as_bytes());
    changed
}

fn malicious_records() -> Vec<Vec<u8>> {
    let fixture = WebDocument::import_rgba8(1, 1, &[255; 4])
        .unwrap()
        .encode_portable()
        .unwrap();
    assert_eq!(fixture[175], 0, "fixture has no color effect");
    assert_eq!(&fixture[176..180], &1_u32.to_le_bytes(), "one tile");
    assert_eq!(fixture[204], 1, "fixture uses run encoding");
    assert_eq!(&fixture[205..209], &12_u32.to_le_bytes(), "two runs");
    assert_eq!(&fixture[209..211], &1_u16.to_le_bytes(), "one opaque pixel");
    [
        (20, vec![5]),
        (29, f32::NAN.to_bits().to_le_bytes().to_vec()),
        (41, vec![2]),
        (104, u128::MAX.to_le_bytes().to_vec()),
        (124, u128::MAX.to_le_bytes().to_vec()),
        (175, vec![2]),
        (176, 1025_u32.to_le_bytes().to_vec()),
        (180, 99_u128.to_le_bytes().to_vec()),
        (196, i32::MIN.to_le_bytes().to_vec()),
        (204, vec![2]),
        (205, 65_537_u32.to_le_bytes().to_vec()),
        (209, 0_u16.to_le_bytes().to_vec()),
        (209, 16_385_u16.to_le_bytes().to_vec()),
        (211, vec![255, 255, 255, 0]),
    ]
    .into_iter()
    .map(|(offset, replacement)| mutate_record(&fixture, offset, &replacement))
    .collect()
}

#[test]
fn shared_brush_history_and_cancellation_preserve_exact_artwork() {
    let mut roots = Vec::new();
    for tool in [
        WebTool::Pencil2H,
        WebTool::Pencil2B,
        WebTool::Pen,
        WebTool::SoftBrush,
    ] {
        let mut first = WebDocument::default();
        let mut second = WebDocument::default();
        for document in [&mut first, &mut second] {
            document.select_tool(tool).unwrap();
            document.set_foreground([220, 50, 30, 255]);
            stroke(document);
        }
        assert_eq!(first.snapshot(), second.snapshot());
        assert_eq!(first.history_len(), 1);
        assert!(first.snapshot().iter().any(|(key, _)| key.x < 0));
        let expected = first.snapshot().clone();
        roots.push(expected.root());
        first.set_foreground([0, 0, 255, 255]);
        first.undo().unwrap();
        assert!(first.snapshot().is_empty());
        first.redo().unwrap();
        assert_eq!(first.snapshot(), &expected);
        assert_eq!(first.foreground(), [0, 0, 255, 255]);
        first.begin_stroke(point(100.0, 100.0, 1.0, 30.0)).unwrap();
        assert!(!first.cancel_stroke().committed);
        assert_eq!(first.snapshot(), &expected);
        assert_eq!(first.history_len(), 1);
        first.begin_stroke(point(100.0, 100.0, 1.0, 40.0)).unwrap();
        assert_eq!(
            first.move_stroke(point(f64::NAN, 0.0, 1.0, 50.0)),
            Err(WebError::InvalidInput)
        );
        assert!(!first.is_drawing());
        assert_eq!(first.snapshot(), &expected);
        let settings = first.brush_settings();
        first
            .set_brush_settings(BrushSettings {
                opacity: 0.0,
                ..settings
            })
            .unwrap();
        first.begin_stroke(point(10.0, 10.0, 1.0, 60.0)).unwrap();
        assert!(
            !first
                .end_stroke(point(50.0, 10.0, 1.0, 70.0))
                .unwrap()
                .committed
        );
        assert_eq!(first.snapshot(), &expected);
        assert_eq!(first.history_len(), 1);
    }
    assert_ne!(roots[0], roots[1]);
    let mut eraser = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
    let before = eraser.snapshot().clone();
    eraser.select_tool(WebTool::Eraser).unwrap();
    eraser.begin_stroke(point(8.0, 8.0, 1.0, 1.0)).unwrap();
    eraser.end_stroke(point(8.0, 8.0, 1.0, 2.0)).unwrap();
    assert_ne!(eraser.snapshot(), &before);
    eraser.undo().unwrap();
    assert_eq!(eraser.snapshot(), &before);
    for (visible, locked, alpha_locked) in [
        (false, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        eraser.set_layer_visible(LayerId(1), visible).unwrap();
        eraser.set_layer_locked(LayerId(1), locked).unwrap();
        eraser
            .set_layer_alpha_locked(LayerId(1), alpha_locked)
            .unwrap();
        let history = eraser.history_len();
        assert_eq!(
            eraser.begin_stroke(point(8.0, 8.0, 1.0, 3.0)),
            Err(WebError::LayerUnavailable)
        );
        assert!(!eraser.is_drawing());
        assert_eq!(eraser.snapshot(), &before);
        assert_eq!(eraser.history_len(), history);
    }
}

#[test]
fn portable_round_trip_keeps_pixels_layers_preferences_and_rejects_damage_atomically() {
    let mut document = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
    document.add_layer("음영").unwrap();
    document.select_tool(WebTool::Pencil2B).unwrap();
    document
        .set_brush_settings(BrushSettings {
            size_px: 12.0,
            smoothing: 15,
            ..document.brush_settings()
        })
        .unwrap();
    document.set_foreground([128, 64, 192, 255]);
    document.set_background([30, 40, 50, 255]);
    stroke(&mut document);
    document
        .set_layer_blend_mode(document.active_layer(), LayerBlendMode::Multiply)
        .unwrap();
    document
        .set_layer_opacity(document.active_layer(), 0.65)
        .unwrap();
    document
        .set_layer_alpha_locked(document.active_layer(), true)
        .unwrap();
    let before = document.export_rgba8().unwrap();
    let bytes = document.encode_portable().unwrap();
    let reopened = WebDocument::decode_portable(&bytes).unwrap();
    assert_eq!(reopened.artwork, document.artwork);
    assert_eq!(reopened.settings, document.settings);
    assert_eq!(reopened.tool(), document.tool());
    assert_eq!(reopened.foreground(), document.foreground());
    assert_eq!(reopened.background(), document.background());
    assert_eq!(reopened.export_rgba8().unwrap(), before);
    assert_eq!(reopened.encode_portable().unwrap(), bytes);
    assert!(!reopened.can_undo());
    let mut invalid = vec![vec![], bytes[..bytes.len() - 1].to_vec()];
    let mut checksum = bytes.clone();
    checksum[30] ^= 1;
    invalid.push(checksum);
    for (offset, value) in [(8, 0_u32), (8, 4097), (120, u32::MAX)] {
        invalid.push(mutate_record(&bytes, offset, &value.to_le_bytes()));
    }
    invalid.push(mutate_record(&bytes, 176, &1_u128.to_le_bytes()));
    invalid.extend(malicious_records());
    for bytes in invalid {
        assert!(WebDocument::decode_portable(&bytes).is_err());
        assert_eq!(document.export_rgba8().unwrap(), before);
    }
    assert!(WebDocument::import_rgba8(u32::MAX, 1, &[]).is_err());
    assert!(WebDocument::import_rgba8(2, 2, &[0; 15]).is_err());
}

#[test]
fn portable_blend_modes_preserve_raw_artwork_and_composite_after_reopen() {
    let mut document = WebDocument::import_rgba8(16, 16, &[160; 16 * 16 * 4]).unwrap();
    document.add_layer("light").unwrap();
    document.set_foreground([240, 110, 40, 200]);
    stroke(&mut document);
    let original = document.snapshot().clone();
    for mode in [
        LayerBlendMode::Normal,
        LayerBlendMode::Multiply,
        LayerBlendMode::Screen,
        LayerBlendMode::Add,
        LayerBlendMode::Overlay,
    ] {
        document
            .set_layer_blend_mode(document.active_layer(), mode)
            .unwrap();
        let expected = document.export_rgba8().unwrap();
        let bytes = document.encode_portable().unwrap();
        let reopened = WebDocument::decode_portable(&bytes).unwrap();
        assert_eq!(reopened.layers(), document.layers());
        assert_eq!(
            reopened.snapshot(),
            &original,
            "mode changes must not repaint raw pixels"
        );
        assert_eq!(reopened.export_rgba8().unwrap(), expected);
        assert_eq!(reopened.encode_portable().unwrap(), bytes);
    }
}

#[test]
fn layer_color_metadata_preserves_raw_artwork_history_and_portable_recovery() {
    // Risk: display colorization overwrites pixels or loses its settings during undo/recovery.
    use nyatidraw_api::LayerCommand;
    for enabled in [true, false] {
        let mut document = WebDocument::import_rgba8(
            4,
            1,
            &[
                0, 0, 0, 255, 255, 255, 255, 255, 128, 128, 128, 128, 0, 0, 0, 0,
            ],
        )
        .unwrap();
        let layer = document.active_layer();
        let original = document.snapshot().clone();
        let original_export = document.export_rgba8().unwrap();
        let effect = LayerColorEffect {
            enabled,
            dark_rgb: [24, 80, 210],
            light_rgb: [245, 220, 190],
        };
        let command = LayerCommand::SetColorEffect {
            layer,
            effect: Some(effect),
        };
        let update = document.apply_layer_command(command.clone()).unwrap();
        assert!(update.committed && update.structure_changed);
        assert_eq!(document.history_len(), 1);
        assert_eq!(document.snapshot(), &original);
        assert!(!document.apply_layer_command(command).unwrap().committed);
        assert_eq!(document.history_len(), 1);
        let colorized = document.export_rgba8().unwrap();
        assert_eq!(colorized == original_export, !enabled);
        assert_eq!(colorized[3], original_export[3]);
        assert_eq!(colorized[11], original_export[11]);
        document.undo().unwrap();
        assert_eq!(document.layers().raster(layer).unwrap().color_effect, None);
        assert_eq!(document.export_rgba8().unwrap(), original_export);
        document.redo().unwrap();
        let portable = document.encode_portable().unwrap();
        assert_eq!(portable[175], 1, "effect metadata is present");
        assert_eq!(portable[176], u8::from(enabled));
        assert!(WebDocument::decode_portable(&mutate_record(&portable, 176, &[2])).is_err());
        let reopened = WebDocument::decode_portable(&portable).unwrap();
        assert_eq!(
            reopened.layers().raster(layer).unwrap().color_effect,
            Some(effect)
        );
        assert_eq!(reopened.snapshot(), &original);
        assert_eq!(reopened.export_rgba8().unwrap(), colorized);
    }

    let original = WebDocument::import_rgba8(1, 1, &[255; 4]).unwrap();
    let mut legacy = original.encode_portable().unwrap();
    legacy.truncate(legacy.len() - 32);
    assert_eq!(legacy.remove(175), 0);
    legacy[..8].copy_from_slice(b"NYWEB001");
    let hash = blake3::hash(&legacy);
    legacy.extend_from_slice(hash.as_bytes());
    let reopened = WebDocument::decode_portable(&legacy).unwrap();
    assert_eq!(reopened.snapshot(), original.snapshot());
    assert_eq!(
        reopened
            .layers()
            .raster(original.active_layer())
            .unwrap()
            .color_effect,
        None
    );
}

#[test]
fn bounded_history_keeps_latest_edits_and_noops_do_not_displace_undo() {
    let mut document = WebDocument::default();
    assert!(!document.clear_active_layer().unwrap().committed);
    assert_eq!(document.history_len(), 0);
    for index in 0..140 {
        document
            .rename_layer(LayerId(1), &format!("Layer {index}"))
            .unwrap();
    }
    assert_eq!(document.history_len(), MAX_HISTORY_ENTRIES);
    assert!(
        !document
            .rename_layer(LayerId(1), "Layer 139")
            .unwrap()
            .committed
    );
    for _ in 0..MAX_HISTORY_ENTRIES {
        document.undo().unwrap();
    }
    assert!(!document.can_undo());
    assert!(document.can_redo());
    let before = document.snapshot().clone();
    document.begin_stroke(point(0.0, 0.0, 1.0, 1.0)).unwrap();
    assert_eq!(
        document.move_stroke(point(32_000.0, 32_000.0, 1.0, 2.0)),
        Err(WebError::LimitExceeded)
    );
    assert_eq!(document.snapshot(), &before);
    assert!(document.can_redo());
    document.rename_layer(LayerId(1), "New branch").unwrap();
    assert!(!document.can_redo());
    let state = document.artwork.clone();
    assert!(
        document
            .set_canvas(CanvasSpec {
                width_px: 4097,
                height_px: 1,
                pixels_per_inch: 96
            })
            .is_err()
    );
    assert_eq!(document.artwork, state);
}
#[test]
fn shared_selection_fill_and_transform_preserve_undo_and_cancelled_artwork() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, LayerCommand, TransformCommand as T};
    let mut doc = WebDocument::new(CanvasSpec {
        width_px: 32,
        height_px: 32,
        pixels_per_inch: 96,
    })
    .unwrap();
    doc.apply_edit(E::SelectLasso {
        vertices: vec![[2, 2], [10, 2], [10, 10], [2, 10]],
    })
    .unwrap();
    doc.apply_edit(E::FillSelection {
        color: [255, 0, 0, 255],
    })
    .unwrap();
    let painted = doc.snapshot().clone();
    assert!(!painted.is_empty());
    doc.apply_edit(E::CopySelection).unwrap();
    let original_tree = doc.layers().clone();
    doc.apply_edit(E::FreeTransform(T::Paste)).unwrap();
    assert_eq!(
        doc.layers(),
        &original_tree,
        "provisional paste must not enter autosave"
    );
    doc.apply_edit(E::FreeTransform(T::Cancel)).unwrap();
    assert_eq!(doc.snapshot(), &painted);
    doc.apply_edit(E::FreeTransform(T::Begin)).unwrap();
    let transform = AffineTransform {
        offset_milli: [4000, 0],
        ..Default::default()
    };
    doc.apply_edit(E::FreeTransform(T::Preview(transform)))
        .unwrap();
    let generation = doc.transform_projection().unwrap().generation;
    assert!(
        doc.apply_edit(E::FreeTransform(T::Commit {
            generation: generation - 1
        }))
        .is_err()
    );
    doc.apply_edit(E::FreeTransform(T::Commit { generation }))
        .unwrap();
    assert_ne!(doc.snapshot(), &painted);
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &painted);
    doc.apply_layer_command(LayerCommand::DuplicateRaster(doc.active_layer()))
        .unwrap();
    let duplicate = doc.active_layer();
    assert!(doc.snapshot().iter().any(|(key, _)| key.layer == duplicate));
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &painted);
}

#[test]
fn inserting_import_or_duplicate_cannot_rebind_existing_clipped_artwork() {
    use nyatidraw_api::{
        EditCommand as E, LayerCommand as L, LayerTreeNodeId as N, RasterImport,
        TransformCommand as T,
    };
    let mut pixels = vec![0; 16 * 4];
    pixels[..4].copy_from_slice(&[255, 0, 0, 255]);
    let mut doc = WebDocument::import_rgba8(16, 1, &pixels).unwrap();
    let base = doc.active_layer();
    doc.apply_layer_command(L::AddRaster).unwrap();
    let clip = doc.active_layer();
    doc.apply_edit(E::SelectAll).unwrap();
    doc.apply_edit(E::FillSelection {
        color: [0, 0, 255, 255],
    })
    .unwrap();
    doc.apply_layer_command(L::SetClipToBelow {
        node: N::Raster(clip),
        clip_to_below: true,
    })
    .unwrap();
    doc.set_active_layer(base).unwrap();
    let expected = doc.export_rgba8().unwrap()[..4].to_vec();
    for duplicate in [false, true] {
        if duplicate {
            doc.apply_layer_command(L::DuplicateRaster(base)).unwrap();
            doc.apply_edit(E::ClearActiveLayer).unwrap();
        } else {
            doc.apply_edit(E::FreeTransform(T::ImportRaster(RasterImport {
                project_epoch: 1,
                name: "image".into(),
                size: [1, 1],
                pixels: std::sync::Arc::from([0, 255, 0, 255]),
            })))
            .unwrap();
            let preview = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(
                doc.display_snapshot(),
                doc.display_layers(),
                doc.canvas(),
            )
            .unwrap();
            assert_eq!(&preview.pixels[..4], &[0, 0, 255, 255]);
            let generation = doc.transform_projection().unwrap().generation;
            doc.apply_edit(E::FreeTransform(T::Commit { generation }))
                .unwrap();
        }
        assert_eq!(&doc.export_rgba8().unwrap()[..4], expected.as_slice());
        doc.undo().unwrap();
        if duplicate {
            doc.undo().unwrap();
        }
        doc.set_active_layer(base).unwrap();
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn group_transform_preserves_hidden_children_structure_cancel_and_single_undo() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, GroupId, TransformCommand as T};
    let mut doc = WebDocument::new(CanvasSpec {
        width_px: 16,
        height_px: 16,
        pixels_per_inch: 96,
    })
    .unwrap();
    let mut root = doc.layers().root().clone();
    let mut clipped = new_layer(LayerId(2), "shade");
    clipped.clip_to_below = true;
    clipped.blend_mode = LayerBlendMode::Multiply;
    clipped.color_effect = Some(LayerColorEffect::default());
    let mut hidden = new_layer(LayerId(3), "hidden");
    hidden.visible = false;
    let nested = GroupNode {
        id: GroupId(11),
        name: "nested".into(),
        visible: false,
        opacity_u16: 32768,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Screen,
        children: vec![LayerTreeNode::Raster(hidden)],
    };
    root.children = vec![
        LayerTreeNode::Group(GroupNode {
            id: GroupId(10),
            name: "subject".into(),
            visible: true,
            opacity_u16: 45000,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Overlay,
            children: vec![
                LayerTreeNode::Raster(new_layer(LayerId(1), "base")),
                LayerTreeNode::Raster(clipped),
                LayerTreeNode::Group(nested),
            ],
        }),
        LayerTreeNode::Raster(new_layer(LayerId(4), "outside")),
    ];
    doc.artwork.layers = LayerTree::new(root).unwrap();
    doc.artwork.active = LayerId(4);
    doc.artwork.tiles = TileSnapshot::from_tiles(
        [
            (1, [-1, 1], [128, 0, 0, 128]),
            (2, [1, 1], [0, 64, 0, 64]),
            (3, [2, 1], [0, 0, 255, 255]),
            (4, [8, 1], [70, 70, 70, 100]),
        ]
        .map(|(id, [x, y], rgba)| {
            let key = TileKey::from_pixel(LayerId(id), 128, x, y);
            let mut pixels = vec![0; TILE_BYTE_LEN];
            let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
            pixels[offset..offset + 4].copy_from_slice(&rgba);
            (key, pixels)
        }),
    )
    .unwrap();
    let original = doc.artwork.clone();
    for alpha in [false, true] {
        if alpha {
            doc.artwork
                .layers
                .set_alpha_locked(LayerId(3), true)
                .unwrap();
        } else {
            doc.artwork.layers.set_locked(LayerId(3), true).unwrap();
        }
        assert!(
            doc.apply_edit(E::FreeTransform(T::BeginGroup(GroupId(10))))
                .is_err()
        );
        assert!(doc.transform_projection().is_none());
        assert_eq!(doc.snapshot(), &original.tiles);
        doc.artwork.layers = original.layers.clone();
    }
    let shift = AffineTransform {
        offset_milli: [2000, 1000],
        ..Default::default()
    };
    let mask = std::sync::Arc::new(
        nyatidraw_paint_cpu::selection_all(
            [-1, 1],
            [1, 1],
            nyatidraw_paint_cpu::EditLimits::default(),
        )
        .unwrap(),
    );
    doc.selection = Some(mask.clone());
    doc.apply_edit(E::FreeTransform(T::BeginGroup(GroupId(10))))
        .unwrap();
    assert_eq!(doc.transform_projection().unwrap().size, [1, 1]);
    doc.apply_edit(E::FreeTransform(T::Preview(shift))).unwrap();
    for id in [LayerId(2), LayerId(3)] {
        let key = TileKey::from_pixel(id, 128, 1, 1);
        assert_eq!(doc.display_snapshot().get(key), original.tiles.get(key));
    }
    doc.apply_edit(E::FreeTransform(T::Cancel)).unwrap();
    assert_eq!(doc.selection, Some(mask));
    doc.selection = None;
    for commit in [false, true] {
        let history = doc.history_len();
        doc.apply_edit(E::FreeTransform(T::BeginGroup(GroupId(10))))
            .unwrap();
        assert_eq!(doc.transform_projection().unwrap().origin, [-1, 1]);
        assert_eq!(doc.transform_projection().unwrap().size, [4, 1]);
        doc.apply_edit(E::FreeTransform(T::Preview(shift))).unwrap();
        assert_eq!(
            doc.artwork, original,
            "previews must not change durable artwork"
        );
        assert_eq!(doc.display_layers(), &original.layers);
        assert_eq!(doc.display_active_layer(), LayerId(4));
        let preview = doc.display_snapshot().clone();
        for (id, x, color) in [
            (1, 1, [128, 0, 0, 128]),
            (2, 3, [0, 64, 0, 64]),
            (3, 4, [0, 0, 255, 255]),
        ] {
            let key = TileKey::from_pixel(LayerId(id), 128, x, 2);
            let offset = usize::try_from(2 * 128 + x).unwrap() * 4;
            assert_eq!(
                &preview.get(key).unwrap().pixels()[offset..offset + 4],
                &color
            );
        }
        let outside = TileKey::from_pixel(LayerId(4), 128, 8, 1);
        assert_eq!(preview.get(outside), original.tiles.get(outside));
        if commit {
            let generation = doc.transform_projection().unwrap().generation;
            doc.apply_edit(E::FreeTransform(T::Commit { generation }))
                .unwrap();
            assert_eq!(doc.snapshot(), &preview);
            assert_eq!(doc.layers(), &original.layers);
            assert_eq!(doc.history_len(), history + 1);
            doc.undo().unwrap();
            assert_eq!(doc.artwork, original);
            doc.redo().unwrap();
            assert_eq!(doc.snapshot(), &preview);
        } else {
            doc.apply_edit(E::FreeTransform(T::Cancel)).unwrap();
            assert_eq!(doc.artwork, original);
            assert!(doc.selection().is_none());
            assert_eq!(doc.history_len(), history);
        }
    }
}

#[test]
fn image_import_preserves_page_clipboard_and_cancel_then_commits_one_undo() {
    use nyatidraw_api::{EditCommand as E, RasterImport, TransformCommand as T};
    let mut doc = WebDocument::import_rgba8(2, 2, &[255, 0, 0, 255].repeat(4)).unwrap();
    doc.apply_edit(E::SelectAll).unwrap();
    doc.apply_edit(E::CopySelection).unwrap();
    let clipboard = doc.clipboard.0.clone();
    let before = doc.snapshot().clone();
    let tree = doc.layers().clone();
    let canvas = doc.canvas();
    let selection = doc.selection().cloned();
    let history = doc.undo_len();
    let import = RasterImport {
        project_epoch: 1,
        name: "외부 PNG".into(),
        size: [4, 2],
        pixels: std::sync::Arc::from([0, 80, 0, 128].repeat(8)),
    };
    let mut invalid = import.clone();
    invalid.size = [u32::MAX, u32::MAX];
    assert!(
        doc.apply_edit(E::FreeTransform(T::ImportRaster(invalid)))
            .is_err()
    );
    for cancel in [true, false] {
        doc.apply_edit(E::FreeTransform(T::ImportRaster(import.clone())))
            .unwrap();
        assert_eq!(
            doc.snapshot(),
            &before,
            "unconfirmed import must stay out of autosave"
        );
        assert_eq!(doc.layers(), &tree);
        assert_eq!(doc.transform_projection().unwrap().origin, [-1, 0]);
        assert_eq!(doc.canvas(), canvas);
        assert_eq!(doc.clipboard.0, clipboard);
        if cancel {
            doc.apply_edit(E::FreeTransform(T::Cancel)).unwrap();
            assert_eq!(doc.selection(), selection.as_ref());
            assert_eq!(doc.undo_len(), history);
        } else {
            let generation = doc.transform_projection().unwrap().generation;
            doc.apply_edit(E::FreeTransform(T::Commit { generation }))
                .unwrap();
            assert_eq!(doc.undo_len(), history + 1);
            assert_ne!(doc.snapshot(), &before);
            assert_eq!(
                doc.layers().raster(doc.active_layer()).unwrap().name,
                "외부 PNG"
            );
            doc.undo().unwrap();
            assert_eq!(doc.snapshot(), &before);
            assert_eq!(doc.layers(), &tree);
        }
    }
}

#[test]
fn color_preview_never_enters_saved_artwork_and_confirmation_is_one_reversible_edit() {
    // Risk: previews leak into save/history or committed color leaves composite readers stale.
    use nyatidraw_api::{ColorAdjustment, ColorAdjustmentCommand as C, EditCommand as E};
    let mut doc = WebDocument::import_rgba8(2, 1, &[255, 0, 0, 255, 60, 100, 160, 128]).unwrap();
    let original = doc.snapshot().clone();
    let original_history = doc.undo_len();
    let hue = ColorAdjustment {
        hue_degrees: 120,
        ..Default::default()
    };
    assert!(
        !doc.apply_edit(E::ColorAdjustment(C::Begin))
            .unwrap()
            .changed
    );
    assert!(
        !doc.apply_edit(E::ColorAdjustment(C::Preview(hue)))
            .unwrap()
            .changed
    );
    let preview = doc.display_snapshot().clone();
    let generation = doc.color_adjustment_projection().unwrap().generation;
    assert_ne!(preview, original);
    assert_eq!(doc.snapshot(), &original);
    assert_eq!(doc.undo_len(), original_history);
    assert!(doc.apply_edit(E::ClearActiveLayer).is_err());
    assert!(doc.undo().is_err());
    assert!(
        doc.apply_edit(E::ColorAdjustment(C::Commit {
            generation: generation - 1
        }))
        .is_err()
    );
    doc.apply_edit(E::ColorAdjustment(C::Preview(hue))).unwrap();
    assert_eq!(
        doc.display_snapshot(),
        &preview,
        "preview must restart from pinned original"
    );
    doc.apply_edit(E::ColorAdjustment(C::Preview(ColorAdjustment::default())))
        .unwrap();
    assert_eq!(doc.display_snapshot(), &original);
    assert!(
        !doc.apply_edit(E::ColorAdjustment(C::Cancel))
            .unwrap()
            .changed
    );
    assert_eq!(doc.snapshot(), &original);
    assert_eq!(doc.undo_len(), original_history);
    doc.apply_edit(E::ColorAdjustment(C::Begin)).unwrap();
    doc.apply_edit(E::ColorAdjustment(C::Preview(hue))).unwrap();
    assert!(
        doc.apply_edit(E::ColorAdjustment(C::Preview(ColorAdjustment {
            hue_degrees: 181,
            ..Default::default()
        })))
        .is_err()
    );
    let invalid = doc.color_adjustment_projection().unwrap();
    assert!(!invalid.can_commit);
    assert!(
        doc.apply_edit(E::ColorAdjustment(C::Commit {
            generation: invalid.generation
        }))
        .is_err()
    );
    doc.apply_edit(E::ColorAdjustment(C::Preview(hue))).unwrap();
    let generation = doc.color_adjustment_projection().unwrap().generation;
    let committed = doc
        .apply_edit(E::ColorAdjustment(C::Commit { generation }))
        .unwrap();
    assert!(committed.changed && committed.committed);
    assert_eq!(doc.snapshot(), &preview);
    assert_eq!(doc.undo_len(), original_history + 1);
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &original);
    doc.redo().unwrap();
    assert_eq!(doc.snapshot(), &preview);
    doc.apply_edit(E::ColorAdjustment(C::Begin)).unwrap();
    let generation = doc.color_adjustment_projection().unwrap().generation;
    let neutral = doc
        .apply_edit(E::ColorAdjustment(C::Commit { generation }))
        .unwrap();
    assert!(!neutral.changed && !neutral.committed);
    assert_eq!(doc.undo_len(), original_history + 1);
}

#[test]
fn direct_artwork_move_preserves_off_page_pixels_and_undo_as_one_edit() {
    use nyatidraw_api::EditCommand as E;
    let mut doc = WebDocument::new(CanvasSpec {
        width_px: 16,
        height_px: 16,
        pixels_per_inch: 96,
    })
    .unwrap();
    for (left, top) in [(-3, 2), (4, 5)] {
        doc.apply_edit(E::SelectLasso {
            vertices: vec![
                [left, top],
                [left + 1, top],
                [left + 1, top + 1],
                [left, top + 1],
            ],
        })
        .unwrap();
        doc.apply_edit(E::FillSelection {
            color: [255, 0, 0, 255],
        })
        .unwrap();
    }
    doc.apply_edit(E::ClearSelection).unwrap();
    let original = doc.snapshot().clone();
    let before = doc.history_len();
    doc.apply_edit(E::MoveArtwork { offset: [6, -4] }).unwrap();
    assert_eq!(doc.history_len(), before + 1);
    assert_ne!(doc.snapshot(), &original);
    let moved = doc.snapshot().clone();
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &original);
    doc.redo().unwrap();
    assert_eq!(doc.snapshot(), &moved);
    doc.apply_edit(E::CenterArtwork {
        horizontal: true,
        vertical: true,
    })
    .unwrap();
    assert_ne!(doc.snapshot(), &moved);
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &moved);
    doc.apply_edit(E::SelectLasso {
        vertices: vec![[10, 1], [11, 1], [11, 2], [10, 2]],
    })
    .unwrap();
    let selected = doc.selection().unwrap().bounds_signed();
    let before_selected_move = doc.snapshot().clone();
    doc.apply_edit(E::MoveArtwork { offset: [2, 3] }).unwrap();
    assert_eq!(
        doc.selection().unwrap().bounds_signed(),
        selected.map(|(origin, size)| ([origin[0] + 2, origin[1] + 3], size))
    );
    assert_ne!(doc.snapshot(), &before_selected_move);
    doc.undo().unwrap();
    assert_eq!(doc.snapshot(), &before_selected_move);
    let bytes = doc.encode_portable().unwrap();
    let reopened = WebDocument::decode_portable(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), &moved);
}

#[test]
fn hidden_or_locked_fill_targets_preserve_artwork_selection_and_undo() {
    use nyatidraw_api::{EditCommand as E, EditSource, FillSettings, GroupId};
    use nyatidraw_document::GroupNode;

    for (layer_visible, parent_visible, ancestor_visible, locked) in [
        (false, true, true, false),
        (true, false, true, false),
        (true, true, false, false),
        (true, true, true, true),
        (true, true, true, false),
    ] {
        let mut doc = WebDocument::import_rgba8(8, 8, &[255; 8 * 8 * 4]).unwrap();
        let mut root = doc.layers().root().clone();
        let LayerTreeNode::Raster(layer) = &mut root.children[0] else {
            panic!("fixture requires a raster");
        };
        layer.visible = layer_visible;
        layer.locked = locked;
        let parent = GroupNode {
            id: GroupId(2),
            name: "parent".into(),
            visible: parent_visible,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            children: std::mem::take(&mut root.children),
        };
        root.children = vec![LayerTreeNode::Group(GroupNode {
            id: GroupId(3),
            name: "ancestor".into(),
            visible: ancestor_visible,
            children: vec![LayerTreeNode::Group(parent.clone())],
            ..parent
        })];
        doc.artwork.layers = LayerTree::new(root).unwrap();
        doc.apply_edit(E::SelectLasso {
            vertices: vec![[1, 1], [5, 1], [5, 5], [1, 5]],
        })
        .unwrap();
        let before = doc.artwork.clone();
        let selection = doc.selection.clone();
        let color = [255, 0, 0, 255];
        for command in [
            E::FillSelection { color },
            E::FloodFill {
                seed: [2, 2],
                tolerance: 0,
                source: EditSource::ActiveLayer,
                color,
            },
            E::FloodFillAdvanced {
                seed: [2, 2],
                tolerance: 0,
                source: EditSource::ActiveLayer,
                color,
                settings: FillSettings::default(),
            },
            E::GradientSelection {
                start: [1, 1],
                end: [5, 5],
                start_color: color,
                end_color: [0, 0, 255, 255],
            },
        ] {
            doc.selection = if matches!(command, E::FloodFill { .. }) {
                None
            } else {
                selection.clone()
            };
            let current_selection = doc.selection.clone();
            let history = doc.history_len();
            let undo = doc.undo_len();
            let result = doc.apply_edit(command);
            if layer_visible && parent_visible && ancestor_visible && !locked {
                assert!(result.unwrap().committed);
                assert_ne!(doc.snapshot(), &before.tiles);
                assert_eq!(doc.selection, current_selection);
                assert_eq!(doc.undo_len(), undo + 1);
                doc.undo().unwrap();
                assert_eq!(doc.undo_len(), undo);
                doc.selection.clone_from(&current_selection);
            } else {
                assert!(matches!(result, Err(WebError::EditRejected(_))));
                assert_eq!(doc.history_len(), history);
            }
            assert_eq!(doc.artwork, before);
            assert_eq!(doc.selection, current_selection);
        }
    }
}

#[test]
fn brush_cannot_change_pixels_outside_the_selection() {
    use nyatidraw_api::EditCommand as E;
    let mut doc = WebDocument::new(CanvasSpec {
        width_px: 32,
        height_px: 32,
        pixels_per_inch: 96,
    })
    .unwrap();
    doc.select_tool(WebTool::Pen).unwrap();
    doc.apply_edit(E::SelectLasso {
        vertices: vec![[4, 4], [8, 4], [8, 8], [4, 8]],
    })
    .unwrap();
    let point = StrokePoint {
        x: 4.0,
        y: 4.0,
        pressure: 1.0,
        time_ms: 1.0,
    };
    doc.begin_stroke(point).unwrap();
    doc.end_stroke(point).unwrap();
    let pixels = doc.export_rgba8().unwrap();
    assert!(
        pixels
            .chunks_exact(4)
            .enumerate()
            .all(|(index, pixel)| pixel[3] == 0
                || ((4..8).contains(&(index % 32)) && (4..8).contains(&(index / 32))))
    );
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
}

#[test]
fn switching_tools_commits_moved_or_pasted_artwork_without_recording_unchanged_drafts() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, TransformCommand as T};
    for pasted in [false, true] {
        for moved in [false, true] {
            let mut document = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
            let original = document.snapshot().clone();
            let tree = document.layers().clone();
            let history = document.history_len();
            document.apply_edit(E::SelectAll).unwrap();
            document.apply_edit(E::CopySelection).unwrap();
            document
                .apply_edit(E::FreeTransform(if pasted { T::Paste } else { T::Begin }))
                .unwrap();
            if moved {
                document
                    .apply_edit(E::FreeTransform(T::Preview(AffineTransform {
                        offset_milli: [4000, 0],
                        ..Default::default()
                    })))
                    .unwrap();
            }
            let preview = document.display_snapshot().clone();
            if moved {
                assert!(
                    document
                        .apply_edit(E::FreeTransform(T::Preview(AffineTransform {
                            scale_ppm: [0, 0],
                            ..Default::default()
                        })))
                        .is_err()
                );
                assert!(!document.transform_projection().unwrap().can_commit);
                assert_eq!(document.display_snapshot(), &preview);
            }
            let update = document.finish_transform_for_tool_switch().unwrap();
            assert!(document.transform_projection().is_none());
            assert_eq!(update.committed, pasted || moved);
            assert_eq!(document.snapshot(), &preview);
            assert_eq!(
                document.history_len(),
                history + usize::from(pasted || moved)
            );
            let portable = document.encode_portable().unwrap();
            let reopened = WebDocument::decode_portable(&portable).unwrap();
            assert_eq!(reopened.snapshot(), document.snapshot());
            assert_eq!(reopened.layers(), document.layers());
            if pasted || moved {
                document.undo().unwrap();
                assert_eq!(document.snapshot(), &original);
                assert_eq!(document.layers(), &tree);
            }
        }
    }
}

#[test]
fn coverage_admission_and_history_cannot_bypass_browser_resource_limits() {
    use nyatidraw_api::MaskId;
    use nyatidraw_document::LayerMask;
    use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey, CoverageTileObject};
    let mut document = WebDocument::new(CanvasSpec {
        width_px: 16,
        height_px: 16,
        pixels_per_inch: 96,
    })
    .unwrap();
    let active = document.active_layer();
    document
        .artwork
        .layers
        .set_mask(
            active,
            Some(LayerMask {
                id: MaskId(2),
                enabled: true,
                default_coverage: 255,
            }),
        )
        .unwrap();
    let key = CoverageTileKey {
        mask: MaskId(2),
        mip: 0,
        x: -1,
        y: 0,
    };
    let tile = CoverageTileObject::new(vec![0; COVERAGE_TILE_BYTE_LEN]).unwrap();
    document.artwork.tiles = TileSnapshot::from_planes([], [(key, tile.clone())]).unwrap();
    let bytes = document.resident_bytes();
    assert!(bytes >= COVERAGE_TILE_BYTE_LEN);
    document.undo.push_back(document.artwork.clone());
    let metadata =
        document.artwork.tiles.total_len() * 256 + editing::node_count(document.layers()) * 1024;
    assert_eq!(document.resident_bytes(), bytes + metadata);
    document.artwork.tiles = document
        .artwork
        .tiles
        .with_coverage_replacements([(key, vec![128; COVERAGE_TILE_BYTE_LEN])])
        .unwrap();
    assert_eq!(
        document.resident_bytes(),
        bytes + metadata + COVERAGE_TILE_BYTE_LEN
    );
    for invalid in [
        CoverageTileKey {
            mask: MaskId(99),
            ..key
        },
        CoverageTileKey { x: 258, ..key },
    ] {
        let snapshot = TileSnapshot::from_planes([], [(invalid, tile.clone())]).unwrap();
        assert!(
            WebDocument::from_snapshot(
                document.canvas(),
                snapshot,
                document.layers().clone(),
                active
            )
            .is_err()
        );
    }
    let over_limit = TileSnapshot::from_planes(
        [],
        (0..=MAX_RESIDENT_TILES).map(|index| {
            (
                CoverageTileKey {
                    x: i32::try_from(index % 257).unwrap(),
                    y: i32::try_from(index / 257).unwrap(),
                    ..key
                },
                tile.clone(),
            )
        }),
    )
    .unwrap();
    assert!(matches!(
        WebDocument::from_snapshot(
            document.canvas(),
            over_limit,
            document.layers().clone(),
            active
        ),
        Err(WebError::LimitExceeded)
    ));
}
