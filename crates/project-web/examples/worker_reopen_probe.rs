use nyatidraw_api::CanvasSpec;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_project_web::{RecoveryWriter, WebProject};
use nyatidraw_web_core::{StrokePoint, WebTool};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args
        .get(1)
        .is_some_and(|argument| argument == "--verify-file")
    {
        let db = ProjectDb::open(std::path::Path::new(&args[2]))?;
        let native = db
            .load_reopened()?
            .ok_or("missing native head")?
            .into_parts();
        assert!(
            !native.current_tiles.is_empty(),
            "download has no artwork tiles"
        );
        drop(db);
        let web = WebProject::decode_ntdr(&std::fs::read(&args[2])?)?;
        assert_eq!(*web.snapshot(), native.current_tiles);
        println!(
            "downloaded file: native and web artwork roots match {:?}",
            native.current_tiles.root().hash
        );
        return Ok(());
    }
    if args.get(1).is_some_and(|argument| argument == "--verify") {
        let db = ProjectDb::open(std::path::Path::new(&args[2]))?;
        let native = db
            .load_reopened()?
            .ok_or("missing native head")?
            .into_parts();
        assert_eq!(format!("{:?}", native.current_tiles.root().hash), args[3]);
        drop(db);
        let bytes = std::fs::read(&args[2])?;
        let web = WebProject::decode_ntdr(&bytes)?;
        assert_eq!(*web.snapshot(), native.current_tiles);
        println!("fresh process: native and web artwork roots match");
        return Ok(());
    }
    let mut web = WebProject::new(CanvasSpec {
        width_px: 64,
        height_px: 64,
        pixels_per_inch: 96,
    })?;
    let mut writer = RecoveryWriter::default();
    let mut checkpoint = None;
    let scratch = std::env::temp_dir().join(format!(
        "nyatidraw-worker-reopen-{}-{}.ntdr",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    for (revision, tool) in [(1, WebTool::Pencil2H), (2, WebTool::Pen)] {
        web.select_tool(tool)
            .map_err(|error| format!("{error:?}"))?;
        let first = StrokePoint {
            x: 21.0,
            y: 12.0,
            pressure: 0.0,
            time_ms: 1.0,
        };
        web.begin_stroke(first)
            .map_err(|error| format!("{error:?}"))?;
        web.end_stroke(StrokePoint {
            x: 22.0,
            pressure: 0.5,
            time_ms: 2.0,
            ..first
        })
        .map_err(|error| format!("{error:?}"))?;
        assert!(!web.snapshot().is_empty());
        let expected = format!("{:?}", web.snapshot().root().hash);
        let request = web.prepare_recovery(1, revision, checkpoint.as_ref())?;
        let bytes = writer.stage(&request.bytes)?;
        if revision == 1 {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&scratch)?;
        }
        std::fs::write(&scratch, &bytes)?;
        writer.accept()?;
        checkpoint = Some(request.checkpoint);
        let status = std::process::Command::new(std::env::current_exe()?)
            .arg("--verify")
            .arg(&scratch)
            .arg(expected)
            .status()?;
        if !status.success() {
            return Err("fresh process verification failed; scratch retained".into());
        }
    }
    std::fs::remove_file(&scratch)?;
    println!("Worker full snapshot and delta: save/restart/reopen passed");
    Ok(())
}
