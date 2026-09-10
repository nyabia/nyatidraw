//! Scratch CPU editing/history/PNG acceptance with a fresh process per reopen.
//! This does not exercise desktop tool gestures or GPU presentation.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_paint_cpu::{
    EditLimits, PremultipliedRgba8, SelectionPaint, SelectionSource, WandRequest,
    flatten_layer_tree_rgba8, lasso_selection, paint_selection, wand_selection,
};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{fs, path::Path, process::Command};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 129,
    height_px: 4,
    pixels_per_inch: 96,
};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--verify") {
        return verify_child(&args);
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "nyatidraw-basic-edit-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&scratch)?;
    fs::write(
        scratch.join(".nyatidraw-scratch-edit-probe"),
        b"scratch-only",
    )?;
    let result = run_cases(&scratch);
    for entry in fs::read_dir(&scratch)? {
        fs::remove_file(entry?.path())?;
    }
    fs::remove_dir(&scratch)?;
    result?;
    if args.iter().any(|arg| arg == "--measure") {
        measure_cpu()?;
    }
    Ok(())
}

fn key(layer: u128, x: i32) -> TileKey {
    TileKey {
        layer: LayerId(layer),
        mip: 0,
        x,
        y: 0,
    }
}
fn fixture_tree() -> LayerTree {
    let raster = |id, reference| {
        LayerTreeNode::Raster(LayerNode {
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: LayerId(id),
            name: format!("Layer{id}"),
            visible: true,
            locked: false,
            reference,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    };
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![
            LayerTreeNode::Group(GroupNode {
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: GroupId(10),
                name: "Reference group".into(),
                visible: true,
                opacity_u16: u16::MAX,
                children: vec![raster(2, true)],
            }),
            raster(1, false),
        ],
    })
    .expect("valid fixture tree")
}

fn expected_tiles(stage: u128) -> Result<TileSnapshot> {
    let mut first = vec![0; TILE_BYTE_LEN];
    let mut second = vec![0; TILE_BYTE_LEN];
    let mut reference = vec![0; TILE_BYTE_LEN];
    second[4..8].copy_from_slice(&[9, 9, 9, 255]); // x=129: off-page padding
    for y in 0..4_usize {
        reference[(y * 128 + 64) * 4..(y * 128 + 64) * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
        for x in 0..129_usize {
            let color = match (stage, x) {
                (2..=4, 0..=63) => [0, 255, 0, 255],
                (3, 64) => [191, 0, 64, 255],
                (3, 65) => [64, 0, 191, 255],
                (3, 66..=128) => [0, 0, 255, 255],
                (4, 65..=128) => [0, 255, 255, 255],
                _ => [0; 4],
            };
            let offset = (y * 128 + x % 128) * 4;
            let tile = if x < 128 { &mut first } else { &mut second };
            tile[offset..offset + 4].copy_from_slice(&color);
        }
    }
    TileSnapshot::from_tiles([
        (key(1, 0), first),
        (key(1, 1), second),
        (key(2, 0), reference),
        (key(1, -1), [7, 7, 7, 255].repeat(TILE_BYTE_LEN / 4)),
    ])
    .map_err(|error| format!("fixture: {error:?}").into())
}

fn commit(
    project: &Path,
    session: &mut HeadlessStrokeSession,
    id: u128,
    after: TileSnapshot,
    tree: &LayerTree,
) -> Result<()> {
    let db = ProjectDb::open(project)?;
    db.persist_canvas_spec(CANVAS)?;
    let batch = session
        .prepare_structural_change(SnapshotId(id), HistoryNodeId(id), u64::try_from(id)?, after)
        .map_err(|error| format!("prepare: {error:?}"))?;
    db.commit_structural_with_layer_tree(&batch, tree)?;
    session
        .accept_structural_change(&batch)
        .map_err(|error| format!("accept: {error:?}"))?;
    Ok(())
}

fn verify_restart(project: &Path, stage: &str, expected: u128, nodes: usize) -> Result<()> {
    let tiles = expected_tiles(expected)?;
    let flattened = flatten_layer_tree_rgba8(&tiles, &fixture_tree(), CANVAS)
        .map_err(|error| format!("expected flatten: {error:?}"))?;
    let golden = project.with_file_name(format!("expected-{stage}.png"));
    nyatidraw_png_io::encode_png(&golden, &flattened)?;
    let status = Command::new(std::env::current_exe()?)
        .arg("--verify")
        .arg(project)
        .arg(stage)
        .arg(expected.to_string())
        .arg(nodes.to_string())
        .status()?;
    if !status.success() {
        return Err(format!("reopen child failed: {stage}").into());
    }
    Ok(())
}

fn verify_child(args: &[String]) -> Result<()> {
    if args.len() != 6 {
        return Err("invalid scratch child arguments".into());
    }
    let project = Path::new(&args[2]);
    let directory = project.parent().ok_or("scratch parent")?;
    if !directory.starts_with(std::env::temp_dir())
        || !directory.join(".nyatidraw-scratch-edit-probe").is_file()
    {
        return Err("child requires marked scratch directory".into());
    }
    let stage = &args[3];
    if !stage
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid scratch stage".into());
    }
    let expected: u128 = args[4].parse()?;
    let nodes: usize = args[5].parse()?;
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing saved artwork")?;
    if reopened.current_snapshot() != SnapshotId(expected)
        || reopened.history().node_count() != nodes
        || reopened.current_tiles() != &expected_tiles(expected)?
        || db.load_layer_tree()? != Some(fixture_tree())
    {
        return Err(format!("{stage}: restored pixels/tree/cursor/branches differ").into());
    }
    let flattened = flatten_layer_tree_rgba8(reopened.current_tiles(), &fixture_tree(), CANVAS)
        .map_err(|error| format!("reopen flatten: {error:?}"))?;
    let actual = directory.join(format!("actual-{stage}.png"));
    nyatidraw_png_io::encode_png(&actual, &flattened)?;
    if fs::read(actual)? != fs::read(directory.join(format!("expected-{stage}.png")))? {
        return Err("reopened PNG differs from independent golden pixels".into());
    }
    println!(
        "basic-edit-reopen stage={stage} snapshot={expected} history_nodes={nodes} exact_tiles=true exact_png=true process_restart=true"
    );
    Ok(())
}

fn undo(project: &Path, session: &mut HeadlessStrokeSession) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let change = session
        .prepare_undo_cursor()
        .map_err(|error| format!("undo: {error:?}"))?;
    let tiles = db.load_cursor_tiles(change.target())?;
    db.persist_history_cursor(change.target())?;
    session
        .accept_history_cursor_move(change, tiles)
        .map_err(|error| format!("undo accept: {error:?}"))?;
    Ok(())
}

fn run_cases(directory: &Path) -> Result<()> {
    let project = directory.join("artwork.ntdr");
    let tree = fixture_tree();
    let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
    commit(&project, &mut session, 1, expected_tiles(1)?, &tree)?;
    verify_restart(&project, "initial", 1, 1)?;
    let request = WandRequest {
        canvas: CANVAS,
        active: LayerId(1),
        source: SelectionSource::ReferenceLayers,
        seed: [0, 0],
        tolerance: 0,
    };
    let mask = wand_selection(session.tiles(), &tree, request, EditLimits::default())
        .map_err(|error| format!("wand: {error:?}"))?;
    if mask.selected_pixels() != 256 {
        return Err("wand crossed reference barrier".into());
    }
    let painted = paint_selection(
        session.tiles(),
        &tree,
        LayerId(1),
        &mask,
        SelectionPaint::Solid(PremultipliedRgba8([0, 255, 0, 255])),
        EditLimits::default(),
    )
    .map_err(|error| format!("fill: {error:?}"))?;
    commit(&project, &mut session, 2, painted.after, &tree)?;
    verify_restart(&project, "wand-fill", 2, 2)?;
    let lasso = lasso_selection(
        CANVAS,
        &[[64, 0], [129, 0], [129, 4], [64, 4]],
        EditLimits::default(),
    )
    .map_err(|error| format!("lasso: {error:?}"))?;
    let gradient = SelectionPaint::LinearGradient {
        start: [64, 0],
        end: [66, 0],
        start_color: PremultipliedRgba8([255, 0, 0, 255]),
        end_color: PremultipliedRgba8([0, 0, 255, 255]),
    };
    let painted = paint_selection(
        session.tiles(),
        &tree,
        LayerId(1),
        &lasso,
        gradient,
        EditLimits::default(),
    )
    .map_err(|error| format!("gradient: {error:?}"))?;
    commit(&project, &mut session, 3, painted.after, &tree)?;
    verify_restart(&project, "lasso-gradient", 3, 3)?;
    undo(&project, &mut session)?;
    verify_restart(&project, "undo-gradient", 2, 3)?;
    let mask = wand_selection(
        session.tiles(),
        &tree,
        WandRequest {
            seed: [128, 0],
            ..request
        },
        EditLimits::default(),
    )
    .map_err(|error| format!("right wand: {error:?}"))?;
    let painted = paint_selection(
        session.tiles(),
        &tree,
        LayerId(1),
        &mask,
        SelectionPaint::Solid(PremultipliedRgba8([0, 255, 255, 255])),
        EditLimits::default(),
    )
    .map_err(|error| format!("branch fill: {error:?}"))?;
    commit(&project, &mut session, 4, painted.after, &tree)?;
    verify_restart(&project, "branch-fill", 4, 4)?;
    undo(&project, &mut session)?;
    let db = ProjectDb::open(&project)?;
    let redo = session
        .prepare_redo_to_cursor(HistoryNodeId(3))
        .map_err(|error| format!("branch redo: {error:?}"))?;
    db.persist_history_cursor(redo.target())?;
    drop(db);
    verify_restart(&project, "redo-gradient", 3, 4)?;
    println!(
        "basic-edit-reopen status=passed cpu_only=true desktop_gestures=false GPU_presentation=false release={}",
        !cfg!(debug_assertions)
    );
    Ok(())
}

fn measure_cpu() -> Result<()> {
    let mut times = Vec::new();
    let snapshot = TileSnapshot::empty();
    let tree = fixture_tree();
    for iteration in 0..21 {
        let start = std::time::Instant::now();
        let mask = wand_selection(
            &snapshot,
            &tree,
            WandRequest {
                canvas: CanvasSpec {
                    width_px: 3840,
                    height_px: 2160,
                    pixels_per_inch: 96,
                },
                active: LayerId(1),
                source: SelectionSource::ActiveLayer,
                seed: [0, 0],
                tolerance: 0,
            },
            EditLimits::default(),
        )
        .map_err(|error| format!("4K wand: {error:?}"))?;
        let painted = paint_selection(
            &snapshot,
            &tree,
            LayerId(1),
            &mask,
            SelectionPaint::Solid(PremultipliedRgba8([0, 255, 0, 255])),
            EditLimits::default(),
        )
        .map_err(|error| format!("4K fill: {error:?}"))?;
        std::hint::black_box(painted);
        if iteration > 0 {
            times.push(start.elapsed().as_micros());
        }
    }
    times.sort_unstable();
    println!(
        "basic-edit-cpu-measure backend=CPU release={} dimensions=3840x2160 samples=20 warmup=1 operation=active-wand-and-solid-fill p50_us={} p95_us={} p99_us={} includes_save=false includes_display=false",
        !cfg!(debug_assertions),
        times[9],
        times[18],
        times[19]
    );
    Ok(())
}
