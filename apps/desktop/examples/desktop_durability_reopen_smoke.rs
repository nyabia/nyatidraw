#[cfg(not(windows))]
fn main() {
    eprintln!("desktop durability reopen smoke is Windows-only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows::run()
}

#[cfg(windows)]
mod windows {
    use std::{
        ffi::c_void,
        fs,
        io::{BufRead as _, BufReader},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::mpsc::{Receiver, Sender, channel},
        thread,
        time::{Duration, Instant},
    };

    use nyatidraw_document::LayerTreeNode;
    use nyatidraw_project_redb::ProjectDb;

    const PROJECT_PATH_ENV: &str = "NAYATI_PROJECT_PATH";
    const DURABILITY_PROBE_ENV: &str = "NAYATI_DESKTOP_DURABILITY_PROBE";
    const CLOSE_RAW_QUEUE_PROBE_ENV: &str = "NAYATI_CLOSE_RAW_QUEUE_PROBE";
    const PROTOCOL_PROBE_ENV: &str = "NAYATI_PROTOCOL_PROBE";
    const INPUT_SAFETY_PROBE_ENV: &str = "NAYATI_INPUT_SAFETY_PROBE";
    const WM_CLOSE: u32 = 0x0010;
    const RUN_TIMEOUT: Duration = Duration::from_secs(45);

    pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
        let desktop = desktop_binary()?;
        let scratch = scratch_directory();
        fs::create_dir(&scratch)?;
        let project = scratch.join("desktop-durability-smoke.ntdr");
        let invalid_project = scratch.join("invalid-non-empty.ntdr");
        let input_safety_project = scratch.join("input-safety.ntdr");

        let result = run_smoke(&desktop, &project, &invalid_project, &input_safety_project);
        if project.exists() {
            fs::remove_file(&project)?;
        }
        if invalid_project.exists() {
            fs::remove_file(&invalid_project)?;
        }
        if input_safety_project.exists() {
            fs::remove_file(&input_safety_project)?;
        }
        fs::remove_dir(&scratch)?;
        result
    }

    fn run_smoke(
        desktop: &Path,
        project: &Path,
        invalid_project: &Path,
        input_safety_project: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let first = run_desktop(
            desktop,
            project,
            true,
            false,
            false,
            "event=raw-input-staged-before-drain",
        )?;
        require_log(&first, "durability=redb-immediate")?;
        require_log(&first, "event=stroke-materialization-close-drain queued=")?;
        require_log(&first, "mode=drained-and-joined")?;

        let database = ProjectDb::open(project)?;
        let first_reopen = database
            .load_reopened()?
            .ok_or("first desktop run did not publish a durable snapshot")?;
        let first_snapshot = first_reopen.current_snapshot();
        let first_root = first_reopen.current_tiles().root();
        let first_tiles = first_reopen.current_tiles().len();
        if first_snapshot.0 != 32 || first_reopen.history().node_count() != 32 {
            return Err(format!(
                "close drain persisted snapshot {} with {} history nodes; expected exactly 32",
                first_snapshot.0,
                first_reopen.history().node_count(),
            )
            .into());
        }
        drop(database);

        let protocol = run_desktop(
            desktop,
            project,
            false,
            true,
            false,
            "event=protocol-proof-complete",
        )?;
        require_log(&protocol, "event=semantic-command-rejected")?;
        require_log(&protocol, "reason=StaleProjection")?;
        require_log(&protocol, "visibility=false opacity=58000")?;
        require_log(&protocol, "layers=5 active=101 solo=true")?;
        let database = ProjectDb::open(project)?;
        verify_protocol_layer_tree(&database)?;
        drop(database);

        let second = run_desktop(
            desktop,
            project,
            false,
            false,
            false,
            "event=first-native-wgpu-present",
        )?;
        require_log(&second, "event=project-open mode=durable-reopened")?;
        require_log(&second, "source=cpu-authoritative-before-first-render")?;
        require_log(&second, "mode=drained-and-joined")?;

        let database = ProjectDb::open(project)?;
        let second_reopen = database
            .load_reopened()?
            .ok_or("second desktop run lost the durable snapshot")?;
        if second_reopen.current_snapshot() != first_snapshot
            || second_reopen.current_tiles().root() != first_root
            || second_reopen.current_tiles().len() != first_tiles
        {
            return Err("desktop reopen changed the durable snapshot or CPU tiles".into());
        }
        drop(database);

        run_input_safety_smoke(desktop, input_safety_project)?;

        let invalid_bytes = b"not a Nayati project\0preserve exactly";
        fs::write(invalid_project, invalid_bytes)?;
        let invalid = run_desktop(
            desktop,
            invalid_project,
            false,
            false,
            false,
            "event=canvas-startup-failed",
        )?;
        require_log(&invalid, "invalid-non-empty-preserved=true")?;
        if invalid.contains("mode=ephemeral") {
            return Err("invalid explicit project path fell back to ephemeral mode".into());
        }
        if fs::read(invalid_project)? != invalid_bytes {
            return Err("desktop modified a non-empty invalid project file".into());
        }

        println!(
            "desktop-durability-smoke status=passed snapshot={} root={:032x} tiles={} close=exact-pid-wm-close reopen=process-restart invalid_nonempty=preserved-no-fallback",
            first_snapshot.0, first_root.id.0, first_tiles,
        );
        Ok(())
    }

    fn verify_protocol_layer_tree(database: &ProjectDb) -> Result<(), Box<dyn std::error::Error>> {
        let tree = database
            .load_layer_tree()?
            .ok_or("protocol run did not persist its layer tree")?;
        let Some(LayerTreeNode::Group(ink_group)) = tree.root().children.first() else {
            return Err("protocol reorder was not durable at root index 0".into());
        };
        if ink_group.id.0 != 10 || ink_group.visible || ink_group.opacity_u16 != 58_000 {
            return Err("protocol layer metadata did not reopen exactly".into());
        }
        if ink_group.children.len() != 3 {
            return Err("protocol layer additions were not durable inside the ink group".into());
        }
        let Some(LayerTreeNode::Raster(added_raster)) = ink_group.children.get(1) else {
            return Err("protocol raster addition did not reopen at the requested index".into());
        };
        let Some(LayerTreeNode::Group(added_group)) = ink_group.children.get(2) else {
            return Err("protocol group addition did not reopen at the requested index".into());
        };
        if added_raster.id.0 != 101
            || added_raster.name != "Character"
            || added_group.id.0 != 102
            || added_group.name != "Group 2"
        {
            return Err("protocol layer additions reopened with unexpected identity".into());
        }
        Ok(())
    }

    fn run_input_safety_smoke(
        desktop: &Path,
        project: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = run_desktop(
            desktop,
            project,
            false,
            false,
            true,
            "event=input-safety-probe-complete",
        )?;
        for marker in [
            "discontinuities=1 acknowledged=1",
            "producer_quarantined=2 consumer_quarantined=2",
            "active_cancelled=true cancelled_layer=1 recovery_layer=2",
            "clean_begin_recovered=true materialized_closed_strokes=1",
        ] {
            require_log(&output, marker)?;
        }
        let database = ProjectDb::open(project)?;
        let reopened = database
            .load_reopened()?
            .ok_or("clean Begin recovery did not durably close its stroke")?;
        if reopened.current_snapshot().0 != 1 || reopened.history().node_count() != 1 {
            return Err(
                "input discontinuity recovery materialized anything except one clean stroke".into(),
            );
        }
        Ok(())
    }

    fn run_desktop(
        desktop: &Path,
        project: &Path,
        inject_stroke: bool,
        protocol_probe: bool,
        input_safety_probe: bool,
        close_after: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut command = Command::new(desktop);
        command
            .env(PROJECT_PATH_ENV, project)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if inject_stroke {
            command
                .env(DURABILITY_PROBE_ENV, "1")
                .env(CLOSE_RAW_QUEUE_PROBE_ENV, "1");
        } else {
            command
                .env_remove(DURABILITY_PROBE_ENV)
                .env_remove(CLOSE_RAW_QUEUE_PROBE_ENV);
        }
        if protocol_probe {
            command.env(PROTOCOL_PROBE_ENV, "1");
        } else {
            command.env_remove(PROTOCOL_PROBE_ENV);
        }
        if input_safety_probe {
            command.env(INPUT_SAFETY_PROBE_ENV, "1");
        } else {
            command.env_remove(INPUT_SAFETY_PROBE_ENV);
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let (line_sender, lines) = channel();
        spawn_reader(
            child
                .stdout
                .take()
                .ok_or("desktop stdout was not captured")?,
            "stdout",
            line_sender.clone(),
        );
        spawn_reader(
            child
                .stderr
                .take()
                .ok_or("desktop stderr was not captured")?,
            "stderr",
            line_sender,
        );

        let result = monitor_and_close(&mut child, pid, close_after, &lines);
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }

    fn spawn_reader(
        stream: impl std::io::Read + Send + 'static,
        source: &'static str,
        sender: Sender<String>,
    ) {
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else {
                    break;
                };
                let _ = sender.send(format!("{source}: {line}"));
            }
        });
    }

    fn monitor_and_close(
        child: &mut Child,
        pid: u32,
        close_after: &str,
        lines: &Receiver<String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + RUN_TIMEOUT;
        let mut output = String::new();
        let mut close_posted = false;
        loop {
            while let Ok(line) = lines.try_recv() {
                println!("{line}");
                output.push_str(&line);
                output.push('\n');
                if !close_posted && line.contains(close_after) {
                    // Dioxus can publish the canvas marker while its top-level
                    // window is still completing creation. Posting WM_CLOSE in
                    // that narrow interval is accepted by Win32 but can be
                    // superseded by the remaining framework initialization.
                    thread::sleep(Duration::from_millis(500));
                    post_close_to_pid(pid)?;
                    close_posted = true;
                }
            }

            if let Some(status) = child.try_wait()? {
                while let Ok(line) = lines.recv_timeout(Duration::from_millis(25)) {
                    println!("{line}");
                    output.push_str(&line);
                    output.push('\n');
                }
                if !status.success() {
                    return Err(format!("desktop exited with {status}").into());
                }
                if !close_posted {
                    return Err(format!("desktop exited before log marker `{close_after}`").into());
                }
                return Ok(output);
            }

            if Instant::now() >= deadline {
                return Err(format!("desktop did not close within {RUN_TIMEOUT:?}").into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn require_log(output: &str, marker: &str) -> Result<(), Box<dyn std::error::Error>> {
        if output.contains(marker) {
            Ok(())
        } else {
            Err(format!("desktop output did not contain `{marker}`").into())
        }
    }

    fn desktop_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        let profile = executable
            .parent()
            .and_then(Path::parent)
            .ok_or("smoke executable has no target profile directory")?;
        let desktop = profile.join("nyatidraw-desktop.exe");
        if !desktop.is_file() {
            return Err(format!(
                "{} is missing; run `cargo build -p nyatidraw-desktop --release` first",
                desktop.display()
            )
            .into());
        }
        Ok(desktop)
    }

    fn scratch_directory() -> PathBuf {
        let sequence = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "nyatidraw-desktop-durability-{}-{sequence}",
            std::process::id()
        ))
    }

    fn post_close_to_pid(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut state = EnumState { pid, window: 0 };
            // SAFETY: `state` remains alive for the synchronous EnumWindows
            // call, and the callback treats LPARAM as this exact pointer type.
            unsafe {
                EnumWindows(
                    Some(find_pid_window),
                    (&raw mut state).cast::<c_void>() as isize,
                );
            }
            if state.window != 0 {
                // SAFETY: EnumWindows supplied this HWND for the exact child
                // PID. WM_CLOSE carries no pointer parameters.
                let posted = unsafe { PostMessageW(state.window, WM_CLOSE, 0, 0) };
                if posted != 0 {
                    return Ok(());
                }
                return Err(std::io::Error::last_os_error().into());
            }
            if Instant::now() >= deadline {
                return Err(format!("no top-level window found for exact child PID {pid}").into());
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    struct EnumState {
        pid: u32,
        window: isize,
    }

    unsafe extern "system" fn find_pid_window(window: isize, parameter: isize) -> i32 {
        let mut owner = 0_u32;
        // SAFETY: Windows supplied `window`; `owner` is a valid out pointer.
        unsafe {
            GetWindowThreadProcessId(window, &raw mut owner);
        }
        // SAFETY: Windows supplied `window` and IsWindowVisible has no further
        // pointer preconditions.
        let visible = unsafe { IsWindowVisible(window) } != 0;
        // SAFETY: post_close_to_pid passes a live EnumState pointer and
        // EnumWindows invokes callbacks synchronously.
        let state = unsafe { &mut *(parameter as *mut EnumState) };
        if owner == state.pid && visible {
            state.window = window;
            0
        } else {
            1
        }
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, isize) -> i32>,
            parameter: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
        fn IsWindowVisible(window: isize) -> i32;
        fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
    }
}
