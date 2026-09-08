//! Scratch-only differential transaction benchmark. Not a production DB adapter.
#[path = "storage_compare/backend.rs"]
mod backend;
#[path = "storage_compare/trace.rs"]
mod trace;

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use backend::{Records, Result, Store, canonical, delta};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() > 1 {
        return child(&args);
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "nyatidraw-storage-compare-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&scratch)?;
    println!(
        "release={} redb=4.2.0 sqlite={} repetitions=3 edits=160 open_samples=32",
        !cfg!(debug_assertions),
        rusqlite::version()
    );
    for (case, noisy) in [("sprite", false), ("noise16", true)] {
        println!("capturing={case}");
        let source = scratch.join(format!("{case}-source.ntdr"));
        let trace = trace::capture(&source, noisy)?;
        let expected = scratch.join(format!("{case}-expected.bin"));
        fs::write(&expected, canonical(&trace.final_records))?;
        println!(
            "case={case} final_records={} logical_bytes={}",
            trace.final_records.len(),
            trace
                .final_records
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
        );
        for round in 0..3 {
            // Rotate order; SQLite modes are settings of one candidate, not new DBs.
            let mut order = ["redb", "sqlite-delete", "sqlite-wal"];
            order.rotate_left(round);
            for kind in order {
                let path = scratch.join(format!("{case}-{kind}-{round}.ntdr"));
                measure(&path, &expected, case, kind, round, &trace)?;
                remove_owned_store(&path)?;
            }
        }
        fs::remove_file(source)?;
        fs::remove_file(expected)?;
    }
    for kind in ["redb", "sqlite-delete", "sqlite-wal"] {
        for stage in ["before", "after"] {
            crash_probe(&scratch, kind, stage)?;
        }
    }
    // No recursive cleanup of this directory or any user artwork directory.
    fs::remove_dir(&scratch)?;
    println!(
        "PASS: exact records, child-process reopen, and commit-boundary kills; scratch removed"
    );
    Ok(())
}

fn measure(
    path: &Path,
    expected: &Path,
    case: &str,
    kind: &str,
    round: usize,
    trace: &trace::Trace,
) -> Result<()> {
    let mut store = Store::open(kind, path, true)?;
    store.apply(&delta(&Records::new(), &trace.initial), || {})?;
    drop(store);
    println!(
        "case={case} backend={kind} round={round} empty_bytes={}",
        fs::metadata(path)?.len()
    );
    let mut store = Store::open(kind, path, false)?;
    let mut commits = Vec::new();
    let mut model = trace.initial.clone();
    for (index, batch) in trace.batches.iter().enumerate() {
        let start = Instant::now();
        store.apply(batch, || {})?;
        commits.push(start.elapsed().as_secs_f64() * 1000.0);
        for (key, value) in batch {
            if let Some(value) = value {
                model.insert(key.clone(), value.clone());
            } else {
                model.remove(key);
            }
        }
        if matches!(index, 0 | 126 | 127 | 128 | 159) {
            assert_eq!(store.all()?, model, "transaction record divergence");
        }
    }
    assert_eq!(model, trace.final_records);
    println!(
        "live_main_bytes={} live_aux_bytes={}",
        fs::metadata(path)?.len(),
        auxiliary_bytes(path)?
    );
    let start = Instant::now();
    drop(store);
    println!(
        "last_write_close_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    percentiles("commit", &mut commits);
    verify_child(kind, path, expected)?;
    let mut opens = Vec::new();
    let mut loads = Vec::new();
    let mut closes = Vec::new();
    for _ in 0..32 {
        let start = Instant::now();
        let store = Store::open(kind, path, false)?;
        opens.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        assert_eq!(store.all()?, trace.final_records);
        loads.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        drop(store);
        closes.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    percentiles("warm_open", &mut opens);
    percentiles("all_records", &mut loads);
    percentiles("read_only_close", &mut closes);
    let bytes = fs::metadata(path)?.len();
    println!("closed_aux_bytes={}", auxiliary_bytes(path)?);
    let mut store = Store::open(kind, path, false)?;
    let start = Instant::now();
    store.compact()?;
    let compact_ms = start.elapsed().as_secs_f64() * 1000.0;
    drop(store);
    verify_child(kind, path, expected)?;
    println!(
        "file_bytes={bytes} compacted_bytes={} compact_ms={compact_ms:.3}",
        fs::metadata(path)?.len()
    );
    Ok(())
}

fn percentiles(label: &str, values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
    let rank = |pct: usize| values[(values.len() * pct).div_ceil(100) - 1];
    println!(
        "{label}_ms n={} p50={:.3} p95={:.3} p99={:.3}",
        values.len(),
        rank(50),
        rank(95),
        rank(99)
    );
}

fn verify_child(kind: &str, path: &Path, expected: &Path) -> Result<()> {
    let status = Command::new(std::env::current_exe()?)
        .arg("--verify")
        .arg(kind)
        .arg(path)
        .arg(expected)
        .status()?;
    if !status.success() {
        return Err("child-process exact record verification failed".into());
    }
    Ok(())
}

fn crash_records(after: bool) -> Records {
    if after {
        [
            (b"head".to_vec(), b"new".to_vec()),
            (b"new-tile".to_vec(), vec![37; 65_536]),
        ]
        .into()
    } else {
        [
            (b"head".to_vec(), b"old".to_vec()),
            (b"old-tile".to_vec(), vec![81; 65_536]),
        ]
        .into()
    }
}

fn crash_probe(scratch: &Path, kind: &str, stage: &str) -> Result<()> {
    let path = scratch.join(format!("crash-{kind}-{stage}.ntdr"));
    let expected = scratch.join(format!("crash-{kind}-{stage}.bin"));
    let mut store = Store::open(kind, &path, true)?;
    store.apply(&delta(&Records::new(), &crash_records(false)), || {})?;
    drop(store);
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--crash")
        .arg(kind)
        .arg(&path)
        .arg(stage)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = send.send(result);
    });
    let ready = receive.recv_timeout(Duration::from_secs(15));
    // Always reap this exact child, including timeout/error paths.
    let killed = child.kill();
    let status = child.wait()?;
    reader.join().map_err(|_| "child output reader panicked")?;
    let line = ready??;
    if line.trim() != "READY" || killed.is_err() || status.success() {
        return Err("crash seam not reached/killed as expected".into());
    }
    fs::write(&expected, canonical(&crash_records(stage == "after")))?;
    let start = Instant::now();
    verify_child(kind, &path, &expected)?;
    println!(
        "crash backend={kind} stage={stage} exact=PASS process_recovery_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    println!("crash_closed_aux_bytes={}", auxiliary_bytes(&path)?);
    remove_owned_store(&path)?;
    fs::remove_file(expected)?;
    Ok(())
}

fn pause_for_kill() {
    println!("READY");
    std::io::stdout().flush().expect("flush ready");
    let mut byte = [0];
    std::io::stdin()
        .read_exact(&mut byte)
        .expect("parent must terminate this child");
    panic!("crash child must not resume");
}

fn auxiliary_bytes(path: &Path) -> Result<u64> {
    let mut total = 0;
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        match fs::metadata(&name) {
            Ok(meta) => total += meta.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(total)
}

// Only after all handles/children are closed and exact recovery is verified.
// SQLite can leave a non-hot journal after a pre-commit kill; it is not artwork.
fn remove_owned_store(path: &Path) -> Result<()> {
    fs::remove_file(path)?;
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        match fs::remove_file(&name) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn child(args: &[String]) -> Result<()> {
    if args.len() != 5 {
        return Err("invalid internal child arguments".into());
    }
    let path = Path::new(&args[3]);
    // Internal modes may only address files inside a probe-owned temp directory.
    let parent = path.parent().ok_or("missing scratch directory")?;
    if parent.parent() != Some(std::env::temp_dir().as_path())
        || !parent
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("nyatidraw-storage-compare-"))
    {
        return Err("internal child path is not probe scratch".into());
    }
    let mut store = Store::open(&args[2], path, false)?;
    match args[1].as_str() {
        "--verify" => {
            let expected = fs::read(&args[4])?;
            assert_eq!(
                canonical(&store.all()?),
                expected,
                "reopened records differ"
            );
        }
        "--crash" => {
            let batch = delta(&crash_records(false), &crash_records(true));
            store.apply(&batch, || {
                if args[4] == "before" {
                    pause_for_kill();
                }
            })?;
            if args[4] == "after" {
                pause_for_kill();
            }
            return Err("unknown crash stage".into());
        }
        _ => return Err("unknown internal child command".into()),
    }
    Ok(())
}
