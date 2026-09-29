//! Loads every FBX file from the ufbx test corpus through the **real** loader
//! (`AssetServer` + `LoadContext` + every processor, exactly the runtime path)
//! and classifies each file's result.
//!
//! Corpus location: `UFBX_TEST_DATA`, else `<crate>/../../libs/ufbx/data` when it
//! exists (skipped silently otherwise). A set-but-invalid `UFBX_TEST_DATA` fails
//! the run: a mistyped corpus path must not degrade into a silent skip.
//!
//! The test fails on:
//! - a load error for a file that is not in [`EXPECTED_LOAD_FAILURES`],
//! - a per-file wall-clock timeout (a hung load must not pass),
//! - a panic escaping the loader (attributed to the file that triggered it).
//!
//! Entries in [`EXPECTED_LOAD_FAILURES`] are malformed/unsupported corpus inputs
//! recorded from an actual run (see `.swarm/results/L5-verification.md`), each with
//! its reason. Never add an entry to hide a loader regression. Files in
//! [`HOSTILE_FILES`] kill the whole process at the C level (`abort()`), so they are
//! not loaded in-process: each is exercised in an isolated subprocess
//! ([`hostile_corpus_files_are_isolated_in_subprocesses`]). The probe child reports
//! *how* the load ended through explicit exit codes (`0` clean load, `42` graceful
//! parser rejection, `43` timeout) plus a structured stdout line, so the parent can
//! tell a recorded native crash apart from a Rust test panic (101), a hang, or a
//! stale exclusion — see [`classify_probe_termination`].
//!
//! Because such a native abort can exist in any not-yet-seen fixture, the sweep also
//! supports deterministic sharding: `UFBX_CORPUS_START` (index, inclusive) and
//! `UFBX_CORPUS_LIMIT` (max files). Every file prints `TRY <index> <path>` *before*
//! loading, so a crash always names the next [`HOSTILE_FILES`] candidate, a crash in
//! one shard only costs that shard, and shards are aggregated from the
//! machine-readable `L5-SHARD-RESULT` line each run prints.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPlugin, AssetServer, LoadState};
use bevy::image::Image;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxPlugin};

/// Subdirectories that contain binary fuzz/garbage data, not valid FBX files.
const SKIP_DIRS: &[&str] = &["fuzz", "cache_fuzz", "obj_fuzz", "mtl_fuzz"];

/// Per-file wall-clock budget for the full loader pass; exceeding it fails the run.
/// Override with `UFBX_CORPUS_TIMEOUT_S` to characterise a suspected slow file.
const DEFAULT_PER_FILE_TIMEOUT: Duration = Duration::from_secs(15);
/// Hard hang guard: a single file blocking longer than this aborts the process so a
/// hung loader can never be mistaken for a pass.
const WATCHDOG_ABORT_AFTER: Duration = Duration::from_secs(45);

/// Corpus files that abort the native parser (`abort()`, whole-process kill), so they
/// cannot be loaded in the in-process sweep. Each is exercised in an isolated
/// subprocess instead — see [`hostile_corpus_files_are_isolated_in_subprocesses`].
const HOSTILE_FILES: &[(&str, &str)] = &[
    (
        "synthetic_parent_directory_bad_7700_ascii.fbx",
        concat!(
            "ufbx native abort on malformed synthetic-parent path (reported by L2); ",
            "kills the process instead of returning a load error",
        ),
    ),
    (
        "synthetic_nurbs_dimension_overflow_fail_7500_ascii.fbx",
        concat!(
            "ufbx native access violation (0xc0000005) on NURBS dimension overflow; ",
            "observed in the 2026-09-29 sharded sweep (index 654); kills the process ",
            "instead of returning a load error",
        ),
    ),
];

/// Corpus files that are *expected* to fail the full loader pass, with the reason
/// observed in a real run. Keep this list explicit and justified per file.
const EXPECTED_LOAD_FAILURES: &[(&str, &str)] = &[
    (
        "synthetic_bad_inf_nan_fail_7500_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_bad_inf_nan_fail_7700_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_legacy_unquoted_child_fail_5800_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_node_cycle_fail_7700_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Cyclic node hierarchy'",
    ),
    (
        "synthetic_node_depth_fail_7400_binary.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_node_depth_fail_7500_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_truncated_compressed_fail_7400_binary.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Truncated file'",
    ),
    (
        "synthetic_truncated_quot_fail_7500_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Failed to load'",
    ),
    (
        "synthetic_unsupported_cube_fail_2000_binary.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Unsupported version (2000)'",
    ),
    (
        "synthetic_unsupported_cube_fail_8000_ascii.fbx",
        "intentional malformed fixture (*_fail_*): ufbx graceful error 'Unsupported version (8000)'",
    ),
];

#[derive(Debug)]
enum Outcome {
    Loaded(Duration),
    Failed { error: String, elapsed: Duration },
    TimedOut(Duration),
}

/// Exit code used by [`isolated_hostile_probe`] when the load completes cleanly.
const PROBE_EXIT_LOADED: i32 = 0;
/// Exit code when the parser rejects the malformed file gracefully (safely on this OS).
const PROBE_EXIT_SAFE_REJECTION: i32 = 42;
/// Exit code when the loader stops making progress: a hang can never pass.
const PROBE_EXIT_TIMEOUT: i32 = 43;

/// Windows exception codes delivered as the raw process exit code
/// (`ExitStatus::code()` reinterprets them as `i32`).
#[cfg(windows)]
const WINDOWS_ACCESS_VIOLATION: u32 = 0xC000_0005;
#[cfg(windows)]
const WINDOWS_FATAL_ERROR: u32 = 0xC000_0409;

/// POSIX signal numbers for native termination. Documented values, so no `libc`
/// dependency is needed.
#[cfg(unix)]
const SIGABRT: i32 = 6;
#[cfg(unix)]
const SIGSEGV: i32 = 11;

/// How an isolated hostile-fixture probe terminated. Only explicit, recognized
/// outcomes count as evidence; anything else fails the parent.
#[derive(Debug, PartialEq, Eq)]
enum ProbeTermination {
    /// The load completed cleanly: the hostile exclusion is stale.
    Loaded,
    /// The parser rejected the malformed input gracefully (safe on this OS).
    SafeRejection,
    /// The loader was still running when the budget expired.
    TimedOut,
    /// Recognized native (process-killing) termination.
    NativeCrash(String),
    /// Neither of the above — e.g. a Rust test panic (exit 101) or an
    /// unrecognized status. Never evidence of anything.
    Unrecognized(String),
}

fn classify_probe_termination(status: &std::process::ExitStatus) -> ProbeTermination {
    if let Some(code) = status.code() {
        return match code {
            PROBE_EXIT_LOADED => ProbeTermination::Loaded,
            PROBE_EXIT_SAFE_REJECTION => ProbeTermination::SafeRejection,
            PROBE_EXIT_TIMEOUT => ProbeTermination::TimedOut,
            other => {
                #[cfg(windows)]
                {
                    let raw = other as u32;
                    if raw == WINDOWS_ACCESS_VIOLATION {
                        return ProbeTermination::NativeCrash(format!(
                            "access violation 0x{raw:08X}"
                        ));
                    }
                    if raw == WINDOWS_FATAL_ERROR {
                        return ProbeTermination::NativeCrash(format!("fatal error 0x{raw:08X}"));
                    }
                }
                ProbeTermination::Unrecognized(format!("exit code {other}"))
            }
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return match signal {
                SIGABRT | SIGSEGV => ProbeTermination::NativeCrash(format!("signal {signal}")),
                other => ProbeTermination::Unrecognized(format!("signal {other}")),
            };
        }
    }
    ProbeTermination::Unrecognized(format!("{status}"))
}

fn corpus_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("UFBX_TEST_DATA") {
        let dir = PathBuf::from(dir);
        assert!(
            dir.is_dir(),
            "UFBX_TEST_DATA is set to '{}' but that is not a directory; refusing to \
             fall back silently to the default corpus location",
            dir.display()
        );
        return Some(dir);
    }
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/ufbx/data");
    default.is_dir().then_some(default)
}

fn collect_fbx_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_recursive(root, &mut files);
    files.sort();
    files
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !SKIP_DIRS.contains(&name) {
                collect_recursive(&path, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("fbx"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

/// Asset-server-relative path with `/` separators (valid on every platform).
fn relative_asset_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Per-file timeout, overridable via `UFBX_CORPUS_TIMEOUT_S`.
fn per_file_timeout() -> Duration {
    std::env::var("UFBX_CORPUS_TIMEOUT_S")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PER_FILE_TIMEOUT)
}

/// Sharding window for crash-isolated sweeps: `(start index, max files)`.
fn shard_config() -> (usize, Option<usize>) {
    let start = std::env::var("UFBX_CORPUS_START")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let limit = std::env::var("UFBX_CORPUS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok());
    (start, limit)
}

fn corpus_app(asset_root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            // `AssetPlugin::file_path` is a `String` in bevy_asset 0.19.
            file_path: asset_root.to_string_lossy().to_string(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .init_asset::<AnimationClip>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .init_asset::<WorldAsset>();
    app
}

/// Run one full loader pass (root `Fbx` asset => materials, meshes, nodes/skins,
/// animation baking and scene build) and return its classified result.
fn load_one(app: &mut App, rel_path: &str) -> Outcome {
    let handle: Handle<Fbx> = app
        .world()
        .resource::<AssetServer>()
        .load(rel_path.to_string());
    let start = Instant::now();
    loop {
        app.update();
        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => return Outcome::Loaded(start.elapsed()),
            LoadState::Failed(err) => {
                return Outcome::Failed {
                    error: err.to_string(),
                    elapsed: start.elapsed(),
                };
            }
            _ => {}
        }
        if start.elapsed() > per_file_timeout() {
            return Outcome::TimedOut(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Aborts the process if the currently loading file makes no progress for
/// [`WATCHDOG_ABORT_AFTER`]; a hung loader must fail loudly, never pass.
struct Watchdog {
    stop: Arc<AtomicBool>,
    current: Arc<Mutex<Option<(String, Instant)>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let current: Arc<Mutex<Option<(String, Instant)>>> = Arc::new(Mutex::new(None));
        let thread = {
            let stop = stop.clone();
            let current = current.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    let hung = current
                        .lock()
                        .ok()
                        .and_then(|guard| guard.clone())
                        .filter(|(_, since)| since.elapsed() > WATCHDOG_ABORT_AFTER);
                    if let Some((file, since)) = hung {
                        eprintln!(
                            "\nFATAL: loader made no progress on '{file}' for {:.0}s \
                             (limit {}s); aborting so the hang cannot be reported as a pass.",
                            since.elapsed().as_secs_f32(),
                            WATCHDOG_ABORT_AFTER.as_secs()
                        );
                        std::process::abort();
                    }
                }
            })
        };
        Self {
            stop,
            current,
            thread: Some(thread),
        }
    }

    fn track(&self, file: &str) {
        if let Ok(mut guard) = self.current.lock() {
            *guard = Some((file.to_string(), Instant::now()));
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn load_all_ufbx_data() {
    let Some(root) = corpus_dir() else {
        println!(
            "Skipping corpus run: set UFBX_TEST_DATA to the ufbx `data` directory, \
             or place the corpus at ../../libs/ufbx/data."
        );
        return;
    };

    let files = collect_fbx_files(&root);
    assert!(
        !files.is_empty(),
        "No .fbx files found in '{}'",
        root.display()
    );
    println!(
        "Corpus: {} FBX files under {} (real loader path: AssetServer + LoadContext)\n",
        files.len(),
        root.display()
    );

    let watchdog = Watchdog::start();
    let mut app = corpus_app(&root);

    let mut loaded = 0usize;
    let mut expected_failures: Vec<(String, String, String)> = Vec::new();
    let mut unexpected_failures: Vec<(String, String)> = Vec::new();
    let mut timeouts: Vec<String> = Vec::new();
    let mut panics: Vec<String> = Vec::new();
    let mut hostile_skipped: Vec<(String, String)> = Vec::new();
    let mut slowest: Vec<(Duration, String)> = Vec::new();
    let total_start = Instant::now();

    let (shard_start, shard_limit) = shard_config();
    let shard_end = shard_limit
        .map(|limit| (shard_start + limit).min(files.len()))
        .unwrap_or(files.len());

    for (index, path) in files
        .iter()
        .enumerate()
        .skip(shard_start)
        .take(shard_end.saturating_sub(shard_start))
    {
        let rel = relative_asset_path(&root, path);

        // Process-killing fixtures run in their own subprocess, never in-process.
        if let Some((_file, reason)) = HOSTILE_FILES.iter().find(|(file, _)| *file == rel.as_str())
        {
            println!("  HOSTILE-SKIP  {rel}: {reason}");
            hostile_skipped.push((rel, (*reason).to_string()));
            continue;
        }

        // Printed before the load: if the native parser aborts, this is the file.
        println!("  TRY {index:04} {rel}");

        watchdog.track(&rel);

        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_one(&mut app, &rel)));

        match outcome {
            Ok(Outcome::Loaded(elapsed)) => {
                loaded += 1;
                if elapsed > Duration::from_millis(500) {
                    println!("  OK  {:>7}ms  {}", elapsed.as_millis(), rel);
                    slowest.push((elapsed, rel.clone()));
                }
            }
            Ok(Outcome::Failed { error, elapsed }) => {
                if let Some((_file, reason)) = EXPECTED_LOAD_FAILURES
                    .iter()
                    .find(|(file, _)| *file == rel.as_str())
                {
                    println!("  EXPECTED-FAIL  {rel}: {reason}");
                    expected_failures.push((rel.clone(), (*reason).to_string(), error));
                } else {
                    eprintln!("  UNEXPECTED-FAIL  {rel}: {error}");
                    unexpected_failures.push((rel.clone(), error));
                }
                slowest.push((elapsed, rel.clone()));
            }
            Ok(Outcome::TimedOut(elapsed)) => {
                eprintln!(
                    "  TIMEOUT  {rel}: no completion after {:.0}s",
                    elapsed.as_secs_f32()
                );
                timeouts.push(format!("{rel} (>{}s)", elapsed.as_secs()));
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                eprintln!("  PANIC  {rel}: {message}");
                panics.push(format!("{rel}: {message}"));
                break;
            }
        }
    }

    watchdog.stop();
    slowest.sort_by(|a, b| b.0.cmp(&a.0));

    let sweep_count = shard_end.saturating_sub(shard_start);

    println!("\n================ corpus summary (real loader) ================");
    println!("  corpus dir      : {}", root.display());
    println!("  files           : {}", files.len());
    println!(
        "  shard           : [{}..{}) of {} (env UFBX_CORPUS_START/UFBX_CORPUS_LIMIT)",
        shard_start,
        shard_end,
        files.len()
    );
    println!(
        "  swept in-process: {}",
        sweep_count - hostile_skipped.len()
    );
    println!("  loaded          : {loaded}");
    println!("  expected-fail   : {}", expected_failures.len());
    println!("  unexpected-fail : {}", unexpected_failures.len());
    println!("  timeouts        : {}", timeouts.len());
    println!("  panics          : {}", panics.len());
    println!("  hostile-skip    : {}", hostile_skipped.len());
    println!(
        "  wall time       : {:.1}s",
        total_start.elapsed().as_secs_f32()
    );
    for (elapsed, rel) in slowest.iter().take(10) {
        println!("  slowest         : {:>7}ms  {rel}", elapsed.as_millis());
    }
    for (file, reason) in &hostile_skipped {
        println!("  hostile (isolated subprocess): {file} — {reason}");
    }
    for (file, reason, error) in &expected_failures {
        println!("  accepted-malformed: {file} — {reason} — {error}");
    }
    for (file, error) in &unexpected_failures {
        println!("  UNEXPECTED: {file} — {error}");
    }
    for timeout in &timeouts {
        println!("  TIMEOUT: {timeout}");
    }
    for panic in &panics {
        println!("  PANIC: {panic}");
    }

    // Stale allowlist entries: the file now loads. Not a failure (upstream corpora
    // change), but it must be visible so the allowlist does not rot silently.
    let stale: Vec<&str> = EXPECTED_LOAD_FAILURES
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| {
            !expected_failures
                .iter()
                .any(|(failed, _, _)| failed == *file)
                && !unexpected_failures
                    .iter()
                    .any(|(failed, _)| failed == *file)
                && !timeouts.iter().any(|t| t.starts_with(*file))
        })
        .collect();
    if !stale.is_empty() {
        println!("  WARNING stale allowlist entries (now load fine): {stale:?}");
    }
    let stale_hostile: Vec<&str> = HOSTILE_FILES
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| !files.iter().any(|p| relative_asset_path(&root, p) == *file))
        .collect();
    if !stale_hostile.is_empty() {
        println!("  WARNING hostile entries no longer present in corpus: {stale_hostile:?}");
    }
    println!(
        "L5-SHARD-RESULT start={shard_start} end={shard_end} files={sweep_count} loaded={loaded} expected={ef} unexpected={uf} timeouts={to} panics={pa} hostile={hs} wall_s={wall:.1}",
        ef = expected_failures.len(),
        uf = unexpected_failures.len(),
        to = timeouts.len(),
        pa = panics.len(),
        hs = hostile_skipped.len(),
        wall = total_start.elapsed().as_secs_f32(),
    );
    println!("==============================================================\n");

    assert!(
        panics.is_empty(),
        "loader panicked on corpus file(s) — see summary above: {panics:?}"
    );
    assert!(
        timeouts.is_empty(),
        "loader timed out on corpus file(s) — see summary above: {timeouts:?}"
    );
    assert!(
        unexpected_failures.is_empty(),
        "{} corpus file(s) failed through the real loader without an allowlist entry:\n{}",
        unexpected_failures.len(),
        unexpected_failures
            .iter()
            .map(|(file, error)| format!("  {file} — {error}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Probe body run only as a child process by
/// [`hostile_corpus_files_are_isolated_in_subprocesses`]: loads the single corpus
/// file named by `UFBX_ISOLATED_PROBE` through the real loader and reports the
/// outcome through an explicit exit code (see the `PROBE_EXIT_*` constants) plus a
/// structured stdout line, so the parent never has to guess from a bare nonzero
/// status. If the native parser aborts, this process dies — which is exactly the
/// evidence the parent records.
#[test]
fn isolated_hostile_probe() {
    let Ok(rel) = std::env::var("UFBX_ISOLATED_PROBE") else {
        return; // normal in-process runs: nothing to do
    };
    let root = corpus_dir().expect("corpus dir for isolated probe");
    let mut app = corpus_app(&root);
    println!("L5-HOSTILE-PROBE try {rel}");
    let (code, outcome) = match load_one(&mut app, &rel) {
        Outcome::Loaded(elapsed) => (PROBE_EXIT_LOADED, format!("loaded in {elapsed:?}")),
        Outcome::Failed { error, .. } => (
            PROBE_EXIT_SAFE_REJECTION,
            format!("safe-rejection: {error}"),
        ),
        Outcome::TimedOut(elapsed) => (PROBE_EXIT_TIMEOUT, format!("timed out after {elapsed:?}")),
    };
    println!("L5-HOSTILE-PROBE outcome={outcome}");
    // `exit` (not a harness return) so the status is exactly this outcome; the test
    // harness would otherwise report every outcome as a pass.
    std::process::exit(code);
}

/// Each [`HOSTILE_FILES`] entry is resolved in its own subprocess: an in-process
/// native abort would kill the whole corpus sweep. Verdicts:
/// - recognized native crash (Windows `0xC0000005`/`0xC0000409`, Unix `SIGSEGV`/`SIGABRT`): the
///   recorded failure mode, accepted;
/// - exit `42` **with** the structured outcome line: the parser safely rejected the malformed
///   input on this platform, also accepted (crashing must not be demanded on every OS);
/// - exit `0` (clean load): the exclusion is stale and the test fails;
/// - exit `43` (timeout), a Rust test panic (`101`), or any unrecognized status: the test fails.
#[test]
fn hostile_corpus_files_are_isolated_in_subprocesses() {
    let Some(root) = corpus_dir() else {
        println!("Skipping hostile-file isolation: no corpus directory.");
        return;
    };
    assert!(
        !HOSTILE_FILES.is_empty(),
        "HOSTILE_FILES should keep the process-killing fixtures documented"
    );

    let exe = std::env::current_exe().expect("current test executable");
    for (file, reason) in HOSTILE_FILES {
        let path = root.join(file);
        assert!(
            path.is_file(),
            "hostile fixture '{file}' not found under {}",
            root.display()
        );

        let mut child = std::process::Command::new(&exe)
            .args(["--exact", "isolated_hostile_probe", "--nocapture"])
            .env("UFBX_ISOLATED_PROBE", file)
            .env("UFBX_TEST_DATA", &root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn isolated probe subprocess");

        let status = match wait_with_timeout(&mut child, Duration::from_secs(60)) {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "hostile fixture '{file}' hung the isolated loader for >60s; \
                     a hang can never pass. Reason: {reason}"
                );
            }
        };
        let output = child
            .wait_with_output()
            .expect("collect isolated probe output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("--- isolated probe '{file}' stdout ---\n{stdout}");
        if !output.stderr.is_empty() {
            eprintln!(
                "--- isolated probe '{file}' stderr ---\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        match classify_probe_termination(&status) {
            ProbeTermination::NativeCrash(how) => {
                println!("HOSTILE-CONFIRMED '{file}': native termination ({how}). Reason: {reason}")
            }
            ProbeTermination::SafeRejection => {
                assert!(
                    stdout.contains("L5-HOSTILE-PROBE outcome=safe-rejection"),
                    "isolated probe exited 42 for '{file}' but its structured outcome line is \
                     missing; refusing to treat an unlabelled status as a safe rejection"
                );
                println!(
                    "HOSTILE-SAFE-REJECTION '{file}': the parser rejects this malformed input \
                     gracefully in isolation (accepted; may differ per OS). Reason: {reason}"
                );
            }
            ProbeTermination::Loaded => panic!(
                "HOSTILE exclusion stale for '{file}': it now loads cleanly in \
                 isolation — remove it from HOSTILE_FILES (recorded reason: {reason})"
            ),
            ProbeTermination::TimedOut => panic!(
                "hostile fixture '{file}' timed out inside the isolated loader; \
                 a hang can never pass. Reason: {reason}"
            ),
            ProbeTermination::Unrecognized(detail) => panic!(
                "hostile fixture '{file}' terminated with an unrecognized status ({detail}); \
                 expected a recognized native crash or a graceful parser rejection. \
                 Reason: {reason}"
            ),
        }
    }
}

/// Wait for `child` at most `timeout`; returns `None` if it had to be killed.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("failed to poll isolated probe: {error}"),
        }
    }
}
