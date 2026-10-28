use crate::{RecoveryWriter, WebProject};
use nyatidraw_api::MaskId;
use nyatidraw_document::LayerMask;
use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey};
use nyatidraw_web_core::WebDocument;

fn project(coverage: u8, enabled: bool) -> WebProject {
    let base = WebDocument::import_rgba8(2, 2, &[255; 16]).unwrap();
    let active = base.active_layer();
    let mut tree = base.layers().clone();
    tree.set_mask(
        active,
        Some(LayerMask {
            id: MaskId(7),
            enabled,
            default_coverage: 255,
        }),
    )
    .unwrap();
    let tiles = base
        .snapshot()
        .with_coverage_replacements([(
            CoverageTileKey::from_pixel(MaskId(7), 128, 0, 0),
            vec![coverage; COVERAGE_TILE_BYTE_LEN],
        )])
        .unwrap();
    WebProject::from_document(
        WebDocument::from_snapshot(base.canvas(), tiles, tree, active).unwrap(),
    )
}

#[test]
fn full_and_delta_mask_recovery_preserve_source_planes_and_reject_old_worker_packets() {
    let web = project(64, true);
    assert!(web.encode_portable().is_err());
    assert_eq!(web.export_rgba8().unwrap(), [255, 255, 255, 64].repeat(4));
    let initial = web.prepare_recovery(1, 1, None).unwrap();
    assert_eq!(&initial.bytes[..8], b"NYWORK07");
    let mut writer = RecoveryWriter::default();
    let initial_bytes = writer.stage(&initial.bytes).unwrap();
    writer.accept().unwrap();
    let reopened = WebProject::decode_ntdr(&initial_bytes).unwrap();
    assert_eq!(reopened.snapshot(), web.snapshot());
    assert_eq!(reopened.layers(), web.layers());
    assert_eq!(
        reopened.export_rgba8().unwrap(),
        [255, 255, 255, 64].repeat(4)
    );
    let edited = project(128, false);
    let delta = edited
        .prepare_recovery(1, 2, Some(&initial.checkpoint))
        .unwrap();
    let bytes = writer.stage(&delta.bytes).unwrap();
    writer.accept().unwrap();
    let restored = WebProject::decode_ntdr(&bytes).unwrap();
    assert_eq!(restored.snapshot(), edited.snapshot());
    assert_eq!(restored.layers(), edited.layers());
    assert_eq!(restored.export_rgba8().unwrap(), [255; 16]);
    assert_eq!(
        restored
            .snapshot()
            .iter()
            .map(|(key, tile)| (key, tile.hash()))
            .collect::<Vec<_>>(),
        web.snapshot()
            .iter()
            .map(|(key, tile)| (key, tile.hash()))
            .collect::<Vec<_>>()
    );
    let next = edited
        .prepare_recovery(1, 3, Some(&delta.checkpoint))
        .unwrap();
    let mut old = next.bytes.clone();
    old[..8].copy_from_slice(b"NYWORK06");
    assert!(writer.stage(&old).is_err());
    assert_eq!(writer.file().unwrap(), bytes);
    assert!(writer.stage(&next.bytes).is_ok());
}
