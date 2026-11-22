//! WHY-24 spike: what would a "before/after snapshot around a Bash call" cost,
//! and how racy is it when the collector takes it asynchronously?
//! Measurement only; nothing here is product code. Results and analysis live
//! in `docs/spikes/WHY-24-shell-write.md`.
//!
//! ```text
//! cargo build --release -p why-hook
//! cargo run --release -p why-hook --example snapshot_bench -- snapshot [runs]
//! cargo run --release -p why-hook --example snapshot_bench -- race [trials]
//! cargo run --release -p why-hook --example snapshot_bench -- mtime
//! ```
//!
//! * `snapshot`: cost of `git status` + `lstat` of the changed set + hashing
//!   it, for this WHY worktree (read-only) and synthetic 10k / 50k-file repos
//!   with 0, 10 and 500 dirty files; warm and "cold" page cache.
//! * `race`: spawns the real `why-hook` with a `PreToolUse` Bash payload, then
//!   runs `sed -i` the way a harness would once the hook exits, while a
//!   stand-in collector starts its snapshot on receiving the line. Counts how
//!   often the snapshot still sees the old content.
//! * `mtime`: timestamp granularity on btrfs and tmpfs, inode behaviour of
//!   `sed -i` vs `>`, and the edit-then-revert case.
//!
//! Synthetic repos live under `src-tauri/target/why24-bench/` (btrfs, ignored
//! by git; `/tmp` here is tmpfs, i.e. RAM, which would make cache numbers
//! meaningless) and are deleted when the run ends. Every git call mirrors
//! `GitRunner`: absolute `/usr/bin/git`, cleared env, `GIT_OPTIONAL_LOCKS=0`,
//! `-c core.fsmonitor=false`, no global/system config.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hasher;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use why_wire::Allowlist;

const GIT: &str = "/usr/bin/git";

// ---------------------------------------------------------------- helpers

/// A git command with the runner's protections and no inherited config.
/// `HOME` points at an empty dir so nothing from the developer leaks in.
fn git(dir: &Path, home: &Path) -> Command {
    let mut c = Command::new(GIT);
    c.env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"]);
    c
}

/// Fixture-only git (may write: it is our own temp repo). Optional locks on,
/// so `update-index --refresh` really refreshes the stat cache.
fn git_fixture(dir: &Path, home: &Path, args: &[&str]) {
    let st = git(dir, home)
        .env("GIT_OPTIONAL_LOCKS", "1")
        .args([
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@example.invalid",
        ])
        .args(args)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    // `update-index --refresh` exits 1 when something is still dirty; fine.
    assert!(
        st.success() || args[0] == "update-index",
        "git {args:?} failed"
    );
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// (median, p95) of a sample set, in whatever unit the samples are.
fn med_p95(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
    (at(0.50), at(0.95))
}

/// Deterministic pseudo-random source text of roughly `size` bytes.
fn content(seed: u64, size: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let mut out = Vec::with_capacity(size + 64);
    let mut n = 0u64;
    while out.len() < size {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        writeln!(out, "    let v{n} = {};", x >> 33).unwrap();
        n += 1;
    }
    out
}

/// SipHash of a whole file: a stand-in for whatever real hasher WHY would
/// use (blake3 would be several times faster per byte; reading dominates for
/// small files anyway).
fn hash_file(p: &Path) -> Option<u64> {
    let bytes = fs::read(p).ok()?;
    let mut h = DefaultHasher::new();
    h.write(&bytes);
    Some(h.finish())
}

/// Drop the page cache for every file under `root` (contents only: dentries
/// and inodes stay cached; a true cold cache needs root and drop_caches).
fn evict(root: &Path) {
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let name = e.file_name();
            if name == "target" || name == "node_modules" {
                continue;
            }
            evict(&p);
        } else if ft.is_file()
            && let Ok(f) = fs::File::open(&p)
        {
            use std::os::fd::AsRawFd;
            // SAFETY: plain syscall on a valid, open fd.
            unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        }
    }
}

// ---------------------------------------------------------------- snapshot

/// Paths in `git status --porcelain=v2 -z` output (changed + untracked).
fn parse_status(out: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut recs = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(r) = recs.next() {
        let s = String::from_utf8_lossy(r);
        // Path is everything after the Nth space; N depends on record type.
        let skip = match r[0] {
            b'1' => 8,
            b'2' => 9,
            b'u' => 10,
            b'?' => 1,
            _ => continue,
        };
        if let Some(p) = s.splitn(skip + 1, ' ').nth(skip) {
            paths.push(p.to_owned());
        }
        if r[0] == b'2' {
            recs.next(); // rename: the original path follows as its own record
        }
    }
    paths
}

struct Snap {
    status: f64,
    stat: f64,
    hash: f64,
    git_hash: f64,
    files: usize,
    bytes: u64,
}

/// One "before" snapshot: status, lstat every changed path, hash them all
/// (their pre-command content is in no git object, so it must be read now).
fn snapshot(repo: &Path, home: &Path, with_git_hash: bool) -> Snap {
    let t = Instant::now();
    let out = git(repo, home)
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let paths = parse_status(&out.stdout);
    let status = ms(t.elapsed());

    let t = Instant::now();
    let stats: Vec<_> = paths
        .iter()
        .filter_map(|p| {
            let full = repo.join(p);
            let m = fs::symlink_metadata(&full).ok()?;
            m.is_file()
                .then_some((full, (m.mtime(), m.mtime_nsec(), m.ino(), m.size())))
        })
        .collect();
    let stat = ms(t.elapsed());
    let bytes = stats.iter().map(|s| s.1.3).sum();

    let t = Instant::now();
    for (p, _) in &stats {
        std::hint::black_box(hash_file(p));
    }
    let hash = ms(t.elapsed());

    let mut git_hash = 0.0;
    if with_git_hash && !stats.is_empty() {
        let t = Instant::now();
        let mut child = git(repo, home)
            .args(["hash-object", "--no-filters", "--stdin-paths"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for (p, _) in &stats {
            writeln!(stdin, "{}", p.display()).unwrap();
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
        git_hash = ms(t.elapsed());
    }
    Snap {
        status,
        stat,
        hash,
        git_hash,
        files: stats.len(),
        bytes,
    }
}

fn bench_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/why24-bench");
    fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

/// A committed synthetic repo of `n` files, ~20 per directory, 1–8 KB each.
fn make_repo(root: &Path, home: &Path, n: usize) -> PathBuf {
    let repo = root.join(format!("repo{n}"));
    for i in 0..n {
        let dir = repo.join(format!("d{}/e{}", i / 1000, (i / 20) % 50));
        fs::create_dir_all(&dir).unwrap();
        let size = 1024 + (i * 7919) % 7168;
        fs::write(dir.join(format!("f{i}.rs")), content(i as u64, size)).unwrap();
    }
    git_fixture(&repo, home, &["init", "-q", "-b", "main"]);
    git_fixture(&repo, home, &["add", "-A"]);
    git_fixture(&repo, home, &["commit", "-q", "-m", "init"]);
    repo
}

fn file_path(repo: &Path, i: usize) -> PathBuf {
    repo.join(format!("d{}/e{}/f{i}.rs", i / 1000, (i / 20) % 50))
}

/// Make `k` files dirty: half modified tracked files, half new untracked.
fn dirty(repo: &Path, k: usize) {
    for i in 0..k / 2 {
        let p = file_path(repo, i * 13);
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        writeln!(f, "// dirty").unwrap();
    }
    for i in 0..k - k / 2 {
        fs::write(
            repo.join(format!("d0/new{i}.rs")),
            content(1_000_000 + i as u64, 4096),
        )
        .unwrap();
    }
}

fn undirty(repo: &Path, home: &Path, k: usize) {
    for i in 0..k / 2 {
        let idx = i * 13;
        let size = 1024 + (idx * 7919) % 7168;
        fs::write(file_path(repo, idx), content(idx as u64, size)).unwrap();
    }
    for i in 0..k - k / 2 {
        fs::remove_file(repo.join(format!("d0/new{i}.rs"))).unwrap();
    }
    // Rewriting restored content but bumped mtimes; refresh the index's stat
    // cache so "0 dirty" really means zero files git must re-read.
    git_fixture(repo, home, &["update-index", "-q", "--refresh"]);
}

/// Touch `k` tracked files (new mtime, same content) without refreshing the
/// index: what every edit-then-revert or `cargo fmt` that changes nothing
/// leaves behind. WHY never refreshes (GIT_OPTIONAL_LOCKS=0), so git re-reads
/// and re-hashes these on every status (and would run filter drivers on them).
fn touch(repo: &Path, k: usize) {
    for i in 0..k {
        let p = file_path(repo, i * 13);
        let data = fs::read(&p).unwrap();
        fs::write(&p, data).unwrap();
    }
}

fn measure(label: &str, repo: &Path, home: &Path, runs: usize, evict_root: &Path) {
    let _ = snapshot(repo, home, true); // warm-up
    let mut cols: [Vec<f64>; 5] = Default::default();
    let mut files = 0;
    let mut bytes = 0;
    for _ in 0..runs {
        let s = snapshot(repo, home, true);
        cols[0].push(s.status);
        cols[1].push(s.stat);
        cols[2].push(s.hash);
        cols[3].push(s.git_hash);
        cols[4].push(s.status + s.stat + s.hash);
        (files, bytes) = (s.files, s.bytes);
    }
    let cold_runs = 5.min(runs);
    let mut cold = Vec::new();
    for _ in 0..cold_runs {
        evict(evict_root);
        let s = snapshot(repo, home, false);
        cold.push(s.status + s.stat + s.hash);
    }
    let f = |v: &Vec<f64>| {
        let (m, p) = med_p95(v.clone());
        format!("{m:7.2} / {p:7.2}")
    };
    println!(
        "| {label:<26} | {files:>5} | {:>6} | {} | {} | {} | {} | {} | {} |",
        bytes / 1024,
        f(&cols[0]),
        f(&cols[1]),
        f(&cols[2]),
        f(&cols[3]),
        f(&cols[4]),
        f(&cold)
    );
}

fn run_snapshot(runs: usize) {
    let root = bench_root();
    let tmp = tempfile::Builder::new()
        .prefix("snap")
        .tempdir_in(&root)
        .unwrap();
    let base = tmp.path();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();

    println!("{runs} warm runs (5 cold) per row; milliseconds, median / p95\n");
    println!(
        "| repo, state                | files |    KiB | git status        | lstat             | hash (SipHash)    | git hash-object   | total (warm)      | total (cold*)     |"
    );
    println!("|{}|", ["---"; 9].join("|"));

    // This WHY worktree, read-only (no fixture git, no refresh, no writes).
    let why = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap();
    let tracked = git(&why, &home).args(["ls-files", "-z"]).output().unwrap();
    let ntracked = tracked.stdout.iter().filter(|b| **b == 0).count();
    measure(
        &format!("WHY ({ntracked} tracked)"),
        &why,
        &home,
        runs,
        &why,
    );

    for n in [10_000, 50_000] {
        let t = Instant::now();
        let repo = make_repo(base, &home, n);
        eprintln!("built {n}-file repo in {:.1} s", t.elapsed().as_secs_f64());
        for k in [0, 10, 500] {
            dirty(&repo, k);
            measure(
                &format!("{}k, {k} dirty", n / 1000),
                &repo,
                &home,
                runs,
                &repo,
            );
            undirty(&repo, &home, k);
        }
        touch(&repo, 500);
        measure(
            &format!("{}k, 500 touched", n / 1000),
            &repo,
            &home,
            runs,
            &repo,
        );
        fs::remove_dir_all(&repo).unwrap();
    }
    println!(
        "\n*cold = page cache for file contents dropped with posix_fadvise; dentries/inodes stay cached."
    );
}

// ---------------------------------------------------------------- race

#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Lower bound: the collector already knows the path and just reads it.
    Read,
    /// Realistic: `git status` first (to learn the changed set), then read.
    StatusThenRead,
}

struct Outcome {
    saw_old: bool,
}

fn hook_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("why-hook")
}

fn run_race(trials: usize) {
    let root = bench_root();
    let tmp = tempfile::Builder::new()
        .prefix("race")
        .tempdir_in(&root)
        .unwrap();
    let base = tmp.path();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let repo = make_repo(base, &home, 10_000);
    dirty(&repo, 10);
    let target = file_path(&repo, 0); // one of the dirty tracked files
    let original = fs::read(&target).unwrap();
    let mut with_mark = original.clone();
    with_mark.extend_from_slice(b"// OLDMARK\n");

    // Socket + data dir in /tmp: a unix socket path must fit in 108 bytes.
    let short = tempfile::Builder::new()
        .prefix("wr")
        .tempdir_in("/tmp")
        .unwrap();
    let data = short.path().join("data");
    let socket = short.path().join("s.sock");
    fs::create_dir_all(&data).unwrap();
    let allow = Allowlist::new(1, vec![repo.to_string_lossy().into_owned()], vec![]);
    fs::write(data.join("hook-allowlist.json"), allow.to_bytes()).unwrap();

    // Stand-in collector: on each line, start the snapshot at once.
    let mode = Arc::new(Mutex::new(Mode::Read));
    let (tx, rx) = mpsc::channel::<Outcome>();
    let listener = UnixListener::bind(&socket).unwrap();
    {
        let (mode, repo, home, target) = (mode.clone(), repo.clone(), home.clone(), target.clone());
        std::thread::spawn(move || {
            for mut conn in listener.incoming().flatten() {
                let mut sink = Vec::new();
                let _ = conn.read_to_end(&mut sink);
                let m = *mode.lock().unwrap();
                if let Mode::StatusThenRead = m {
                    let _ = git(&repo, &home)
                        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"])
                        .output();
                }
                let now = fs::read(&target).unwrap_or_default();
                let saw_old = now.windows(7).any(|w| w == b"OLDMARK");
                let _ = tx.send(Outcome { saw_old });
            }
        });
    }

    let payload = format!(
        r#"{{"session_id":"s","cwd":{},"hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"t","tool_input":{{"command":"sed -i s/OLDMARK/NEWMARK/ f"}}}}"#,
        serde_json::to_string(&repo).unwrap()
    );
    let commands = [
        (
            "sed -i",
            format!("sed -i 's/OLDMARK/NEWMARK/' '{}'", target.display()),
        ),
        ("echo >", format!("echo NEWMARK > '{}'", target.display())),
    ];

    println!(
        "{trials} trials per row; % of runs where the async snapshot read the OLD content (it won the race)\n"
    );
    println!(
        "| command | collector snapshot | harness gap after hook exit | snapshot won | hook p50 ms | command p50 ms |"
    );
    println!("|---|---|---|---|---|---|");
    for (name, sed) in &commands {
        for m in [Mode::Read, Mode::StatusThenRead] {
            *mode.lock().unwrap() = m;
            for gap_ms in [0u64, 1, 5, 20] {
                let (mut won, mut lost) = (0, 0);
                let (mut hook_t, mut sed_t) = (Vec::new(), Vec::new());
                for _ in 0..trials {
                    fs::write(&target, &with_mark).unwrap();
                    let t = Instant::now();
                    let mut child = Command::new(hook_binary())
                        .args([
                            "--harness",
                            "claude-code",
                            "--instance",
                            "installed",
                            "--data-dir",
                        ])
                        .arg(&data)
                        .arg("--socket-path")
                        .arg(&socket)
                        .env_clear()
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap();
                    child
                        .stdin
                        .take()
                        .unwrap()
                        .write_all(payload.as_bytes())
                        .unwrap();
                    child.wait().unwrap();
                    hook_t.push(ms(t.elapsed()));
                    if gap_ms > 0 {
                        std::thread::sleep(Duration::from_millis(gap_ms));
                    }
                    let t = Instant::now();
                    Command::new("/bin/sh")
                        .args(["-c", sed])
                        .env_clear()
                        .env("PATH", "/usr/bin:/bin")
                        .status()
                        .unwrap();
                    sed_t.push(ms(t.elapsed()));
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(o) if o.saw_old => won += 1,
                        Ok(_) => {}
                        Err(_) => lost += 1,
                    }
                }
                assert_eq!(lost, 0, "hook events did not reach the stand-in collector");
                println!(
                    "| {name} | {m:?} | {gap_ms} ms | {:5.1} % | {:.2} | {:.2} |",
                    100.0 * won as f64 / trials as f64,
                    med_p95(hook_t).0,
                    med_p95(sed_t).0
                );
            }
        }
    }
    assert!(!data.join("spool").exists(), "events must go to the socket");
}

// ---------------------------------------------------------------- mtime

fn mtime_ns(p: &Path) -> i128 {
    fs::metadata(p)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128
}

fn granularity(label: &str, dir: &Path) {
    let (a, b) = (dir.join("a"), dir.join("b"));
    let n = 2000;
    // 1. Two writes, no stat in between: do they share a coarse timestamp?
    let (mut same, mut min_delta) = (0, i128::MAX);
    for i in 0..n {
        fs::write(&a, format!("{i:08}")).unwrap();
        fs::write(&b, format!("{i:08}")).unwrap();
        let d = mtime_ns(&b) - mtime_ns(&a);
        if d == 0 {
            same += 1;
        } else {
            min_delta = min_delta.min(d);
        }
    }
    // 2. write, stat, write same size: would a stat fast path miss the change?
    let mut missed = 0;
    for i in 0..n {
        fs::write(&a, format!("{i:08}")).unwrap();
        let m1 = mtime_ns(&a);
        fs::write(&a, format!("{:08}", i + 1)).unwrap();
        if mtime_ns(&a) == m1 {
            missed += 1;
        }
    }
    // 3. The coarse tick itself: many fresh files, no stat until the end, then
    //    the smallest step between distinct mtimes.
    let files: Vec<_> = (0..n).map(|i| dir.join(format!("g{i}"))).collect();
    let t = Instant::now();
    for p in &files {
        fs::write(p, b"x").unwrap();
    }
    let span = t.elapsed();
    let mut stamps: Vec<i128> = files.iter().map(|p| mtime_ns(p)).collect();
    stamps.sort();
    stamps.dedup();
    let tick = stamps.windows(2).map(|w| w[1] - w[0]).min().unwrap_or(0);
    println!(
        "| {label} | {:.1} % | {} | {:.1} % | {} distinct over {:.1} ms, smallest step {:.3} ms |",
        100.0 * same as f64 / n as f64,
        if min_delta == i128::MAX {
            "none seen".to_owned()
        } else {
            format!("{min_delta} ns")
        },
        100.0 * missed as f64 / n as f64,
        stamps.len(),
        ms(span),
        tick as f64 / 1e6
    );
}

fn run_mtime() {
    let root = bench_root();
    let btrfs = tempfile::Builder::new()
        .prefix("mt")
        .tempdir_in(&root)
        .unwrap();
    let tmpfs = tempfile::Builder::new()
        .prefix("mt")
        .tempdir_in("/tmp")
        .unwrap();
    println!("2000 iterations each\n");
    println!(
        "| fs | 2 writes, no stat between: same mtime | min nonzero delta | write, stat, write (same size): mtime unchanged | coarse tick |"
    );
    println!("|---|---|---|---|---|");
    granularity("btrfs", btrfs.path());
    granularity("tmpfs", tmpfs.path());

    // Inode and edit-then-revert behaviour (btrfs).
    let f = btrfs.path().join("f.txt");
    fs::write(&f, "alpha\nbeta\n").unwrap();
    let ino = |p: &Path| fs::metadata(p).unwrap().ino();
    let sh = |cmd: &str| {
        Command::new("/bin/sh")
            .args(["-c", cmd])
            .current_dir(btrfs.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .unwrap()
    };
    let i0 = ino(&f);
    sh("echo gamma > f.txt");
    let i1 = ino(&f);
    sh("sed -i 's/gamma/delta/' f.txt");
    let i2 = ino(&f);
    println!(
        "\n`>` keeps the inode: {}; `sed -i` replaces it: {}",
        i0 == i1,
        i1 != i2
    );

    fs::write(&f, "alpha\nbeta\n").unwrap();
    let (h0, m0) = (hash_file(&f), mtime_ns(&f));
    sh("sed -i 's/alpha/ALPHA/' f.txt && sed -i 's/ALPHA/alpha/' f.txt");
    let (h1, m1) = (hash_file(&f), mtime_ns(&f));
    println!(
        "edit-then-revert: mtime changed: {}, content hash equal: {} (net change = none, which is correct for line origins)",
        m0 != m1,
        h0 == h1
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let n = args.next().and_then(|a| a.parse().ok());
    match cmd.as_str() {
        "snapshot" => run_snapshot(n.unwrap_or(30)),
        "race" => run_race(n.unwrap_or(200)),
        "mtime" => run_mtime(),
        _ => eprintln!("usage: snapshot_bench <snapshot|race|mtime> [n]"),
    }
}
