//! Exact-child, scratch-only window lifecycle acceptance; no physical input proof.
//! `NAYATI_DESKTOP_SMOKE_BINARY` selects an existing release desktop bundle.
//! Evidence is retained under target/window-lifecycle-<pid>-<nonce>.
//! Pass `--require-render-worker` when accepting the render-thread implementation.
//! `--repeat-startup` imports a zero-byte paired PNG, saves one synthetic stroke,
//! and reopens it twelve times. Timeout evidence precedes exact-child cleanup.

#[cfg(not(windows))]
fn main() {
    eprintln!("desktop window lifecycle acceptance is Windows-only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_probe::run()
}

#[cfg(windows)]
mod windows_probe {
    use std::{
        fs::{self, File},
        io::{BufRead as _, BufReader, Write as _},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::mpsc::{Receiver, channel},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
    use nyatidraw_editor::HeadlessStrokeSession;
    use nyatidraw_project_redb::ProjectDb;
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
    use windows::{
        Win32::{
            Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HWND, LPARAM, RECT, WPARAM},
            System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE},
            UI::WindowsAndMessaging::{
                EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetParent,
                GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
                PostMessageW, SMTO_ABORTIFHUNG, SW_MINIMIZE, SW_RESTORE, SWP_ASYNCWINDOWPOS,
                SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SendMessageTimeoutW, SetWindowPos,
                ShowWindowAsync, WM_CLOSE, WM_NULL,
            },
        },
        core::{BOOL, w},
    };

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    const TIMEOUT: Duration = Duration::from_secs(30);
    const FIRST_PRESENT: &str = "event=first-native-wgpu-present";
    const MARKER: &str = ".nyatidraw-window-lifecycle-scratch";
    const PROJECT: &str = "lifecycle artwork.ntdr";
    const CANVAS: CanvasSpec = CanvasSpec {
        width_px: 64,
        height_px: 64,
        pixels_per_inch: 96,
    };

    pub(super) fn run() -> Result<()> {
        let args: Vec<_> = std::env::args_os().collect();
        if args.get(1).is_some_and(|arg| arg == "--verify") {
            return verify(Path::new(args.get(2).ok_or("missing scratch directory")?));
        }
        ensure_no_primary()?;
        let executable = desktop_binary()?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let output = std::env::current_dir()?
            .join("target")
            .join(format!("window-lifecycle-{}-{nonce}", std::process::id()));
        fs::create_dir(&output)?;
        println!(
            "window-lifecycle evidence={} executable={}",
            output.display(),
            executable.display()
        );
        if args.iter().any(|arg| arg == "--repeat-startup") {
            return repeated_startup(&executable, &output.join("repeated-startup"));
        }
        let resize = output.join("resize-close");
        seed(&resize)?;
        resize_case(&executable, &resize)?;
        let startup = output.join("startup-close");
        seed(&startup)?;
        startup_case(&executable, &startup)?;
        println!(
            "window-lifecycle status=passed cases=resize-minimize-restore-close,startup-close verifier=separate-process artwork=exact page-tree-history-png=exact physical_input=false first_visible_pixel=false"
        );
        Ok(())
    }

    fn desktop_binary() -> Result<PathBuf> {
        let path = if let Some(path) = std::env::var_os("NAYATI_DESKTOP_SMOKE_BINARY") {
            PathBuf::from(path)
        } else {
            std::env::current_exe()?
                .parent()
                .and_then(Path::parent)
                .ok_or("missing target profile")?
                .join("nyatidraw-desktop.exe")
        };
        Ok(path.canonicalize()?)
    }

    fn ensure_no_primary() -> Result<()> {
        // SAFETY: read-only open of the app's fixed local-session mutex. Never
        // launch a secondary which could route a scratch path into a user's app.
        match unsafe {
            OpenMutexW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                w!("Local\\NyatiDraw.SingleInstance.v1"),
            )
        } {
            Ok(handle) => {
                unsafe { CloseHandle(handle) }?;
                Err("existing NyatiDraw primary; close it before acceptance".into())
            }
            Err(error)
                if error.code() == windows::core::HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) =>
            {
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn tree() -> LayerTree {
        LayerTree::new(GroupNode {
            id: GroupId(100),
            name: "Lifecycle root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![LayerTreeNode::Raster(LayerNode {
                id: LayerId(1),
                name: "Lifecycle scratch".into(),
                visible: true,
                locked: false,
                reference: false,
                opacity_u16: u16::MAX,
                content_root: ContentRootId(0),
            })],
        })
        .expect("fixed valid fixture tree")
    }

    fn tiles() -> Result<TileSnapshot> {
        TileSnapshot::from_tiles([(-1, [180, 20, 70, 255]), (0, [20, 180, 70, 255])].map(
            |(x, pixel)| {
                (
                    TileKey {
                        layer: LayerId(1),
                        mip: 0,
                        x,
                        y: 0,
                    },
                    pixel.repeat(TILE_BYTE_LEN / 4),
                )
            },
        ))
        .map_err(|error| format!("fixture pixels: {error:?}").into())
    }

    fn seed(directory: &Path) -> Result<()> {
        fs::create_dir(directory)?; // Exclusive; never initialize an existing path.
        fs::write(directory.join(MARKER), b"window-lifecycle-v1")?;
        let project = directory.join(PROJECT);
        let db = ProjectDb::open(&project)?;
        db.persist_canvas_spec(CANVAS)?;
        let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, tiles()?)
            .map_err(|error| format!("fixture commit: {error:?}"))?;
        db.commit_structural_with_layer_tree(&batch, &tree())?;
        let flat = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&tiles()?, &tree(), CANVAS)
            .map_err(|error| format!("fixture composite: {error:?}"))?;
        nyatidraw_png_io::encode_png(&project.with_extension("png"), &flat)?;
        fs::copy(
            project.with_extension("png"),
            directory.join("expected.png"),
        )?;
        Ok(())
    }

    fn verify(directory: &Path) -> Result<()> {
        let marker = fs::read(directory.join(MARKER))?;
        if marker == b"window-lifecycle-zero-byte-v1" {
            let db = ProjectDb::open(&directory.join(PROJECT))?;
            let baseline = ProjectDb::open(&directory.join("baseline.ntdr"))?;
            let current = db.load_reopened()?.ok_or("missing reopened snapshot")?;
            let expected = baseline
                .load_reopened()?
                .ok_or("missing baseline snapshot")?;
            if current.current_snapshot() != expected.current_snapshot()
                || current.history().node_count() != expected.history().node_count()
                || current.current_tiles() != expected.current_tiles()
                || db.load_canvas_spec()? != baseline.load_canvas_spec()?
                || db.load_layer_tree()? != baseline.load_layer_tree()?
                || fs::read(directory.join(PROJECT).with_extension("png"))?
                    != fs::read(directory.join("expected.png"))?
            {
                return Err(
                    "repeated startup changed durable artwork/history/page/tree/PNG".into(),
                );
            }
            for cursor in [None, Some(HistoryNodeId(1)), Some(HistoryNodeId(2))] {
                if db.load_cursor_tiles(
                    current
                        .cursor_for_head(cursor)
                        .ok_or("missing history cursor")?,
                )? != baseline.load_cursor_tiles(
                    expected
                        .cursor_for_head(cursor)
                        .ok_or("missing baseline cursor")?,
                )? {
                    return Err("repeated startup changed history artwork".into());
                }
            }
            println!(
                "window-lifecycle-verify status=passed fixture=zero-byte history=exact png=exact"
            );
            return Ok(());
        }
        if marker != b"window-lifecycle-v1" {
            return Err("not a lifecycle scratch fixture".into());
        }
        let project = directory.join(PROJECT);
        if !project.is_file() {
            return Err("scratch artwork disappeared".into());
        }
        let db = ProjectDb::open(&project)?;
        let reopened = db.load_reopened()?.ok_or("missing durable snapshot")?;
        if reopened.current_snapshot() != SnapshotId(1)
            || reopened.history().node_count() != 1
            || reopened.current_tiles() != &tiles()?
            || db.load_canvas_spec()? != CANVAS
            || db.load_layer_tree()? != Some(tree())
            || fs::read(project.with_extension("png"))? != fs::read(directory.join("expected.png"))?
        {
            return Err("lifecycle changed artwork/page/tree/history/PNG".into());
        }
        println!(
            "window-lifecycle-verify status=passed tiles=2 signed_offpage=true history=1 png=exact directory={}",
            directory.display()
        );
        Ok(())
    }

    fn separate_verify(directory: &Path) -> Result<()> {
        let status = Command::new(std::env::current_exe()?)
            .arg("--verify")
            .arg(directory)
            .status()?;
        if !status.success() {
            return Err(format!("verifier failed: {status}").into());
        }
        Ok(())
    }

    fn repeated_startup(executable: &Path, directory: &Path) -> Result<()> {
        fs::create_dir(directory)?;
        fs::write(directory.join(MARKER), b"window-lifecycle-zero-byte-v1")?;
        fs::write(directory.join(PROJECT), [])?;
        let flat = nyatidraw_tiles::FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 2,
            height: 2,
            pixels: [255, 0, 0, 255].repeat(4),
        };
        nyatidraw_png_io::encode_png(&directory.join(PROJECT).with_extension("png"), &flat)?;
        let mut seed = Desktop::start(executable, directory, "zero-byte-seed")?;
        seed.until("event=png-export-finished generation=2")?;
        seed.post_close(seed.window()?)?;
        seed.success()?;
        fs::copy(directory.join(PROJECT), directory.join("baseline.ntdr"))?;
        fs::copy(
            directory.join(PROJECT).with_extension("png"),
            directory.join("expected.png"),
        )?;
        separate_verify(directory)?;
        for iteration in 0..12 {
            restart(executable, directory, &format!("repeat-{iteration:02}"))?;
        }
        println!("window-lifecycle repeated-startup=passed iterations=12 physical_input=false");
        Ok(())
    }

    fn resize_case(executable: &Path, directory: &Path) -> Result<()> {
        let mut app = Desktop::start(executable, directory, "cycles")?;
        app.until(FIRST_PRESENT)?;
        let window = app.window()?;
        for (index, (width, height)) in [(1000, 700), (1400, 900), (900, 640), (1280, 800)]
            .into_iter()
            .enumerate()
        {
            unsafe {
                let _ = ShowWindowAsync(window, SW_RESTORE);
            }
            wait_iconic(&mut app, window, false)?;
            resize(window, width, height)?;
            thread::sleep(Duration::from_millis(100));
            responsive(window)?;
            let mut rect = RECT::default();
            unsafe { GetClientRect(window, &raw mut rect) }?;
            if rect.right <= 0 || rect.bottom <= 0 {
                return Err("restored client is empty".into());
            }
            unsafe {
                let _ = ShowWindowAsync(window, SW_MINIMIZE);
            }
            wait_iconic(&mut app, window, true)?;
            responsive(window)?;
            unsafe {
                let _ = ShowWindowAsync(window, SW_RESTORE);
            }
            wait_iconic(&mut app, window, false)?;
            responsive(window)?;
            println!(
                "window-lifecycle cycle={index} pid={} client={}x{} minimize_restore=acknowledged",
                app.child.id(),
                rect.right,
                rect.bottom
            );
        }
        // Close immediately after an asynchronous resize, without waiting for
        // another presentation, to exercise pending geometry retirement.
        resize(window, 1100, 720)?;
        app.post_close(window)?;
        app.success()?;
        separate_verify(directory)?;
        restart(executable, directory, "cycles-reopen")
    }

    fn startup_case(executable: &Path, directory: &Path) -> Result<()> {
        let mut app = Desktop::start(executable, directory, "startup")?;
        let deadline = Instant::now() + TIMEOUT;
        let window = loop {
            app.drain()?;
            if app.contains(FIRST_PRESENT) {
                return Err(
                    "startup Close coverage missed: first-present preceded HWND discovery".into(),
                );
            }
            if let Some(window) = find_window(app.child.id()) {
                break window;
            }
            app.ensure_alive()?;
            if Instant::now() >= deadline {
                return Err("startup HWND discovery timed out".into());
            }
            thread::sleep(Duration::from_millis(2));
        };
        app.post_close(window)?;
        println!(
            "window-lifecycle startup_close=requested-before-observed-first-present pid={} message_processing_order=not-inferred",
            app.child.id()
        );
        app.success()?;
        println!(
            "window-lifecycle startup_close=normal-exit first_present_observed={} no_present_before_exit_proof={}",
            app.contains(FIRST_PRESENT),
            !app.contains(FIRST_PRESENT)
        );
        separate_verify(directory)?;
        restart(executable, directory, "startup-reopen")
    }

    fn restart(executable: &Path, directory: &Path, label: &str) -> Result<()> {
        let mut app = Desktop::start(executable, directory, label)?;
        app.until(FIRST_PRESENT)?;
        if !app.contains("event=project-open mode=durable-reopened") {
            return Err("restart did not report durable reopen".into());
        }
        app.post_close(app.window()?)?;
        app.success()?;
        separate_verify(directory)
    }

    fn resize(window: HWND, width: i32, height: i32) -> Result<()> {
        // SAFETY: only an HWND enumerated for the exact owned child is used.
        unsafe {
            SetWindowPos(
                window,
                None,
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        }?;
        Ok(())
    }

    fn responsive(window: HWND) -> Result<()> {
        let mut result = 0;
        let start = Instant::now();
        // WM_NULL has no pointer payload; the timeout bounds an unresponsive UI.
        if unsafe {
            SendMessageTimeoutW(
                window,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                2000,
                Some(&raw mut result),
            )
        }
        .0 == 0
        {
            return Err("owned window failed bounded WM_NULL response".into());
        }
        println!(
            "window-lifecycle wm_null_ack_us={} measurement=message-response-not-input-latency",
            start.elapsed().as_micros()
        );
        Ok(())
    }

    fn wait_iconic(app: &mut Desktop, window: HWND, expected: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.drain()?;
            app.ensure_alive()?;
            if unsafe { IsIconic(window) }.as_bool() == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("minimize/restore state timed out".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    struct Desktop {
        child: Child,
        lines: Receiver<Option<String>>,
        seen: Vec<String>,
        log: File,
        close_posted: bool,
        readers_finished: usize,
    }

    impl Desktop {
        fn start(executable: &Path, directory: &Path, label: &str) -> Result<Self> {
            ensure_no_primary()?;
            let mut command = Command::new(executable);
            for (name, _) in std::env::vars_os() {
                if name.to_string_lossy().starts_with("NAYATI_") {
                    command.env_remove(name);
                }
            }
            command
                .env("NAYATI_LAYOUT_PATH", directory.join("layout.state"))
                .env("WEBVIEW2_USER_DATA_FOLDER", directory.join("webview"))
                .arg(if label == "zero-byte-seed" {
                    directory.join(PROJECT).with_extension("png")
                } else {
                    directory.join(PROJECT)
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if label == "zero-byte-seed" {
                command
                    .env("NAYATI_ACTIVE_CLOSE_PROBE", "1")
                    .env("NAYATI_SAVE_PROBE", "end");
            }
            let log = File::create(directory.join(format!("{label}.log")))?;
            let mut child = command.spawn()?;
            let (sender, lines) = channel();
            for stream in [
                Box::new(child.stdout.take().ok_or("stdout capture")?)
                    as Box<dyn std::io::Read + Send>,
                Box::new(child.stderr.take().ok_or("stderr capture")?)
                    as Box<dyn std::io::Read + Send>,
            ] {
                let sender = sender.clone();
                thread::spawn(move || {
                    for line in BufReader::new(stream)
                        .lines()
                        .map_while(std::result::Result::ok)
                    {
                        if sender.send(Some(line)).is_err() {
                            break;
                        }
                    }
                    let _ = sender.send(None);
                });
            }
            println!("window-lifecycle launched label={label} pid={}", child.id());
            Ok(Self {
                child,
                lines,
                seen: Vec::new(),
                log,
                close_posted: false,
                readers_finished: 0,
            })
        }

        fn contains(&self, marker: &str) -> bool {
            self.seen.iter().any(|line| line.contains(marker))
        }

        fn drain(&mut self) -> Result<()> {
            while let Ok(item) = self.lines.try_recv() {
                let Some(line) = item else {
                    self.readers_finished += 1;
                    continue;
                };
                writeln!(self.log, "{line}")?;
                println!("{line}");
                let failed = [
                    "event=render-failed",
                    "event=canvas-startup-failed",
                    "event=canvas-host-failed",
                    "event=close-failed",
                    "panicked at",
                ]
                .iter()
                .any(|marker| line.contains(marker));
                self.seen.push(line.clone());
                if failed {
                    return Err(format!("desktop failure: {line}").into());
                }
            }
            self.log.flush()?;
            Ok(())
        }

        fn ensure_alive(&mut self) -> Result<()> {
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("child exited early: {status}").into());
            }
            Ok(())
        }

        fn until(&mut self, marker: &str) -> Result<()> {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                self.drain()?;
                if self.contains(marker) {
                    return Ok(());
                }
                self.ensure_alive()?;
                if Instant::now() >= deadline {
                    self.diagnose_windows()?;
                    return Err(format!("timeout awaiting {marker}").into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn window(&self) -> Result<HWND> {
            find_window(self.child.id()).ok_or_else(|| "owned top-level HWND missing".into())
        }

        fn diagnose_windows(&mut self) -> Result<()> {
            let mut state = (self.child.id(), Vec::<HWND>::new());
            // Synchronous enumeration retains only exact-child top-level windows.
            let _ = unsafe { EnumWindows(Some(collect_top), LPARAM((&raw mut state) as isize)) };
            let tops = state.1.clone();
            for window in tops {
                let _ = unsafe {
                    EnumChildWindows(
                        Some(window),
                        Some(collect_child),
                        LPARAM((&raw mut state.1) as isize),
                    )
                };
            }
            for window in state.1.into_iter().take(64) {
                let mut pid = 0;
                let tid = unsafe { GetWindowThreadProcessId(window, Some(&raw mut pid)) };
                let mut class = [0_u16; 256];
                let length = unsafe { GetClassNameW(window, &mut class) };
                let class =
                    String::from_utf16_lossy(&class[..usize::try_from(length).unwrap_or(0)]);
                let mut rect = RECT::default();
                let mut client = RECT::default();
                let window_rect = unsafe { GetWindowRect(window, &raw mut rect) };
                let client_rect = unsafe { GetClientRect(window, &raw mut client) };
                let parent = unsafe { GetParent(window) };
                let visible = unsafe { IsWindowVisible(window) }.as_bool();
                let iconic = unsafe { IsIconic(window) }.as_bool();
                let mut result = 0;
                let ack = unsafe {
                    SendMessageTimeoutW(
                        window,
                        WM_NULL,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG,
                        250,
                        Some(&raw mut result),
                    )
                }
                .0 != 0;
                let line = format!(
                    "harness event=timeout-window hwnd={window:?} pid={pid} tid={tid} parent={parent:?} class={class:?} visible={visible} iconic={iconic} window={rect:?} client={client:?} rect_ok={} client_ok={} wm_null_ack={ack}",
                    window_rect.is_ok(),
                    client_rect.is_ok()
                );
                println!("{line}");
                writeln!(self.log, "{line}")?;
            }
            self.log.flush()?;
            Ok(())
        }

        fn post_close(&mut self, window: HWND) -> Result<()> {
            unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }?;
            self.close_posted = true;
            writeln!(
                self.log,
                "harness event=wm-close-posted pid={}",
                self.child.id()
            )?;
            Ok(())
        }

        fn verify_retirement(&self) -> Result<()> {
            let worker = self.contains("desktop-render event=worker-started");
            if std::env::args().any(|arg| arg == "--require-render-worker")
                && self.contains(FIRST_PRESENT)
                && !worker
            {
                return Err("presented without required render-worker evidence".into());
            }
            if worker {
                let retired = self
                    .seen
                    .iter()
                    .position(|line| line.contains("desktop-render event=surface-retired"));
                let destroyed = self.seen.iter().position(|line| {
                    line.contains("desktop-render event=canvas-hwnd-destroyed surface-retired=true")
                });
                if !matches!((retired, destroyed), (Some(a), Some(b)) if a < b) {
                    return Err(
                        "worker retirement before HWND destruction was not established".into(),
                    );
                }
                println!(
                    "window-lifecycle retirement=surface-before-hwnd observed_stdout_order=true"
                );
            }
            Ok(())
        }

        fn success(&mut self) -> Result<()> {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                self.drain()?;
                if let Some(status) = self.child.try_wait()? {
                    // Do not infer a missing first-present from delayed pipe
                    // output. Both readers must report EOF within a bound.
                    let pipe_deadline = Instant::now() + Duration::from_secs(2);
                    while self.readers_finished != 2 {
                        self.drain()?;
                        if Instant::now() >= pipe_deadline {
                            return Err("child exited but captured output is incomplete".into());
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    if !self.close_posted || !status.success() {
                        return Err(format!("not a normal requested Close: {status}").into());
                    }
                    self.verify_retirement()?;
                    println!(
                        "window-lifecycle exited pid={} status={status} cleanup=normal-wm-close",
                        self.child.id()
                    );
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err("normal Close timed out; exact-child abort follows".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Desktop {
        fn drop(&mut self) {
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                eprintln!(
                    "window-lifecycle cleanup=exact-child-abort-not-graceful pid={}",
                    self.child.id()
                );
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    fn find_window(pid: u32) -> Option<HWND> {
        let mut state = (pid, HWND::default());
        // Callback and its tuple live entirely inside this synchronous call.
        let _ = unsafe { EnumWindows(Some(find_pid), LPARAM((&raw mut state) as isize)) };
        (!state.1.is_invalid()).then_some(state.1)
    }

    unsafe extern "system" fn collect_top(hwnd: HWND, parameter: LPARAM) -> BOOL {
        let state = unsafe { &mut *(parameter.0 as *mut (u32, Vec<HWND>)) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
        if pid == state.0 {
            state.1.push(hwnd);
        }
        BOOL(1)
    }

    unsafe extern "system" fn collect_child(hwnd: HWND, parameter: LPARAM) -> BOOL {
        let windows = unsafe { &mut *(parameter.0 as *mut Vec<HWND>) };
        if windows.len() >= 64 {
            return BOOL(0);
        }
        windows.push(hwnd);
        BOOL(1)
    }

    unsafe extern "system" fn find_pid(hwnd: HWND, parameter: LPARAM) -> BOOL {
        let state = unsafe { &mut *(parameter.0 as *mut (u32, HWND)) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
        if pid != state.0 {
            return BOOL(1);
        }
        let mut title = [0_u16; 128];
        let length = unsafe { GetWindowTextW(hwnd, &mut title) };
        if usize::try_from(length)
            .ok()
            .is_some_and(|n| String::from_utf16_lossy(&title[..n]) == "NyatiDraw")
        {
            state.1 = hwnd;
            return BOOL(0);
        }
        BOOL(1)
    }
}
