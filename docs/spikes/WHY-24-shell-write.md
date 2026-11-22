# WHY-24 spike: capturing what shell commands write

Timeboxed spike (decision 31): can a before/after snapshot of the worktree run around an agent's Bash call within the hook's fail-open latency budget, or only app-side? This note covers measurements only. No product code changed. The bench is `src-tauri/crates/why-hook/examples/snapshot_bench.rs`.

**Recommendation: defer to v0.3.1.** Do not snapshot in the hook. The design worth building is app-side (option C below), and it needs a contract change and new origin semantics that do not belong in v0.3. Details are in [Recommendation](#recommendation).

## Setup

| | |
|---|---|
| Machine | Intel Core Ultra 7 255H (16 threads), 14 GB RAM, NVMe SSD |
| Kernel | Linux 7.3.3-1-cachyos, `CONFIG_HZ=1000` |
| Filesystem | btrfs (`noatime,compress=zstd:1,ssd`) for the worktree and the bench repos. `/tmp` is **tmpfs** (RAM), so the bench repos live under `src-tauri/target/why24-bench/` (git-ignored, btrfs, deleted afterwards). No ext4 was available. |
| git | `/usr/bin/git` 2.56.0, cleared environment, `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_OPTIONAL_LOCKS=0`, `-c core.fsmonitor=false` (the `GitRunner` rules) |
| Repos | This WHY worktree (690 tracked files, measured read-only as it was: 1 untracked file). Synthetic repos with 10k and 50k files, about 20 files per directory, 1–8 KB each (10k ≈ 45 MB). Dirty states: 0, 10 and 500 files, half modified tracked files and half new untracked ones; 500 "touched" = new mtime with identical content and no index refresh. |
| Machine state | Not quiesced. Other sessions may have been building at the same time, which is part of why p95 sits at about 1.7× the median. |

Run it:

```bash
cargo build --release -p why-hook
cargo run --release -p why-hook --example snapshot_bench -- snapshot 30
cargo run --release -p why-hook --example snapshot_bench -- race 200
cargo run --release -p why-hook --example snapshot_bench -- mtime
cargo run --release -p why-hook --example latency 1000
```

## 1. Snapshot cost

One "before" snapshot does three things:
1. `git status --porcelain=v2 -z --untracked-files=all --ignore-submodules=dirty` (a spawned child, so process start-up is included);
2. `lstat` every changed path;
3. hash every changed file.

The hash column is a **worst case** (a first snapshot): the bench hashes the whole dirty set on every run. In the steady state only files whose stat changed would be re-hashed, so the cost there is close to status + lstat. Dirty files must be read at "before" time at least once, because their pre-command content exists in no git object. A clean tracked file's content is in the index blob and needs no read.

The hash is std SipHash over the whole file, read included. A real hasher (blake3) would differ: faster per byte, with the same read cost. `git hash-object --no-filters --stdin-paths` is shown as the git-side alternative.

Milliseconds, **median / p95** over 30 warm runs:

| repo, state | files | KiB | git status | lstat | hash (SipHash) | git hash-object | total warm | total cold* |
|---|--:|--:|---|---|---|---|---|---|
| WHY (690 tracked), as is | 1 | 21 | 2.60 / 3.78 | 0.00 / 0.02 | 0.01 / 0.05 | 0.82 / 1.48 | **2.63 / 3.85** | 2.86 / 4.11 |
| 10k, 0 dirty | 0 | 0 | 9.55 / 15.48 | – | – | – | **9.55 / 15.48** | 10.59 / 10.72 |
| 10k, 10 dirty | 10 | 36 | 8.27 / 14.11 | 0.02 / 0.04 | 0.04 / 0.08 | 1.03 / 1.64 | **8.31 / 14.23** | 10.61 / 12.50 |
| 10k, 500 dirty | 500 | 2133 | 9.62 / 18.17 | 0.65 / 1.11 | 1.95 / 4.00 | 8.35 / 10.34 | **12.49 / 20.43** | 59.33 / 62.80 |
| 10k, 500 touched | 0 | 0 | 18.64 / 22.07 | – | – | – | **18.64 / 22.07** | 57.99 / 64.28 |
| 50k, 0 dirty | 0 | 0 | 54.28 / 96.69 | – | – | – | **54.28 / 96.69** | 50.75 / 61.59 |
| 50k, 10 dirty | 10 | 36 | 57.38 / 95.65 | 0.03 / 0.07 | 0.05 / 0.13 | 1.56 / 3.71 | **57.51 / 95.73** | 67.66 / 100.00 |
| 50k, 500 dirty | 500 | 2133 | 59.26 / 98.69 | 1.00 / 1.85 | 3.20 / 6.91 | 12.78 / 13.89 | **64.40 / 103.56** | 103.72 / 146.65 |
| 50k, 500 touched | 0 | 0 | 68.76 / 111.20 | – | – | – | **68.76 / 111.20** | 103.49 / 312.90 |

\*Cold has three caveats:
- It was produced with `posix_fadvise(POSIX_FADV_DONTNEED)` on every file, which drops **file contents only, not dentries/inodes**.
- `fadvise` cannot drop dirty pages, and the bench had just written these repos, so cold is a **floor**.
- It uses only 5 samples, so its "p95" is the max (the 312.90 is one outlier). The WHY row did not evict the main repository's object store or index.

For a true cold-cache number, run this once before each cold sample (needs root):

```bash
sudo sh -c 'sync; echo 3 > /proc/sys/vm/drop_caches'
```

Reading the table:
- **`git status` dominates and scales with tracked files, not with dirty files:** about 1 µs per file warm (10k ≈ 9 ms, 50k ≈ 55 ms). Dirty files add little until git has to read them.
- **"Touched" files double the cost** (10k: 9.5 → 18.6 ms). WHY runs git with `GIT_OPTIONAL_LOCKS=0`, so it never refreshes the index. Every file whose stat changed but whose content did not (edit-then-revert, `cargo fmt` that changed nothing, `touch`) is re-read and re-hashed by git on **every** status, and run through its clean filter if one is set (see §4). The agent's own `git status`/`commit` would heal the index, but WHY can't count on that.
- `lstat` of the changed set is cheap: 0.02 ms for 10 files, 0.65–1.0 ms for 500. This is the cost of option D below.
- The untracked-cache and fsmonitor speedups are not available: the first needs WHY to write the index, and the second is disabled because it executes a program from repository config.
- **Not measured:** the WHY repo with 10 and 500 dirty files. Extrapolate from the synthetic deltas (+0.04 ms and about +2.6 ms warm on top of 2.6 ms).

## 2. Where it can run

### Baseline: the hook today

`examples/latency.rs`, 1000 runs, wall time per hook run (spawn, payload, exit), in milliseconds. It was measured on `main` `1f371e7`, which includes WHY-65's `prepare_line` change. The earlier run on `f42e078` gave the same picture: p50 0.44–0.54, p99 1.0–1.3.

| scenario | p50 | p90 | p99 | max |
|---|--:|--:|--:|--:|
| app down (spool) | 0.44 | 0.57 | 1.47 | 2.69 |
| app up (socket) | 0.43 | 0.54 | 0.69 | 0.87 |
| outside allowlist | 0.40 | 0.48 | 0.62 | 0.84 |

The snapshot and race numbers were measured on `f42e078`. They depend on git and the filesystem, not on the hook's code, except that the race rows include the hook run (~0.5–0.9 ms there).

There are two budgets to judge against:
- **The target, "a few ms" per event** (payloads.md §4). Today's hook sits at about 0.5 ms.
- **The ceiling:** `SEND_BUDGET` = 100 ms, with ≤ 200 ms per payloads.md §7. The installed hook `timeout` is 5 s for `PreToolUse`, and a hook that hits it is cancelled and adds the full timeout to that tool call.

### A. In `why-hook`, synchronously on `PreToolUse` (Bash)

| | |
|---|---|
| Latency | Hook + snapshot ≈ 0.5 ms + the table above: WHY repo ≈ 3 ms; 10k ≈ 9–13 ms (p95 15–20); 10k touched ≈ 19 ms; 50k ≈ 55–70 ms (**p95 ≈ 100–110 ms, at or past `SEND_BUDGET`**); cold 50k 100–300 ms. That is 5× to 200× today's hook, on **every** Bash call, growing with repo size. It needs a hard deadline that kills git, and on timeout the snapshot is lost (Unknown). |
| Race | None: the agent waits, so the snapshot always comes first. |
| Safety | **This is the blocker, ahead of latency.** `git status` runs `filter.<name>.clean` drivers chosen by repository-controlled `.gitattributes`/config for every stat-dirty file, and the "touched" row shows that these are re-filtered on every call. In the app, `read_status` needs a `ReadPermit` from the filter guard (WHY-39). The hook has no `GitRunner` and no guard, so it would execute repository-controlled programs **unless the app passes it trust state** (for example per-root "filters trusted" in `hook-allowlist.json`). That is a `why-wire` contract change, plus re-implementing the runner's env and flags and a kill-on-deadline inside the hook. Hashing itself is safe (in Rust, or `hash-object --no-filters`). |
| WHY closed | Works (the snapshot travels in the spooled envelope), but the hook gets bigger and slower for users who are not even looking. |

**Verdict:** rejected. It is unsafe without a contract change, and it misses the "few ms" target by an order of magnitude on 10k files and the ceiling on 50k.

### B. In the collector, asynchronously when the Pre event arrives

The stand-in setup:
- The real `why-hook` binary gets a `PreToolUse` Bash payload and sends it to a stand-in collector socket.
- The stand-in harness waits for the hook to exit, then waits a fixed gap, then runs the command against a dirty tracked file in the 10k repo.
- The stand-in collector starts its snapshot the moment the line arrives. In mode `Read` it reads one known 4 KB file directly. In mode `StatusThenRead` it runs `git status` first (it has to learn the changed set) and then reads.

200 trials per row. **"Snapshot won"** = it read the old content.

| command | collector snapshot | harness gap after hook exit | snapshot won | hook p50 ms | command p50 ms |
|---|---|---|--:|--:|--:|
| `sed -i` | Read | 0 / 1 / 5 / 20 ms | 100 / 100 / 100 / 100 % | 0.6–0.7 | 1.4–1.6 |
| `sed -i` | StatusThenRead | 0 / 1 / 5 / 20 ms | **0 / 0 / 5.5 / 100 %** | 0.7–0.9 | 1.5–2.1 |
| `echo >` | Read | 0 / 1 / 5 / 20 ms | 100 / 100 / 100 / 100 % | 0.5–0.7 | 0.7–1.2 |
| `echo >` | StatusThenRead | 0 / 1 / 5 / 20 ms | **0 / 0 / 0 / 100 %** | 0.9–1.0 | 1.1–1.6 |

How to read it:
- **These are best-case numbers for the collector.** The stand-in has no mpsc queue, no SQLite ingest and no git semaphore, so the real collector would win less often at each gap. A real snapshot would have to be triggered in the socket server before ingest.
- **`Read` wins only because it reads one known file.** Reading and hashing 500 dirty files takes about 2.6 ms warm (much more cold), which is longer than the 0.7–1.5 ms a fast command needs, so with a large dirty set even "already know the paths" would lose part of the race.
- **`StatusThenRead` loses almost every time** unless the harness waits ≥ 20 ms after the hook exits (status on 10k takes ~9 ms; on 50k ~55 ms, so it would lose even at 20 ms).
- **The real harness gap was not measured** (no real agent runs in this spike). The recorded fixtures give an **upper bound**: a trivial Bash call takes **Pre→Post 92 ms** in Claude Code 2.1.322 (`exit 3`) and **154 ms** in Codex 0.170.0 (`printf … > file`), and that span includes the command. So the gap lies somewhere in 0–90 ms, and that is the open variable for option B.
- **While WHY is closed: nothing.** Events are spooled and the worktree state at Pre time is gone for good. Those shell writes stay leads (Unknown).

**Verdict:** naive B (status on Pre) is racy for exactly the fast commands that matter (`sed -i`, `>`). It is usable only when the before-state is already known, which leads to C.

### C. App-side, "the after of the previous call is the before of the next" (recommended design, v0.3.1)

The app already tracks the worktree:
- After every tool call's **Post** event (and on watcher events), it takes a full snapshot: status + lstat + hashes of the dirty files. The Post snapshot has no race with the command, which has finished. Its only race is with the next write, and the agent can't start another tool until the PostToolUse hook returns and usually a model turn follows. **Parallel tool calls** are the exception.
- On **Pre**, the app has nothing slow to do. The before-state of clean files is the index blob, and dirty files were hashed at the previous Post. It only checks that its cached state is still current: an `lstat` of the cached dirty set (0.02–1 ms), plus "did the watcher see anything since then?". Any difference means a write outside any tool window, so the affected files become Unknown.
- **On Post**, it diffs the new snapshot against the cached one: those changes are the command's (subject to §3).
- **Hook cost: zero.** App cost per Bash call: one async status (10–70 ms, in the background), which the existing filter guard and `ReadPermit` already cover. The hashing is in Rust.
- **Weak spots:**
  - blind while WHY is closed (as B);
  - a dirty set that grows large makes Pre-time verification approach the race again (500 files ≈ 1 ms of lstat, which is fine; hashing is not needed at Pre);
  - concurrency (§3).

### D. Hook records only a stat fingerprint (optional add-on to C)

The app writes the worktree's cached dirty set next to the allowlist. On Pre, the hook `lstat`s those paths (no git, no reads, no filters) and puts `(path, mtime_ns, ctime_ns, ino, size)` in the envelope:
- **Cost:** the lstat column, 0.02 ms (10 files) to 1 ms (500 files), plus reading the list.
- **What it gives:** a synchronous Pre-time fingerprint that makes the C/B race **detectable**. If a file's stat at app-read time differs from the hook's fingerprint, that file's before-content was lost, and it becomes Unknown instead of being misattributed.
- **What it misses:** files that were clean (their content is in the index anyway) and new untracked files that did not exist yet (they are created by the command, so "before = absent" is correct).
- **Cost to build:** a contract change (envelope field + list file) and a hook that reads one more file. It is safe: no git and no file contents.

## 3. Correctness limits

- **Timestamp granularity (measured, btrfs and tmpfs, kernel 7.3):**

  | fs | 2 writes, no stat between: same mtime | write → stat → write (same size): mtime unchanged | coarse tick |
  |---|--:|--:|---|
  | btrfs | 100 % | **0 %** | ~0.75–1 ms (24 distinct stamps over 22.9 ms) |
  | tmpfs | 100 % | **0 %** | ~0.8–1 ms (8 distinct over 7.1 ms) |

  Writes nobody has looked at share a coarse ~1 ms (jiffy, `CONFIG_HZ=1000`) timestamp. Once a file's timestamp has been *queried*, the next change gets a fine-grained one. This is the kernel's **multigrain timestamps** (merged around Linux 6.13 for btrfs, ext4, xfs and tmpfs). It is what makes a stat fast path reliable **here**:
  - On older kernels, macOS (not measured) or other filesystems, the fingerprint must also compare ctime, size and inode, and treat a file whose mtime falls within one tick of the snapshot as "racily clean" and re-hash it (git's own rule).
  - Coarse mtimes can lag the wall clock by up to a tick, so never compare a file's mtime with the hook's wall-clock stamp to decide order.
- **ext4 (not measured, from documentation):** inodes of 256 bytes (the mkfs default) store nanosecond timestamps, with multigrain on recent kernels. Old 128-byte-inode filesystems have **1 s** granularity, so the stat fast path would miss same-second, same-size edits and would need racy-clean re-hashing.
- **Inodes:** `sed -i` writes a temp file and renames it (new inode). `>` truncates in place (same inode, new mtime/ctime). Both change the fingerprint (measured).
- **Edit then revert in one command (measured):** mtime changes, the content hash is equal, so the net change is none, which is the correct answer for line origins (intermediate states never reach the file). The cost side is the "touched" row: the file stays stat-dirty to git until someone refreshes the index.
- **Two sessions in one worktree, and parallel tool calls within one session:** a snapshot diff can't tell which of two overlapping windows wrote a file. Every file that changed inside the union of overlapping windows must be `Unknown(ConcurrentWriter)`. WHY can see the overlap only for writers it observes (both sessions are hooked). **Unobserved writers are invisible:** a human's editor, `cargo watch`, a dev server's codegen. Their writes inside a window would be attributed to the command, a known false attribution source that §C's watcher check can only narrow, not close. So shell-captured lines deserve a weaker origin (a "shell snapshot" lead) than direct `Edit`/`Write` evidence, and they belong in the WHY-26 benchmark as their own row.
- **Long commands (minutes):** the window is the command's duration, so the chance of an overlapping writer grows linearly with it (not measured; this follows from the design). Writes into ignored paths (`target/`) never show up. **Background processes the command starts** (`npm run dev &`) keep writing after Post and would be charged to whichever tool window is open next: another reason a shell-snapshot origin is a lead, not proof.
- **Not measured:** the two-writers and long-command cases as runs (the reasoning above follows from the design); macOS/APFS; a true cold cache; the real harness gap; the handshake variant (the hook waiting for an "app took its snapshot" ack). The handshake's cost is derivable: snapshot time plus about 0.1 ms of socket round-trip, i.e. the same as option A but in the app, with the same budget problem.

## 4. Safety

| rule | A (hook runs git) | B / C (app) | D (hook lstat only) |
|---|---|---|---|
| `GIT_OPTIONAL_LOCKS=0`, no `index.lock`, `core.fsmonitor=false` | must be re-implemented in the hook | yes, via `GitRunner` | no git |
| filter drivers not run on working-tree reads | **no**, unless the app passes trust state (contract change) | yes, `read_status` needs a `ReadPermit` | no git, no content reads |
| hashing without filters | Rust or `hash-object --no-filters` | Rust (or `--no-filters`) | none |
| fail-open (exit 0, no stdout, short deadline) | needs a kill-on-deadline for git; a slow repo eats the budget | not on the agent's path | trivially (bounded `lstat`) |

Only B/C (with D as an optional add-on) can respect every rule without the hook running repository-controlled code.

## Recommendation

**Defer shell-write capture to v0.3.1. Do not build it in v0.3.**

1. **Option A (snapshot in the hook) is rejected.**
   - Safety: `git status` executes repository-controlled filter drivers, and the hook has no filter guard. Fixing that means a `why-wire` contract change.
   - Latency: 9–20 ms per Bash call on 10k files, and 55–110 ms (p95 at `SEND_BUDGET`) on 50k, against today's 0.5 ms and the "few ms" target.
2. **Naive option B (status when Pre arrives) loses the race** for `sed -i` and `>` unless the harness waits ≥ 20 ms (10k) or more (50k), and that gap is unmeasured (bounded above by about 90 ms from fixtures).
3. **The v0.3.1 design is C, optionally with D:**
   - app-side snapshots chained Post→Pre, behind the existing `ReadPermit`, with Rust hashing;
   - Pre-time verification via `lstat`;
   - optionally the hook's stat fingerprint, so a lost race becomes Unknown instead of a wrong attribution.
   - **Budget:** the hook adds **≤ 1 ms p99** (D: `lstat` of ≤ 500 cached paths, no git, no reads), or nothing (C alone). The app's async snapshot target is ≤ 100 ms p95 on 50k files, which is met warm per the table above.
   - **Results:**
     - shell writes become a separate, weaker origin kind (lead confidence);
     - overlapping windows become `Unknown(ConcurrentWriter)`;
     - writes while WHY is closed stay Unknown.
4. **Why not v0.3:** C needs watcher integration in the collector path, a new origin kind and its UI meaning, and (for D) an envelope and contract change. It also has an inherent false-attribution source (unobserved concurrent writers, background processes) that v0.3's "zero known false attribution" target can't absorb. v0.3 ships direct edits (decision 31), and shell writes stay leads as in v0.2.

**Open before building:**
- measure the real harness gap (hook exit → command start) with a sandboxed agent run;
- run C's Post→Pre chain against a recorded session;
- measure on macOS/APFS.
