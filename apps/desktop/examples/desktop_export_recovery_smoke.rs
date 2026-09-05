//! Scratch-only PNG pairing, process-kill and failed-Close desktop acceptance.
//! Set `NAYATI_DESKTOP_SMOKE_BINARY` to exercise the DX release bundle.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows-only desktop export recovery acceptance");
}

#[cfg(windows)]
mod windows_probe {
    use nyatidraw_api::{
        CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, LayerTreeNodeId, SnapshotId,
    };
    use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
    use nyatidraw_editor::HeadlessStrokeSession;
    use nyatidraw_project_redb::ProjectDb;
    use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
    use std::{
        fs,
        io::{BufRead as _, BufReader},
        os::windows::fs::OpenOptionsExt as _,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::mpsc::{Receiver, channel},
        thread,
        time::{Duration, Instant},
    };
    use windows::{
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            UI::WindowsAndMessaging::{
                EnumWindows, FindWindowExW, GetWindowTextW, GetWindowThreadProcessId,
                IsWindowVisible, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_APP,
                WM_CLOSE, WM_KEYDOWN, WM_NULL,
            },
        },
        core::{BOOL, w},
    };

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    const TIMEOUT: Duration = Duration::from_secs(45);

    pub(super) fn run() -> Result<()> {
        let executable = if let Some(path) = std::env::var_os("NAYATI_DESKTOP_SMOKE_BINARY") {
            PathBuf::from(path).canonicalize()?
        } else {
            std::env::current_exe()?
                .parent()
                .and_then(Path::parent)
                .ok_or("missing target profile")?
                .join("nyatidraw-desktop.exe")
        };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nyatidraw-export-recovery-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root)?;
        let result = run_cases(&executable, &root);
        // All children have joined via Desktop::drop before cleanup. Enumerate
        // only direct children of the directory created exclusively above.
        for case in fs::read_dir(&root)? {
            let case = case?.path();
            for entry in fs::read_dir(&case)? {
                fs::remove_file(entry?.path())?;
            }
            fs::remove_dir(case)?;
        }
        fs::remove_dir(root)?;
        result
    }

    fn run_cases(executable: &Path, root: &Path) -> Result<()> {
        layer_history_case(executable, &root.join("layer-history"))?;
        if std::env::args().any(|argument| argument == "--layers-only") {
            return Ok(());
        }
        history_branches_case(executable, &root.join("history-branches"))?;
        initial_import_history_case(executable, &root.join("initial-history"))?;
        for sibling in ["absent", "zero-byte"] {
            png_pair_case(executable, &root.join(sibling), sibling)?;
        }
        invalid_pair_case(executable, &root.join("invalid-pair"))?;
        for stage in ["encoded", "synced", "before-replace", "after-replace"] {
            crash_case(executable, &root.join(stage), stage)?;
        }
        superseded_case(executable, &root.join("superseded"))?;
        failed_close_case(executable, &root.join("failed-close"))?;
        println!(
            "desktop-export-recovery-smoke status=passed png_pair=absent-zero-valid-invalid activation=same-primary crash_boundaries=4 supersession=encoded-old-job-discarded failed_close=reopened-and-retried physical_pen_proof=false power_loss_proof=false"
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn layer_history_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, _) = fixture(directory)?;
        let canvas = CanvasSpec {
            width_px: 4,
            height_px: 4,
            pixels_per_inch: 96,
        };
        let group = |id, children| {
            LayerTreeNode::Group(GroupNode {
                id: GroupId(id),
                name: format!("Group{id}"),
                visible: true,
                opacity_u16: u16::MAX,
                children,
            })
        };
        let rasters = (1..=20)
            .map(|id| {
                LayerTreeNode::Raster(LayerNode {
                    id: LayerId(id),
                    name: format!("Layer{id}"),
                    visible: true,
                    locked: false,
                    reference: false,
                    opacity_u16: u16::MAX,
                    content_root: ContentRootId(0),
                })
            })
            .collect();
        let baseline = LayerTree::new(GroupNode {
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![group(10, vec![group(11, rasters)])],
        })
        .map_err(|error| format!("layer fixture: {error:?}"))?;
        let all_tiles = TileSnapshot::from_tiles((1_u8..=20).map(|id| {
            (
                TileKey {
                    layer: LayerId(u128::from(id)),
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                [id * 10, 0, 0, 255].repeat(TILE_BYTE_LEN / 4),
            )
        }))
        .map_err(|error| format!("layer pixels: {error:?}"))?;
        let db = ProjectDb::open(&project)?;
        db.persist_canvas_spec(canvas)?;
        db.persist_layer_tree(&baseline)?;
        let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, all_tiles.clone())
            .map_err(|error| format!("layer initial snapshot: {error:?}"))?;
        db.commit_structural(&batch)?;
        drop(db);
        let mut renamed = baseline.clone();
        renamed
            .rename(LayerTreeNodeId::Raster(LayerId(1)), "Renamed layer")
            .map_err(|error| format!("rename expectation: {error:?}"))?;
        let mut restored = renamed.clone();
        restored
            .set_opacity(LayerTreeNodeId::Group(GroupId(10)), 32768)
            .map_err(|error| format!("opacity expectation: {error:?}"))?;
        let mut removed = restored.clone();
        removed
            .remove(LayerTreeNodeId::Raster(LayerId(1)))
            .map_err(|error| format!("remove expectation: {error:?}"))?;
        let mut reordered = restored.clone();
        reordered
            .reorder(LayerTreeNodeId::Raster(LayerId(1)), GroupId(100), 0)
            .map_err(|error| format!("reorder expectation: {error:?}"))?;
        let mut survivor = reordered.clone();
        survivor
            .remove(LayerTreeNodeId::Group(GroupId(10)))
            .map_err(|error| format!("subtree expectation: {error:?}"))?;
        let mut reference = reordered.clone();
        reference
            .set_reference(LayerId(1), true)
            .map_err(|error| format!("reference expectation: {error:?}"))?;
        let mut inside = reference.clone();
        inside
            .reorder(LayerTreeNodeId::Raster(LayerId(1)), GroupId(11), 19)
            .map_err(|error| format!("drag inside expectation: {error:?}"))?;
        let mut below = inside.clone();
        below
            .reorder(LayerTreeNodeId::Raster(LayerId(1)), GroupId(11), 0)
            .map_err(|error| format!("drag below expectation: {error:?}"))?;
        let mut extracted = inside.clone();
        extracted
            .reorder(LayerTreeNodeId::Group(GroupId(11)), GroupId(100), 0)
            .map_err(|error| format!("drag group expectation: {error:?}"))?;
        let mut stale = inside.clone();
        stale
            .reorder(LayerTreeNodeId::Raster(LayerId(1)), GroupId(11), 18)
            .map_err(|error| format!("toolbar before stale drop expectation: {error:?}"))?;
        for (probe, snapshot, count, expected_tree) in [
            ("rename-r:1", 2, 20, Some(&renamed)),
            ("opacity-g:10", 3, 20, Some(&restored)),
            ("delete-r:1", 4, 19, Some(&removed)),
            ("undo", 3, 20, Some(&restored)),
            ("delete-g:10", 5, 0, None),
            ("undo", 3, 20, Some(&restored)),
            ("redo:5", 5, 0, None),
            ("undo", 3, 20, Some(&restored)),
            ("reorder-r:1", 6, 20, Some(&reordered)),
            ("delete-g:10", 7, 1, Some(&survivor)),
            ("undo", 6, 20, Some(&reordered)),
            ("reference-r:1", 8, 20, Some(&reference)),
            ("undo", 6, 20, Some(&reordered)),
            ("redo:8", 8, 20, Some(&reference)),
            ("ui-drag:inside", 9, 20, Some(&inside)),
            ("ui-drag:below", 10, 20, Some(&below)),
            ("ui-drag:above", 11, 20, Some(&inside)),
            ("ui-drag:group", 12, 20, Some(&extracted)),
            ("undo", 11, 20, Some(&inside)),
            ("ui-drag:cancel", 11, 20, Some(&inside)),
            ("ui-drag:stale", 13, 20, Some(&stale)),
        ] {
            let mut app = Desktop::start_with_history(executable, &project, Some(probe))?;
            if probe.starts_with("ui-drag:") {
                app.until("event=drag-probe-complete")?;
                app.until("result=passed browser_events=synthetic")?;
            } else {
                app.until("event=history-probe-complete")?;
                app.until("active_valid=true")?;
                app.until(if snapshot >= 8 {
                    "reference_count=1"
                } else {
                    "reference_count=0"
                })?;
            }
            app.until("event=first-native-wgpu-present")?;
            let window = find_window(app.child.id())?;
            let child =
                unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
            post(child, WM_KEYDOWN, usize::from(b'S'))?;
            app.until("event=png-export-finished")?;
            app.close()?;
            let db = ProjectDb::open(&project)?;
            let reopened = db.load_reopened()?.ok_or("missing layer history")?;
            let tree = db.load_layer_tree()?.ok_or("missing layer tree")?;
            if reopened.current_snapshot() != SnapshotId(snapshot)
                || reopened.current_tiles().iter().count() != count
            {
                return Err(format!("{probe}: incorrect layer snapshot or tile count").into());
            }
            if let Some(expected) = expected_tree {
                if &tree != expected {
                    return Err(format!("{probe}: layer tree changed across restart").into());
                }
            } else if tree.root().children.len() != 1
                || tree
                    .ancestors(LayerTreeNodeId::Raster(LayerId(101)))
                    .is_none()
            {
                return Err(format!(
                    "{probe}: deleting last raster did not create a drawable fallback"
                )
                .into());
            }
            let expected_tiles = all_tiles
                .with_replacements(
                    all_tiles
                        .iter()
                        .filter(|(key, _)| {
                            tree.ancestors(LayerTreeNodeId::Raster(key.layer)).is_none()
                        })
                        .map(|(key, _)| (key, vec![0; TILE_BYTE_LEN])),
                )
                .map_err(|error| format!("expected deletion tiles: {error:?}"))?;
            if reopened.current_tiles() != &expected_tiles {
                return Err(format!("{probe}: deleted or restored artwork is not exact").into());
            }
            // Reference toggles must export exactly the same pixels as the
            // preceding unmarked tree, including after Undo/restart/Redo.
            let export_tree = if snapshot == 8 { &reordered } else { &tree };
            let flattened =
                nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&expected_tiles, export_tree, canvas)
                    .map_err(|error| format!("expected layer composite: {error:?}"))?;
            verify_png(&project.with_extension("png"), &flattened)?;
        }
        println!(
            "desktop-layer-history status=passed rasters=20 nesting=2 metadata_undo=exact subtree_delete_undo_redo=exact last_layer_fallback=valid save_restart_reopen_png=exact"
        );
        Ok(())
    }

    fn fixture(directory: &Path) -> Result<(PathBuf, Vec<u8>)> {
        fs::create_dir(directory)?;
        fs::write(
            directory.join(".nyatidraw-scratch-export-probe"),
            b"scratch-only",
        )?;
        let project = directory.join("artwork.ntdr");
        let old = FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 2,
            height: 2,
            pixels: [255, 0, 0, 255].repeat(4),
        };
        nyatidraw_png_io::encode_png(&project.with_extension("png"), &old)?;
        let bytes = fs::read(project.with_extension("png"))?;
        Ok((project, bytes))
    }

    #[allow(clippy::too_many_lines)]
    fn history_branches_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, _) = fixture(directory)?;
        let db = ProjectDb::open(&project)?;
        let canvas = CanvasSpec {
            width_px: 2,
            height_px: 2,
            pixels_per_inch: 96,
        };
        db.persist_canvas_spec(canvas)?;
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let mut expected = Vec::new();
        // 66 sibling branches exercise the bounded page boundary and ensure
        // later branches remain selectable. No UI layout mocks are involved.
        for id in 1_u8..=67 {
            let tiles = TileSnapshot::from_tiles([(
                TileKey {
                    layer: LayerId(1),
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                [id, 0, 0, 255].repeat(TILE_BYTE_LEN / 4),
            )])
            .map_err(|error| format!("branch pixels: {error:?}"))?;
            let batch = session
                .prepare_structural_change(
                    SnapshotId(u128::from(id)),
                    HistoryNodeId(u128::from(id)),
                    u64::from(id),
                    tiles.clone(),
                )
                .map_err(|error| format!("branch prepare: {error:?}"))?;
            db.commit_structural(&batch)?;
            session
                .accept_structural_change(&batch)
                .map_err(|error| format!("branch accept: {error:?}"))?;
            expected.push(tiles);
            if id > 1 {
                let undo = session
                    .prepare_undo_cursor()
                    .map_err(|error| format!("branch undo: {error:?}"))?;
                let tiles = db.load_cursor_tiles(undo.target())?;
                db.persist_history_cursor(undo.target())?;
                session
                    .accept_history_cursor_move(undo, tiles)
                    .map_err(|error| format!("branch undo accept: {error:?}"))?;
            }
        }
        drop(session);
        drop(db);
        for (probe, selected, marker) in [
            ("page", 1, "more=true"),
            (
                "page:65",
                1,
                "candidates=[66, 67] after=Some(65) more=false",
            ),
            ("redo", 1, "history-prepare:History(AmbiguousRedo"),
            (
                "redo:999",
                1,
                "history-prepare:History(UnknownRedoCandidate",
            ),
            ("redo:67", 67, "event=history-probe-complete candidates=[]"),
            ("undo", 1, "more=true"),
            ("redo:2", 2, "event=history-probe-complete candidates=[]"),
        ] {
            let mut app = Desktop::start_with_history(executable, &project, Some(probe))?;
            app.until(marker)?;
            app.until("event=history-probe-complete")?;
            app.until("event=first-native-wgpu-present")?;
            let window = find_window(app.child.id())?;
            let child =
                unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
            post(child, WM_KEYDOWN, usize::from(b'S'))?;
            app.until("event=png-export-finished")?;
            app.close()?;
            let db = ProjectDb::open(&project)?;
            let reopened = db.load_reopened()?.ok_or("missing branch project")?;
            if reopened.history().node_count() != 67
                || reopened.history().head() != Some(HistoryNodeId(selected))
                || reopened.current_tiles() != &expected[usize::try_from(selected - 1)?]
            {
                return Err(
                    format!("{probe}: branch selection lost artwork, cursor or siblings").into(),
                );
            }
            verify_png(
                &project.with_extension("png"),
                &reopened
                    .current_tiles()
                    .crop_base_layer_rgba8_to_canvas(LayerId(1), canvas)
                    .map_err(|error| format!("branch export: {error:?}"))?,
            )?;
        }
        let mut restarted = Desktop::start(executable, &project, false, None)?;
        restarted.until("event=first-native-wgpu-present")?;
        restarted.close()?;
        let db = ProjectDb::open(&project)?;
        let reopened = db.load_reopened()?.ok_or("missing restarted branch")?;
        if reopened.history().head() != Some(HistoryNodeId(2))
            || reopened.history().node_count() != 67
        {
            return Err("final process restart lost selected branch".into());
        }
        println!(
            "desktop-history-branches status=passed siblings=66 page_size=64 selected=67-then-2 invalid_and_ambiguous=unchanged save_restart_reopen_export=exact"
        );
        Ok(())
    }

    fn png_pair_case(executable: &Path, directory: &Path, sibling: &str) -> Result<()> {
        let (project, source_bytes) = fixture(directory)?;
        let png = project.with_extension("png");
        if sibling == "zero-byte" {
            fs::write(&project, [])?;
        }
        let mut app = Desktop::start(executable, &png, false, None)?;
        app.until("event=png-imported")?;
        app.until("event=first-native-wgpu-present")?;
        if fs::metadata(&project)?.len() == 0 || fs::read(&png)? != source_bytes {
            return Err("PNG activation did not preserve source and bootstrap its project".into());
        }
        for activation in [&png, &project] {
            app.seen.clear();
            let mut secondary = Desktop::start(executable, activation, false, None)?;
            secondary.until("event=activation-routed target=existing-primary")?;
            secondary.success()?;
            app.until("event=activation-reused-current-project")?;
            if app.child.try_wait()?.is_some() {
                return Err("same-pair activation lost its primary".into());
            }
        }
        app.close()?;
        let (imported_tiles, imported_pixels) = durable_artwork(&project)?;
        verify_png(&png, &imported_pixels)?;
        if fs::read(&png)? != source_bytes {
            return Err("closing without Save changed the original PNG".into());
        }

        // Deliberately change the disposable PNG. A valid sibling must retain
        // its durable pixels instead of silently reimporting this blue image.
        let mut changed_png = imported_pixels.clone();
        changed_png.pixels = [0, 0, 255, 255].repeat(4);
        nyatidraw_png_io::encode_png(&png, &changed_png)?;
        let mut reopened = Desktop::start(executable, &png, false, None)?;
        reopened.until("event=project-open mode=durable-reopened")?;
        reopened.until("event=first-native-wgpu-present")?;
        verify_png(&png, &changed_png)?;
        let window = find_window(reopened.child.id())?;
        let child = unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
        post(child, WM_KEYDOWN, usize::from(b'S'))?;
        reopened.until("event=png-export-finished")?;
        reopened.close()?;
        verify_png(&png, &imported_pixels)?;
        if durable_artwork(&project)?.0 != imported_tiles {
            return Err("valid PNG sibling was reimported or Save changed its artwork".into());
        }

        // The existing semantic stroke probe draws outside this 2x2 page.
        // Its durable tiles must survive restart while PNG keeps the page crop.
        let mut edited = Desktop::start(executable, &png, true, None)?;
        edited.until("event=png-export-finished generation=2")?;
        edited.close()?;
        let (edited_tiles, edited_pixels) = durable_snapshot(&project, 2)?;
        if edited_tiles == imported_tiles || edited_pixels != imported_pixels {
            return Err("off-page stroke was lost or leaked into PNG page crop".into());
        }
        verify_png(&png, &edited_pixels)?;
        let mut restarted = Desktop::start(executable, &png, false, None)?;
        restarted.until("event=first-native-wgpu-present")?;
        restarted.close()?;
        if durable_snapshot(&project, 2)?.0 != edited_tiles {
            return Err("paired process restart lost off-page artwork".into());
        }
        println!(
            "desktop-png-pair status=passed sibling={sibling}-then-valid source_before_save=exact restart=exact activation=same-primary off_page_artwork=durable png=page-cropped physical_pen_proof=false"
        );
        Ok(())
    }

    fn initial_import_history_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, _) = fixture(directory)?;
        let png = project.with_extension("png");
        for (probe, snapshot, expected_color) in
            [("undo", 0, [0, 0, 0, 0]), ("redo:1", 1, [255, 0, 0, 255])]
        {
            let mut app = Desktop::start_with_history(executable, &png, Some(probe))?;
            app.until("event=history-probe-complete")?;
            app.until("event=first-native-wgpu-present")?;
            let window = find_window(app.child.id())?;
            let child =
                unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
            post(child, WM_KEYDOWN, usize::from(b'S'))?;
            app.until("event=png-export-finished")?;
            app.close()?;
            let db = ProjectDb::open(&project)?;
            let reopened = db
                .load_reopened()?
                .ok_or("initial import history missing")?;
            if reopened.current_snapshot() != SnapshotId(snapshot)
                || reopened.history().node_count() != 1
            {
                return Err(
                    "fresh import undo/redo did not retain its initial cursor and branch".into(),
                );
            }
            verify_png(
                &png,
                &FlattenedRgba8 {
                    origin_x: 0,
                    origin_y: 0,
                    width: 2,
                    height: 2,
                    pixels: expected_color.repeat(4),
                },
            )?;
        }
        println!(
            "desktop-initial-history status=passed fresh_import_undo=initial redo_after_restart=exact artwork_and_branch=preserved"
        );
        Ok(())
    }

    fn invalid_pair_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, source_bytes) = fixture(directory)?;
        let invalid = b"scratch non-empty invalid sibling must survive";
        fs::write(&project, invalid)?;
        let mut app = Desktop::start(executable, &project.with_extension("png"), false, None)?;
        app.until("invalid-non-empty-preserved=true")?;
        // Startup failure has no canvas close worker; Drop joins the owned PID.
        drop(app);
        if fs::read(&project)? != invalid
            || fs::read(project.with_extension("png"))? != source_bytes
        {
            return Err("invalid PNG pair was overwritten or imported as a fallback".into());
        }
        println!("desktop-png-pair status=passed sibling=invalid project_and_png=byte-exact");
        Ok(())
    }

    fn crash_case(executable: &Path, directory: &Path, stage: &str) -> Result<()> {
        let (project, old) = fixture(directory)?;
        let mut app = Desktop::start(executable, &project, true, Some(stage))?;
        app.until(&format!("event=export-probe-paused stage={stage}"))?;
        let window = find_window(app.child.id())?;
        post(window, WM_CLOSE, 0)?;
        app.until("event=close-started")?;
        app.until("event=close-dialog-mounted")?;
        responsive(window)?;
        post(window, WM_CLOSE, 0)?; // Repeated Close must not destroy a saving owner.
        responsive(window)?;
        app.child.kill()?;
        app.child.wait()?;
        let (tiles, expected) = durable_artwork(&project)?;
        let png = project.with_extension("png");
        if stage == "after-replace" {
            verify_png(&png, &expected)?;
        } else if fs::read(&png)? != old {
            return Err(format!("{stage}: incomplete export replaced the prior PNG").into());
        }
        let mut reopened = Desktop::start(executable, &project, false, None)?;
        reopened.until("event=first-native-wgpu-present")?;
        let window = find_window(reopened.child.id())?;
        post(window, WM_CLOSE, 0)?;
        reopened.until("event=close-ready")?;
        reopened.success()?;
        if durable_artwork(&project)?.0 != tiles {
            return Err("process restart changed durable artwork".into());
        }
        if stage != "after-replace" && fs::read(&png)? != old {
            return Err("reopen silently replaced source PNG".into());
        }
        println!(
            "desktop-export-crash-case status=passed stage={stage} project_snapshot=1 restart=exact previous_or_latest_png=complete ui=responsive"
        );
        Ok(())
    }

    fn failed_close_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, old) = fixture(directory)?;
        let png = project.with_extension("png");
        // Actual Windows sharing denial at replacement, not a fake worker.
        let lock = fs::OpenOptions::new().read(true).share_mode(1).open(&png)?;
        let mut app = Desktop::start(executable, &project, true, Some("synced"))?;
        app.until("event=export-probe-paused stage=synced")?;
        let window = find_window(app.child.id())?;
        post(window, WM_CLOSE, 0)?;
        app.until("event=close-started")?;
        app.until("event=close-dialog-mounted")?;
        responsive(window)?;
        fs::write(directory.join("release-export"), b"release")?;
        app.until("event=close-failed project_saved=true export_failed=true")?;
        responsive(window)?;
        if app.child.try_wait()?.is_some() || fs::read(&png)? != old {
            return Err("failed Close lost window or old PNG".into());
        }
        let (tiles, expected) = durable_artwork(&project)?;
        drop(lock);
        // Invoke the same child handler as the dialog's reopen button.
        let child = unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
        post(child, WM_APP + 0x4e4, 0)?;
        app.until("event=close-reopened authority=durable-project")?;
        // Reopening retains the accepted durable stroke; retry uses Save's
        // ordinary native keyboard/semantic path. Startup probes run only once.
        post(child, WM_KEYDOWN, usize::from(b'S'))?;
        app.until("event=png-export-finished")?;
        verify_png(&png, &expected)?;
        post(window, WM_CLOSE, 0)?;
        app.until("event=close-ready")?;
        app.success()?;
        if durable_artwork(&project)?.0 != tiles {
            return Err("retry changed project artwork".into());
        }
        println!(
            "desktop-export-failed-close status=passed project=durable old_png=preserved recovery=reopen-save window=responsive"
        );
        Ok(())
    }

    fn superseded_case(executable: &Path, directory: &Path) -> Result<()> {
        let (project, old) = fixture(directory)?;
        let mut app = Desktop::start(executable, &project, true, Some("encoded"))?;
        app.until("event=export-probe-paused stage=encoded")?;
        let window = find_window(app.child.id())?;
        let child = unsafe { FindWindowExW(Some(window), None, w!("NyatiDrawWgpuCanvas"), None) }?;
        post(child, WM_KEYDOWN, usize::from(b'S'))?;
        app.until("event=project-save-accepted export_generation=3")?;
        if fs::read(project.with_extension("png"))? != old {
            return Err("old export replaced the PNG before generation recheck".into());
        }
        fs::write(directory.join("release-export"), b"release")?;
        app.until("reason=superseded-before-replace")?;
        app.until("event=png-export-finished generation=3")?;
        post(window, WM_CLOSE, 0)?;
        app.until("event=close-ready")?;
        app.success()?;
        verify_png(
            &project.with_extension("png"),
            &durable_artwork(&project)?.1,
        )?;
        println!(
            "desktop-export-supersession status=passed old_generation=2 latest_generation=3 old_encoded_file=discarded final_pixels=exact"
        );
        Ok(())
    }

    fn durable_artwork(project: &Path) -> Result<(TileSnapshot, FlattenedRgba8)> {
        durable_snapshot(project, 1)
    }

    fn durable_snapshot(project: &Path, expected: u128) -> Result<(TileSnapshot, FlattenedRgba8)> {
        let db = ProjectDb::open(project)?;
        let reopened = db.load_reopened()?.ok_or("missing durable stroke")?;
        if reopened.current_snapshot().0 != expected
            || reopened.history().node_count() as u128 != expected
        {
            return Err("accepted final stroke was not durable exactly once".into());
        }
        let tiles = reopened.current_tiles().clone();
        let pixels = tiles
            .crop_base_layer_rgba8_to_canvas(LayerId(1), db.load_canvas_spec()?)
            .map_err(|error| format!("expected pixels: {error:?}"))?;
        Ok((tiles, pixels))
    }

    fn verify_png(path: &Path, expected: &FlattenedRgba8) -> Result<()> {
        let decoded = nyatidraw_png_io::decode_png(path, LayerId(1))?;
        let actual = decoded
            .tiles
            .crop_base_layer_rgba8_to_canvas(LayerId(1), decoded.canvas)
            .map_err(|error| format!("PNG pixels: {error:?}"))?;
        if &actual != expected {
            return Err("PNG pixels differ from final durable artwork".into());
        }
        Ok(())
    }

    struct Desktop {
        child: Child,
        lines: Receiver<String>,
        seen: Vec<String>,
    }
    impl Desktop {
        fn start(
            executable: &Path,
            project: &Path,
            seed: bool,
            pause: Option<&str>,
        ) -> Result<Self> {
            Self::start_options(executable, project, seed, pause, None)
        }
        fn start_with_history(
            executable: &Path,
            project: &Path,
            history: Option<&str>,
        ) -> Result<Self> {
            Self::start_options(executable, project, false, None, history)
        }
        fn start_options(
            executable: &Path,
            project: &Path,
            seed: bool,
            pause: Option<&str>,
            history: Option<&str>,
        ) -> Result<Self> {
            let mut command = Command::new(executable);
            for name in [
                "NAYATI_DESKTOP_DURABILITY_PROBE",
                "NAYATI_CLOSE_RAW_QUEUE_PROBE",
                "NAYATI_ACTIVE_CLOSE_PROBE",
                "NAYATI_SAVE_PROBE",
                "NAYATI_PROTOCOL_PROBE",
                "NAYATI_INPUT_SAFETY_PROBE",
                "NAYATI_EXPORT_PAUSE",
                "NAYATI_EXPORT_PROBE_DIR",
                "NAYATI_HISTORY_PROBE",
                "NAYATI_LAYER_DRAG_PROBE",
            ] {
                command.env_remove(name);
            }
            command
                .env_remove("NAYATI_PROJECT_PATH")
                .arg(project)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if seed {
                command
                    .env("NAYATI_ACTIVE_CLOSE_PROBE", "1")
                    .env("NAYATI_SAVE_PROBE", "end");
            }
            if let Some(history) = history {
                if let Some(mode) = history.strip_prefix("ui-drag:") {
                    command.env("NAYATI_LAYER_DRAG_PROBE", mode);
                } else {
                    command.env("NAYATI_HISTORY_PROBE", history);
                }
            }
            if let Some(stage) = pause {
                command.env("NAYATI_EXPORT_PAUSE", stage).env(
                    "NAYATI_EXPORT_PROBE_DIR",
                    project.parent().ok_or("missing scratch parent")?,
                );
            }
            let mut child = command.spawn()?;
            let (sender, lines) = channel();
            let streams: [Box<dyn std::io::Read + Send>; 2] = [
                Box::new(child.stdout.take().ok_or("stdout")?),
                Box::new(child.stderr.take().ok_or("stderr")?),
            ];
            for stream in streams {
                let sender = sender.clone();
                thread::spawn(move || {
                    for line in BufReader::new(stream)
                        .lines()
                        .map_while(std::result::Result::ok)
                    {
                        let _ = sender.send(line);
                    }
                });
            }
            Ok(Self {
                child,
                lines,
                seen: Vec::new(),
            })
        }
        fn close(&mut self) -> Result<()> {
            post(find_window(self.child.id())?, WM_CLOSE, 0)?;
            self.until("event=close-ready")?;
            self.success()
        }
        fn until(&mut self, marker: &str) -> Result<()> {
            if self.seen.iter().any(|line| line.contains(marker)) {
                return Ok(());
            }
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(100)) {
                    println!("{line}");
                    let found = line.contains(marker);
                    self.seen.push(line);
                    if found {
                        return Ok(());
                    }
                } else if self.child.try_wait()?.is_some() {
                    break;
                }
            }
            Err(format!("desktop did not reach {marker}").into())
        }
        fn success(&mut self) -> Result<()> {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait()? {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(format!("desktop exit: {status}").into())
                    };
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err("desktop did not exit after durable close".into())
        }
    }
    impl Drop for Desktop {
        fn drop(&mut self) {
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    fn find_window(pid: u32) -> Result<HWND> {
        let mut state = (pid, HWND::default());
        // SAFETY: EnumWindows invokes this callback synchronously with our live tuple.
        unsafe { EnumWindows(Some(find_pid), LPARAM((&raw mut state) as isize)) }?;
        if state.1.is_invalid() {
            Err("no visible window for owned child PID".into())
        } else {
            Ok(state.1)
        }
    }
    unsafe extern "system" fn find_pid(hwnd: HWND, data: LPARAM) -> BOOL {
        let state = unsafe { &mut *(data.0 as *mut (u32, HWND)) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
        let mut title = [0_u16; 128];
        let length = unsafe { GetWindowTextW(hwnd, &mut title) };
        if pid == state.0
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
            && usize::try_from(length)
                .ok()
                .is_some_and(|length| String::from_utf16_lossy(&title[..length]) == "NyatiDraw")
        {
            state.1 = hwnd;
        }
        BOOL(1)
    }
    fn post(hwnd: HWND, message: u32, value: usize) -> Result<()> {
        unsafe { PostMessageW(Some(hwnd), message, WPARAM(value), LPARAM(0)) }?;
        Ok(())
    }
    fn responsive(hwnd: HWND) -> Result<()> {
        let mut result = 0;
        if !unsafe { IsWindowVisible(hwnd) }.as_bool()
            || unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    1000,
                    Some(&raw mut result),
                )
            }
            .0 == 0
        {
            return Err("saving window is hidden or unresponsive".into());
        }
        Ok(())
    }
}
