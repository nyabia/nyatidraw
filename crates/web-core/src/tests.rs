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
    [
        (20, vec![5]),
        (29, f32::NAN.to_bits().to_le_bytes().to_vec()),
        (41, vec![2]),
        (104, u128::MAX.to_le_bytes().to_vec()),
        (124, u128::MAX.to_le_bytes().to_vec()),
        (175, 1025_u32.to_le_bytes().to_vec()),
        (179, 99_u128.to_le_bytes().to_vec()),
        (195, i32::MIN.to_le_bytes().to_vec()),
        (203, vec![2]),
        (204, 65_537_u32.to_le_bytes().to_vec()),
        (208, 0_u16.to_le_bytes().to_vec()),
        (208, 16_385_u16.to_le_bytes().to_vec()),
        (210, vec![255, 255, 255, 0]),
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
    invalid.push(mutate_record(&bytes, 175, &1_u128.to_le_bytes()));
    invalid.extend(malicious_records());
    for bytes in invalid {
        assert!(WebDocument::decode_portable(&bytes).is_err());
        assert_eq!(document.export_rgba8().unwrap(), before);
    }
    assert!(WebDocument::import_rgba8(u32::MAX, 1, &[]).is_err());
    assert!(WebDocument::import_rgba8(2, 2, &[0; 15]).is_err());
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
