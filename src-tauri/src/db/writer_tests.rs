use super::*;
use rusqlite::params;
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, mpsc},
    time::Instant,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
struct TestDb(PathBuf);
impl TestDb {
    fn new() -> Self {
        let root = std::env::var_os("FLUXREADER_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "fluxreader-writer-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> PathBuf {
        self.0.join("main.db")
    }
    fn open(&self) -> Connection {
        open(&self.path())
    }
}
impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("all test connections must be closed");
    }
}
fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS writes(seq INTEGER PRIMARY KEY,actor TEXT NOT NULL);",
    )
    .unwrap();
    conn
}
fn wait_for_queue(gate: &Gate, count: usize) {
    let start = Instant::now();
    loop {
        if lock(&gate.state).queue.len() == count {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "writer did not reach queue"
        );
        std::thread::yield_now();
    }
}
fn actors(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT actor FROM writes ORDER BY seq")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

struct RecoveryWorker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl RecoveryWorker {
    fn finish(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}
impl Drop for RecoveryWorker {
    fn drop(&mut self) {
        self.finish();
    }
}

#[test]
fn writer_fifo_two_connections_prevents_recovery_reacquiring_ahead_of_foreground() {
    let dir = TestDb::new();
    let foreground = dir.open();
    let recovery = dir.open();
    let gate = gate(&identity(&foreground).unwrap()).unwrap();
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let recovery_thread = std::thread::spawn(move || {
        let tx = write_transaction(&recovery).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('recovery-first')", [])
            .unwrap();
        held_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        tx.commit().unwrap();
        for _ in 0..32 {
            let tx = write_transaction(&recovery).unwrap();
            tx.execute("INSERT INTO writes(actor) VALUES('recovery-next')", [])
                .unwrap();
            tx.commit().unwrap();
        }
    });
    held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let foreground_thread = std::thread::spawn(move || {
        let tx = write_transaction(&foreground).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('foreground')", [])
            .unwrap();
        tx.commit().unwrap();
        foreground
    });
    wait_for_queue(&gate, 1);
    release_tx.send(()).unwrap();
    let foreground = foreground_thread.join().unwrap();
    recovery_thread.join().unwrap();
    let order = actors(&foreground);
    assert_eq!(&order[..2], &["recovery-first", "foreground"]);
    assert_eq!(order.len(), 34, "every write survives; no no-op sample");
}

#[test]
fn writer_implicit_setting_joins_same_queue_as_business_transactions() {
    let dir = TestDb::new();
    let first = dir.open();
    first
        .execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT)")
        .unwrap();
    let second = dir.open();
    let tx = write_transaction(&first).unwrap();
    let gate = gate(&identity(&first).unwrap()).unwrap();
    let thread = std::thread::spawn(move || {
        crate::db::set_setting(&second, "foreground", "kept").unwrap();
        second
    });
    wait_for_queue(&gate, 1);
    tx.commit().unwrap();
    let second = thread.join().unwrap();
    assert_eq!(
        crate::db::get_setting(&second, "foreground")
            .unwrap()
            .as_deref(),
        Some("kept")
    );
}

#[test]
fn writer_rollback_and_commit_failure_release_only_after_sqlite_is_closed() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    first.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED);").unwrap();
    {
        let tx = write_transaction(&first).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('rolled-back')", [])
            .unwrap();
    }
    assert!(first.is_autocommit());
    assert!(actors(&second).is_empty());
    let tx = write_transaction(&first).unwrap();
    tx.execute("INSERT INTO child(parent) VALUES(404)", [])
        .unwrap();
    assert!(tx.commit().is_err(), "deferred FK must fail COMMIT");
    assert!(
        first.is_autocommit(),
        "failed commit must roll back before permit release"
    );
    let tx = write_transaction(&second).unwrap();
    tx.execute("INSERT INTO writes(actor) VALUES('after-error')", [])
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(actors(&first), ["after-error"]);
    assert_eq!(
        first
            .query_row("SELECT COUNT(*) FROM child", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn writer_unwind_rolls_back_before_allowing_the_next_connection() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let tx = write_transaction(&first).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('panic')", [])
            .unwrap();
        panic!("injected writer unwind");
    }));
    assert!(result.is_err());
    assert!(first.is_autocommit());
    let tx = write_transaction(&second).unwrap();
    tx.execute("INSERT INTO writes(actor) VALUES('after-unwind')", [])
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(actors(&first), ["after-unwind"]);
}

#[test]
fn writer_wal_read_snapshot_does_not_hold_admission_or_see_partial_writes() {
    let dir = TestDb::new();
    let reader = dir.open();
    let writer = dir.open();
    let snapshot = reader.unchecked_transaction().unwrap();
    assert!(actors(&snapshot).is_empty());
    let tx = write_transaction(&writer).unwrap();
    tx.execute("INSERT INTO writes(actor) VALUES('committed')", [])
        .unwrap();
    tx.commit().unwrap();
    assert!(
        actors(&snapshot).is_empty(),
        "reader keeps its original snapshot"
    );
    snapshot.commit().unwrap();
    assert_eq!(actors(&reader), ["committed"]);
}

#[test]
fn writer_nested_savepoint_keeps_outer_atomicity_and_rejects_nested_begin() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    let tx = write_transaction(&first).unwrap();
    tx.execute("INSERT INTO writes(actor) VALUES('outer')", [])
        .unwrap();
    let nested = crate::db::contents::with_write_savepoint(&tx, "writer_nested", || {
        let _guard = writer_guard(&tx)?;
        tx.execute("INSERT INTO writes(actor) VALUES('inner')", [])?;
        Err::<(), _>(AppError::new("injected", "rollback only inner"))
    });
    assert_eq!(nested.unwrap_err().code, "injected");
    assert!(write_transaction(&first).is_err());
    assert!(
        actors(&second).is_empty(),
        "WAL reader cannot see uncommitted outer writes"
    );
    tx.commit().unwrap();
    assert_eq!(actors(&second), ["outer"]);
}

#[test]
fn writer_nested_savepoint_panic_rolls_back_inner_and_keeps_outer_usable() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    let tx = write_transaction(&first).unwrap();
    tx.execute("INSERT INTO writes(actor) VALUES('outer')", [])
        .unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::db::contents::with_write_savepoint(&tx, "writer_panic", || -> AppResult<()> {
            tx.execute("INSERT INTO writes(actor) VALUES('inner-panic')", [])?;
            panic!("injected nested savepoint panic");
        })
    }));
    assert!(panicked.is_err());
    assert_eq!(
        actors(&tx),
        ["outer"],
        "RAII ROLLBACK TO must remove only the inner partial write during unwind"
    );
    assert!(
        !first.is_autocommit(),
        "outer transaction must remain usable"
    );
    tx.execute("INSERT INTO writes(actor) VALUES('after-panic')", [])
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(actors(&second), ["outer", "after-panic"]);
}

#[test]
fn writer_cancelled_waiter_withdraws_ticket_without_losing_later_writes() {
    let dir = TestDb::new();
    let first = dir.open();
    let cancelled_connection = dir.open();
    let later = dir.open();
    let tx = write_transaction(&first).unwrap();
    let gate = gate(&identity(&first).unwrap()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let cancelled = std::thread::spawn(move || {
        with_write_cancellation(worker_stop, || {
            write_transaction(&cancelled_connection)
                .err()
                .expect("wait must cancel")
                .code
        })
    });
    wait_for_queue(&gate, 1);
    let later = std::thread::spawn(move || {
        let tx = write_transaction(&later).unwrap();
        tx.execute("INSERT INTO writes(actor) VALUES('later')", [])
            .unwrap();
        tx.commit().unwrap();
    });
    wait_for_queue(&gate, 2);
    stop.store(true, Ordering::Release);
    assert_eq!(cancelled.join().unwrap(), "cancelled");
    wait_for_queue(&gate, 1);
    tx.commit().unwrap();
    later.join().unwrap();
    assert_eq!(actors(&first), ["later"]);
    assert!(lock(&gate.state).queue.is_empty());
}

#[test]
fn writer_stop_during_owned_transaction_does_not_discard_intent_or_reenter_begin() {
    let dir = TestDb::new();
    let conn = dir.open();
    conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT)")
        .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    with_write_cancellation(stop.clone(), || {
        let tx = write_transaction(&conn).unwrap();
        stop.store(true, Ordering::Release);
        crate::db::set_setting(&tx, "intent", "durable").unwrap();
        tx.commit().unwrap();
        assert_eq!(write_transaction(&conn).err().unwrap().code, "cancelled");
    });
    assert_eq!(
        crate::db::get_setting(&conn, "intent").unwrap().as_deref(),
        Some("durable")
    );
    write_transaction(&conn).unwrap().commit().unwrap();
}

#[test]
fn writer_try_admission_preserves_busy_policy_and_timeout() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    second.busy_timeout(Duration::from_millis(4321)).unwrap();
    let tx = write_transaction(&first).unwrap();
    let started = Instant::now();
    let error = try_write_transaction(&second).err().unwrap();
    assert_eq!(error.code, "db");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        second
            .pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
            .unwrap(),
        4321
    );
    tx.commit().unwrap();
    try_write_transaction(&second).unwrap().commit().unwrap();
}

#[test]
fn writer_different_databases_and_private_memory_do_not_block_each_other() {
    let a = TestDb::new();
    let b = TestDb::new();
    let first = a.open();
    let second = b.open();
    let memory_a = Connection::open_in_memory().unwrap();
    let memory_b = Connection::open_in_memory().unwrap();
    let temp = Connection::open("").unwrap();
    let one = write_transaction(&first).unwrap();
    let two = write_transaction(&second).unwrap();
    let three = write_transaction(&memory_a).unwrap();
    let four = write_transaction(&memory_b).unwrap();
    let five = write_transaction(&temp).unwrap();
    five.commit().unwrap();
    four.commit().unwrap();
    three.commit().unwrap();
    two.commit().unwrap();
    one.commit().unwrap();
    assert_ne!(identity(&first).unwrap(), identity(&second).unwrap());
    assert_ne!(identity(&memory_a).unwrap(), identity(&memory_b).unwrap());
}

#[test]
fn writer_same_database_cross_connection_reentry_is_error_not_self_deadlock() {
    let dir = TestDb::new();
    let first = dir.open();
    let second = dir.open();
    let tx = write_transaction(&first).unwrap();
    assert_eq!(writer_guard(&second).err().unwrap().code, "dbWriterReentry");
    tx.commit().unwrap();
    write_transaction(&second).unwrap().commit().unwrap();
}

#[test]
fn writer_private_gate_registry_does_not_retain_closed_connection_history() {
    let conn = Connection::open_in_memory().unwrap();
    let key = identity(&conn).unwrap();
    {
        let permit = writer_guard(&conn).unwrap();
        assert!(lock(&REGISTRY).gates.get(&key).unwrap().upgrade().is_some());
        drop(permit);
    }
    let other = Connection::open_in_memory().unwrap();
    let _other = writer_guard(&other).unwrap();
    assert!(!lock(&REGISTRY).gates.contains_key(&key));
}

#[test]
fn writer_failed_file_stays_failed_closed_after_weak_gate_cleanup() {
    let dir = TestDb::new();
    let conn = dir.open();
    let key = identity(&conn).unwrap();
    let original = gate(&key).unwrap();
    // This tests the recovery-state lifetime. The real commit-failure test
    // separately exercises SQLite rollback; no synthetic I/O failure is claimed.
    fail_closed(&key, &original);
    drop(original);
    assert_eq!(gate(&key).err().unwrap().code, "dbWriterRecovery");
    assert!(!lock(&REGISTRY).gates.contains_key(&key));
    let second = dir.open();
    assert_eq!(
        write_transaction(&second).err().unwrap().code,
        "dbWriterRecovery"
    );
    // Only this isolated test may clear its synthetic fault. Product recovery
    // requires process restart, so a new weak gate cannot erase the poison.
    lock(&REGISTRY).failed.remove(&key);
}

#[test]
fn writer_real_read_star_and_bulk_survive_repeated_background_slices_and_reopen() {
    let dir = TestDb::new();
    let conn = crate::db::open(&dir.path()).unwrap();
    let feed = crate::db::create_folder(&conn, "writer", "article").unwrap();
    let source = crate::db::insert_feed(
        &conn,
        "https://writer.invalid/rss",
        None,
        "writer",
        None,
        feed,
        "inherit",
        false,
        false,
    )
    .unwrap();
    {
        let tx = write_transaction(&conn).unwrap();
        for id in 1..=24 {
            tx.execute(
                "INSERT INTO articles(id,feed_id,guid,title) VALUES(?1,?2,?3,?3)",
                params![id, source, format!("writer-{id}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    crate::local_sync::database::initialize(&conn, &"a".repeat(32)).unwrap();
    let background = crate::db::open(&dir.path()).unwrap();
    conn.busy_timeout(Duration::ZERO).unwrap();
    background.busy_timeout(Duration::ZERO).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let background_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        while !background_stop.load(Ordering::Acquire) {
            crate::local_sync::database::resume_local_slice(&background).unwrap();
            std::thread::yield_now();
        }
    });
    let mut worker = RecoveryWorker {
        stop,
        thread: Some(thread),
    };
    for id in 1..=20 {
        assert_eq!(
            crate::db::set_read_with_enqueue(&conn, id, true)
                .unwrap()
                .len(),
            1
        );
        crate::db::set_starred_with_enqueue(&conn, id, true).unwrap();
    }
    assert_eq!(
        crate::db::set_read_bulk_with_enqueue(&conn, &[21, 22, 23], true)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        crate::db::mark_all_read_with_enqueue(&conn, Some(source), None, false, None, None)
            .unwrap(),
        1
    );
    worker.finish();
    drop(conn);
    let reopened = crate::db::open(&dir.path()).unwrap();
    crate::local_sync::logical::drain_local(&reopened).unwrap();
    let (read, stars): (i64, i64) = reopened
        .query_row(
            "SELECT SUM(is_read),SUM(is_starred) FROM articles",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((read, stars), (24, 20));
}

#[test]
fn writer_new_file_reopen_and_path_aliases_resolve_actual_file_identity() {
    let dir = TestDb::new();
    let first = dir.open();
    let original = identity(&first).unwrap();
    let same = Connection::open(dir.path()).unwrap();
    assert_eq!(identity(&same).unwrap(), original);
    let alias = dir.0.join("hardlink.db");
    std::fs::hard_link(dir.path(), &alias).unwrap();
    let linked = Connection::open(&alias).unwrap();
    assert_eq!(identity(&linked).unwrap(), original);
    // Do not write through hardlink aliases: SQLite's WAL name itself is path
    // based. This probe verifies admission identity, not SQLite alias safety.
    #[cfg(windows)]
    {
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let extended = Connection::open(canonical).unwrap();
        assert_eq!(identity(&extended).unwrap(), original);
        let upper = Connection::open(dir.path().to_string_lossy().to_uppercase()).unwrap();
        assert_eq!(identity(&upper).unwrap(), original);
    }
    drop(linked);
    drop(same);
    drop(first);
    let reopened = dir.open();
    assert_eq!(identity(&reopened).unwrap(), original);
    write_transaction(&reopened).unwrap().commit().unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "requires Windows symlink permission; run in the reserved identity probe"]
fn writer_windows_symlink_alias_resolves_actual_file_identity() {
    let dir = TestDb::new();
    let first = dir.open();
    let alias = dir.0.join("symlink.db");
    std::os::windows::fs::symlink_file(dir.path(), &alias).unwrap();
    let second = Connection::open(alias).unwrap();
    assert_eq!(identity(&first).unwrap(), identity(&second).unwrap());
}

#[test]
fn writer_external_sqlite_busy_still_propagates_without_stranding_the_queue() {
    let dir = TestDb::new();
    let external = dir.open();
    let conn = dir.open();
    let external_tx =
        Transaction::new_unchecked(&external, TransactionBehavior::Immediate).unwrap();
    assert_eq!(write_transaction(&conn).err().unwrap().code, "db");
    external_tx.commit().unwrap();
    let tx = write_transaction(&conn).unwrap();
    tx.execute(
        "INSERT INTO writes(actor) VALUES(?1)",
        params!["after-external"],
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(actors(&conn), ["after-external"]);
}
