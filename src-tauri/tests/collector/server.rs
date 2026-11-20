//! The socket server against hostile and crowded peers.

use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use why_lib::collector::{CollectorStatus, spawn_core};
use why_wire::{MAX_LINE_BYTES, encode_line};

use super::*;

async fn send(rig: &Rig, bytes: &[u8]) {
    let mut s = UnixStream::connect(rig.socket()).await.unwrap();
    s.write_all(bytes).await.unwrap();
    // Drop closes the connection: the server sees our bytes, then EOF.
}

fn line(rig: &Rig, session: &str) -> Vec<u8> {
    encode_line(&envelope(session, &rig.repo.path, 1)).unwrap()
}

#[tokio::test]
async fn a_valid_line_is_stored_and_attributed_to_the_worktree() {
    let rig = Rig::new();
    let core = rig.start().await;
    send(&rig, &line(&rig, "s1")).await;
    eventually(Duration::from_secs(5), "event stored", || {
        rig.has_session("s1")
    })
    .await;
    assert_eq!(rig.sessions(), 1, "resolved to the registered worktree");
    assert_eq!(core.stats().received, 1);
}

#[tokio::test]
async fn bad_lines_are_counted_and_dropped_without_killing_the_connection() {
    let rig = Rig::new();
    let core = rig.start().await;
    let mut unknown_v = envelope("sv", &rig.repo.path, 1);
    unknown_v.v = 99;
    let mut v2 = serde_json::to_vec(&unknown_v).unwrap();
    v2.push(b'\n');

    let mut batch: Vec<u8> = Vec::new();
    batch.extend(b"this is not json\n");
    batch.extend(b"{\"v\":1}\n"); // missing fields
    batch.extend(b"\xff\xfe\xfd\n"); // not UTF-8
    batch.extend(&v2); // future protocol version
    batch.extend(line(&rig, "good")); // ...and then a good one on the same connection
    send(&rig, &batch).await;

    eventually(
        Duration::from_secs(5),
        "good line after the bad ones",
        || rig.has_session("good"),
    )
    .await;
    let s = core.stats();
    assert_eq!((s.malformed, s.unsupported_version, s.received), (3, 1, 1));
    assert!(!rig.has_session("sv"));
}

#[tokio::test]
async fn an_oversized_line_is_dropped_and_its_connection_closed() {
    let rig = Rig::new();
    let core = rig.start().await;
    let mut s = UnixStream::connect(rig.socket()).await.unwrap();
    // 3 MiB with no newline: the server must give up at 1 MiB, not buffer it.
    let junk = vec![b'x'; 3 * MAX_LINE_BYTES];
    let _ = s.write_all(&junk).await; // may fail once the server closes: fine
    let mut rest = Vec::new();
    let eof = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest)).await;
    assert!(eof.is_ok(), "the server must close the connection");
    eventually(Duration::from_secs(5), "oversized counted", || {
        core.stats().oversized == 1
    })
    .await;
    // The server is still fine afterwards.
    send(&rig, &line(&rig, "after")).await;
    eventually(Duration::from_secs(5), "later event", || {
        rig.has_session("after")
    })
    .await;
}

#[tokio::test]
async fn a_partial_line_is_never_decoded() {
    let rig = Rig::new();
    let core = rig.start().await;
    let full = line(&rig, "torn");
    send(&rig, &full[..full.len() - 1]).await; // all but the newline
    eventually(Duration::from_secs(5), "unterminated counted", || {
        core.stats().unterminated == 1
    })
    .await;
    assert!(!rig.has_session("torn"));
}

#[tokio::test]
async fn a_client_that_sends_nothing_is_dropped_after_the_idle_timeout() {
    let rig = Rig::new();
    let core = rig.start().await; // idle_timeout is 300 ms in tests
    let mut s = UnixStream::connect(rig.socket()).await.unwrap();
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await;
    assert_eq!(
        read.unwrap().unwrap(),
        0,
        "server closed the idle connection"
    );
    assert_eq!(core.stats().timeouts, 1);
}

#[tokio::test]
async fn a_client_dribbling_bytes_is_cut_off_by_the_overall_deadline() {
    let rig = Rig::new();
    let core = rig.start().await; // deadline 3 s, idle 300 ms in tests
    let mut s = UnixStream::connect(rig.socket()).await.unwrap();
    let started = Instant::now();
    // One byte every 100 ms resets the idle timer forever; the deadline must win.
    loop {
        if s.write_all(b"x").await.is_err() || started.elapsed() > Duration::from_secs(6) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(started.elapsed() < Duration::from_secs(6), "never cut off");
    eventually(Duration::from_secs(2), "timeout counted", || {
        core.stats().timeouts == 1
    })
    .await;
}

/// 1000 hook-like connections at once: every line is either received or
/// dropped on purpose and COUNTED; none vanishes, nothing panics, and what was
/// received is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thousand_rapid_connections_are_all_accounted_for() {
    let rig = Rig::new();
    let core = rig.start().await;
    let socket = rig.socket();
    let mut clients = Vec::new();
    for i in 0..1000 {
        let line = line(&rig, &format!("c{i}"));
        let socket = socket.clone();
        clients.push(tokio::spawn(async move {
            // The kernel refuses a connect while the listen backlog is full
            // (a hook would spool instead); here we retry so that every one of
            // the 1000 lines really reaches the server, and the counters can
            // be checked against exactly 1000.
            for _ in 0..500 {
                if let Ok(mut s) = UnixStream::connect(&socket).await {
                    let _ = s.write_all(&line).await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("could not connect");
        }));
    }
    for c in clients {
        c.await.unwrap();
    }
    eventually(Duration::from_secs(20), "everything settled", || {
        let s = core.stats();
        s.ingested + s.ingest_errors == s.received
            && s.received + s.dropped_backpressure + s.rejected_busy + s.timeouts >= 1000
            && s.received > 0
    })
    .await;
    let s = core.stats();
    assert_eq!(
        s.received + s.dropped_backpressure + s.rejected_busy,
        1000,
        "received + dropped + refused must equal sent: {s:?}"
    );
    assert_eq!(s.ingest_errors, 0);
    assert_eq!(
        rig.sessions() as u64,
        s.received,
        "what was received is stored"
    );
}

/// A full queue drops and counts, and the server stays responsive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backpressure_drops_instead_of_growing() {
    let rig = Rig::new();
    // 20 ms per event against a 1024-slot queue: 3000 quick lines overflow it.
    let core = rig
        .start_with(Arc::new(SlowNormalizer(Duration::from_millis(20))))
        .await;
    let mut batch = Vec::new();
    for i in 0..3000 {
        batch.extend(line(&rig, &format!("b{i}")));
    }
    send(&rig, &batch).await;
    eventually(Duration::from_secs(10), "all lines accounted", || {
        let s = core.stats();
        s.received + s.dropped_backpressure + s.malformed == 3000
    })
    .await;
    let s = core.stats();
    assert!(s.dropped_backpressure > 0, "the queue never filled: {s:?}");
    assert!(
        s.received <= 1100,
        "more than queue + in-flight was accepted: {s:?}"
    );
}

#[tokio::test]
async fn a_second_instance_on_the_same_socket_does_not_steal_it() {
    let rig = Rig::new();
    let first = rig.start().await;
    let second = spawn_core(rig.config(Arc::new(TestNormalizer)));
    let status = second.ready().await;
    assert!(
        matches!(&status, CollectorStatus::Disabled(m) if m.contains("another process")),
        "{status:?}"
    );
    // The first still serves, and stopping the second must not unlink its socket.
    drop(second);
    assert!(rig.socket().exists());
    send(&rig, &line(&rig, "still-first")).await;
    eventually(Duration::from_secs(5), "first instance still works", || {
        rig.has_session("still-first")
    })
    .await;
    drop(first);
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.socket().parent().unwrap()).unwrap();
    // A crashed process leaves its socket file behind.
    drop(std::os::unix::net::UnixListener::bind(rig.socket()).unwrap());
    assert!(rig.socket().exists());
    // Other tests in this process fork child processes (git, the hook). A fork
    // briefly duplicates EVERY open descriptor, including the listener we just
    // dropped, so for a few milliseconds the "dead" socket can still answer.
    // The collector is right to call that live; the test waits it out.
    let mut core = spawn_core(rig.config(Arc::new(TestNormalizer)));
    for _ in 0..40 {
        if core.ready().await == CollectorStatus::Listening {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        core = spawn_core(rig.config(Arc::new(TestNormalizer)));
    }
    assert_eq!(core.ready().await, CollectorStatus::Listening);
    send(&rig, &line(&rig, "fresh")).await;
    eventually(Duration::from_secs(5), "served after reclaiming", || {
        rig.has_session("fresh")
    })
    .await;
    drop(core);
}

#[tokio::test]
async fn a_regular_file_at_the_socket_path_is_never_deleted() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.socket().parent().unwrap()).unwrap();
    std::fs::write(rig.socket(), b"precious").unwrap();
    let core = spawn_core(rig.config(Arc::new(TestNormalizer)));
    assert!(matches!(core.ready().await, CollectorStatus::Disabled(_)));
    assert_eq!(std::fs::read(rig.socket()).unwrap(), b"precious");
}

#[tokio::test]
async fn a_too_long_socket_path_disables_the_collector_with_a_clear_error() {
    let mut rig = Rig::new();
    let long = rig.tmp.path().join("d".repeat(120)).join("s.sock");
    rig.paths.socket_path = long;
    let core = spawn_core(rig.config(Arc::new(TestNormalizer)));
    match core.ready().await {
        CollectorStatus::Disabled(msg) => assert!(msg.contains("longer than"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_socket_is_private_and_removed_on_stop() {
    let rig = Rig::new();
    let core = rig.start().await;
    let meta = std::fs::metadata(rig.socket()).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600);
    let dir = std::fs::metadata(rig.socket().parent().unwrap()).unwrap();
    assert_eq!(
        dir.mode() & 0o777 & !0o700,
        0,
        "directory must not be group/other accessible"
    );
    core.stop();
    assert!(!rig.socket().exists(), "stop must remove the socket file");
}

#[tokio::test]
async fn stop_does_not_remove_a_socket_another_process_put_there() {
    let rig = Rig::new();
    let core = rig.start().await;
    // Another instance replaced the file (e.g. we were frozen, it declared us
    // stale): our stop must not unlink ITS socket.
    std::fs::remove_file(rig.socket()).unwrap();
    let theirs = std::os::unix::net::UnixListener::bind(rig.socket()).unwrap();
    core.stop();
    assert!(rig.socket().exists());
    drop(theirs);
}

#[tokio::test]
async fn connecting_after_stop_is_refused() {
    let rig = Rig::new();
    let core = rig.start().await;
    core.stop();
    let err = UnixStream::connect(rig.socket()).await.unwrap_err();
    assert!(matches!(
        err.kind(),
        ErrorKind::NotFound | ErrorKind::ConnectionRefused
    ));
}

/// Two collectors start at the same moment against a STALE socket (both would
/// find it dead): exactly one may own the socket, the other must stand down.
/// Without the per-instance lock the second one's `unlink` could delete the
/// first one's freshly bound socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_against_a_stale_socket_elect_exactly_one_owner() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.socket().parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(rig.socket()).unwrap());
    // (see `a_stale_socket_file_is_replaced`: wait out other tests' forks)
    let t = Instant::now();
    while std::os::unix::net::UnixStream::connect(rig.socket()).is_ok() {
        assert!(t.elapsed() < Duration::from_secs(5), "listener never died");
        std::thread::sleep(Duration::from_millis(5));
    }

    let cores: Vec<_> = (0..8)
        .map(|_| spawn_core(rig.config(Arc::new(TestNormalizer))))
        .collect();
    let mut statuses = Vec::new();
    for c in &cores {
        statuses.push(c.ready().await);
    }
    let owners = statuses
        .iter()
        .filter(|s| **s == CollectorStatus::Listening)
        .count();
    assert_eq!(owners, 1, "{statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|s| matches!(s, CollectorStatus::Listening | CollectorStatus::Disabled(_)))
    );
    // And the owner's socket really works.
    send(&rig, &line(&rig, "owner")).await;
    eventually(Duration::from_secs(5), "owner serves", || {
        rig.has_session("owner")
    })
    .await;
    drop(cores);
    assert!(!rig.socket().exists(), "the owner removed its socket");
}

/// After the owner stops, the lock is free again: a new collector starts.
#[tokio::test]
async fn the_lock_is_released_when_the_owner_stops() {
    let rig = Rig::new();
    let first = rig.start().await;
    first.stop();
    drop(first);
    // A `flock` is released when EVERY duplicate of the descriptor is closed.
    // Other tests in this process fork children (git, the hook), and a fork
    // briefly duplicates all descriptors, so allow those few milliseconds.
    let mut status = CollectorStatus::Starting;
    for _ in 0..40 {
        let again = spawn_core(rig.config(Arc::new(TestNormalizer)));
        status = again.ready().await;
        if status == CollectorStatus::Listening {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the lock was never released: {status:?}");
}

/// The queue is bounded by BYTES too: big envelopes fill the budget long before
/// the 1024-slot count limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ingest_queue_is_bounded_by_bytes_not_just_count() {
    let rig = Rig::new();
    let one = encode_line(&{
        let mut e = envelope("m0", &rig.repo.path, 1);
        e.payload["pad"] = serde_json::json!("x".repeat(50_000));
        e
    })
    .unwrap()
    .len();
    let mut cfg = rig.config(Arc::new(SlowNormalizer(Duration::from_millis(40))));
    cfg.server.max_queued_bytes = one * 5; // room for ~5 envelopes
    let core = spawn_core(cfg);
    assert_eq!(core.ready().await, CollectorStatus::Listening);

    let mut batch = Vec::new();
    for i in 0..200 {
        let mut e = envelope(&format!("m{i}"), &rig.repo.path, 1 + i as u64);
        e.payload["pad"] = serde_json::json!("x".repeat(50_000));
        batch.extend(encode_line(&e).unwrap());
    }
    send(&rig, &batch).await;
    eventually(Duration::from_secs(10), "all accounted", || {
        let s = core.stats();
        s.received + s.dropped_memory + s.dropped_backpressure == 200
    })
    .await;
    let s = core.stats();
    assert!(s.dropped_memory > 100, "the byte budget never bit: {s:?}");
    assert_eq!(
        s.dropped_backpressure, 0,
        "count limit must not be what stopped it: {s:?}"
    );
    assert!(s.received >= 5, "{s:?}");
    // Everything accepted is eventually stored, and the budget drains back to 0
    // (so later events are accepted again).
    eventually(Duration::from_secs(10), "accepted events stored", || {
        rig.sessions() as u64 == core.stats().received
    })
    .await;
    send(&rig, &line(&rig, "later")).await;
    eventually(Duration::from_secs(10), "budget released", || {
        rig.has_session("later")
    })
    .await;
}
