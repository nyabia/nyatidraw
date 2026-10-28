use crate::{RecoveryWriter, WebProject};
use nyatidraw_api::{AnimationCommand as A, EditCommand};
use nyatidraw_project::encode_animation_timeline;
use nyatidraw_web_core::WebDocument;

#[test]
fn animation_worker_metadata_only_save_survives_fresh_process_without_losing_inactive_cels() {
    const REOPEN: &str = "NYATIDRAW_WEB_ANIMATION_REOPEN";
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = std::path::PathBuf::from(path);
        let bytes = std::fs::read(&path).unwrap();
        let mut web = WebProject::decode_ntdr(&bytes).unwrap();
        assert_eq!(
            web.snapshot().root().hash.0.as_slice(),
            std::fs::read(path.with_extension("root")).unwrap()
        );
        assert_eq!(
            encode_animation_timeline(web.animation_timeline()).unwrap(),
            std::fs::read(path.with_extension("timeline")).unwrap()
        );
        let timeline = web.animation_timeline().unwrap().clone();
        assert_eq!(timeline.frames()[1].duration_ms, 237);
        web.apply_animation_command(A::Select(timeline.frames()[0].id))
            .unwrap();
        assert_eq!(web.export_rgba8().unwrap(), vec![255; 16]);
        web.apply_animation_command(A::Select(timeline.frames()[1].id))
            .unwrap();
        assert_eq!(web.export_rgba8().unwrap(), vec![0; 16]);
        return;
    }
    let mut web = WebProject::from_document(WebDocument::import_rgba8(2, 2, &[255; 16]).unwrap());
    web.apply_animation_command(A::Enable).unwrap();
    let first = web.active_frame().unwrap();
    web.apply_animation_command(A::Duplicate(first)).unwrap();
    let second = web.active_frame().unwrap();
    web.apply_edit(EditCommand::ClearActiveLayer).unwrap();
    let mut writer = RecoveryWriter::default();
    let initial = web.prepare_recovery(1, 1, None).unwrap();
    let initial_bytes = writer.stage(&initial.bytes).unwrap();
    writer.accept().unwrap();
    let root = web.snapshot().root();
    web.apply_animation_command(A::SetDuration {
        frame: second,
        duration_ms: 237,
    })
    .unwrap();
    let delta = web
        .prepare_recovery(1, 2, Some(&initial.checkpoint))
        .unwrap();
    let bytes = writer.stage(&delta.bytes).unwrap();
    assert_eq!(web.snapshot().root(), root);
    assert_ne!(bytes, initial_bytes);
    writer.accept().unwrap();
    let expected = encode_animation_timeline(web.animation_timeline()).unwrap();
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-web-animation-{}-{}.ntdr",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &bytes).unwrap();
    std::fs::write(path.with_extension("root"), root.hash.0).unwrap();
    std::fs::write(path.with_extension("timeline"), expected).unwrap();
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "animation_tests::animation_worker_metadata_only_save_survives_fresh_process_without_losing_inactive_cels"])
        .env(REOPEN, &path).status().unwrap().success());
    for file in [
        path.with_extension("root"),
        path.with_extension("timeline"),
        path,
    ] {
        std::fs::remove_file(file).unwrap();
    }
    let next = web.prepare_recovery(1, 3, Some(&delta.checkpoint)).unwrap();
    let mut downgraded = next.bytes;
    downgraded[..8].copy_from_slice(b"NYWORK05");
    assert!(writer.stage(&downgraded).is_err());
    let correct = web.prepare_recovery(1, 3, Some(&delta.checkpoint)).unwrap();
    assert!(writer.stage(&correct.bytes).is_ok());
}
