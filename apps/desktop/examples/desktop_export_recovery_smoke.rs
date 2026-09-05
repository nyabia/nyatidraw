//! Scratch-only process-kill and failed-Close acceptance of the real desktop.
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
    use nyatidraw_api::LayerId;
    use nyatidraw_project_redb::ProjectDb;
    use nyatidraw_tiles::{FlattenedRgba8, TileSnapshot};
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
        for stage in ["encoded", "synced", "before-replace", "after-replace"] {
            crash_case(executable, &root.join(stage), stage)?;
        }
        superseded_case(executable, &root.join("superseded"))?;
        failed_close_case(executable, &root.join("failed-close"))?;
        println!(
            "desktop-export-recovery-smoke status=passed crash_boundaries=4 supersession=encoded-old-job-discarded failed_close=reopened-and-retried physical_pen_proof=false power_loss_proof=false"
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
        let db = ProjectDb::open(project)?;
        let reopened = db.load_reopened()?.ok_or("missing durable stroke")?;
        if reopened.current_snapshot().0 != 1 || reopened.history().node_count() != 1 {
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
            ] {
                command.env_remove(name);
            }
            command
                .env("NAYATI_PROJECT_PATH", project)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if seed {
                command
                    .env("NAYATI_ACTIVE_CLOSE_PROBE", "1")
                    .env("NAYATI_SAVE_PROBE", "end");
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
