use crate::WebDocument;
use nyatidraw_api::{AnimationCommand as A, EditCommand, LayerCommand};

#[test]
fn editing_playback_and_onion_never_mutate_another_cel_or_export_virtual_pixels() {
    let mut document = WebDocument::import_rgba8(2, 2, &[255; 16]).unwrap();
    let original_layer = document.active_layer();
    let original_tiles = document.snapshot().clone();
    document.apply_animation_command(A::Enable).unwrap();
    let first = document.active_frame().unwrap();
    document
        .apply_animation_command(A::Duplicate(first))
        .unwrap();
    let second = document.active_frame().unwrap();
    assert_ne!(document.active_layer(), original_layer);
    document.apply_edit(EditCommand::ClearActiveLayer).unwrap();
    assert_eq!(document.snapshot(), &original_tiles);
    assert_eq!(document.sample_rgba8(0, 0).unwrap(), [0; 4]);
    document
        .apply_layer_command(LayerCommand::AddRaster)
        .unwrap();
    let added = document.active_layer();
    document.delete_layer(added).unwrap();
    assert_eq!(document.snapshot(), &original_tiles);
    let history = document.undo_len();
    document.apply_animation_command(A::Select(first)).unwrap();
    assert_eq!(document.undo_len(), history);
    assert_eq!(document.active_layer(), original_layer);
    let exported = document.export_rgba8().unwrap();
    let tree = document.layers().clone();
    document.set_playback_frame(Some(second)).unwrap();
    assert_eq!(document.active_frame(), Some(first));
    assert_eq!(document.active_layer(), original_layer);
    assert_eq!(document.export_rgba8().unwrap(), exported);
    document.set_playback_frame(None).unwrap();
    let display = nyatidraw_editor::animation_display::build_frame_display(
        document.layers(),
        document.snapshot(),
        document.animation_timeline(),
        Some(first),
        document.canvas(),
        true,
        128 * 1024 * 1024,
    )
    .unwrap();
    document.set_animation_display(Some(display));
    assert_eq!(document.layers(), &tree);
    assert_eq!(document.snapshot(), &original_tiles);
    assert_eq!(document.export_rgba8().unwrap(), exported);
    assert_eq!(document.undo_len(), history);
    assert!(document.encode_portable().is_err());
}

#[test]
fn timeline_history_and_resource_rejection_preserve_atomic_artwork() {
    let mut document = WebDocument::default();
    document.apply_animation_command(A::Enable).unwrap();
    let frame = document.active_frame().unwrap();
    let original = document.animation_timeline().unwrap().clone();
    let root = document.snapshot().root();
    let history = document.undo_len();
    assert!(
        document
            .apply_animation_command_checked(A::Duplicate(frame), |_, _| Err("budget".into()))
            .is_err()
    );
    assert_eq!(document.animation_timeline(), Some(&original));
    assert_eq!(document.undo_len(), history);
    document
        .apply_animation_command(A::SetDuration {
            frame,
            duration_ms: 234,
        })
        .unwrap();
    assert_eq!(document.snapshot().root(), root);
    assert_eq!(
        document.animation_timeline().unwrap().frames()[0].duration_ms,
        234
    );
    document.undo().unwrap();
    assert_eq!(document.animation_timeline(), Some(&original));
    document.redo().unwrap();
    assert_eq!(
        document.animation_timeline().unwrap().frames()[0].duration_ms,
        234
    );
    document
        .apply_animation_command(A::InsertBlank { after: frame })
        .unwrap();
    document.undo().unwrap();
    assert_eq!(document.active_frame(), Some(frame));
    assert!(
        document
            .render_layers()
            .raster(document.active_layer())
            .is_some()
    );
}
