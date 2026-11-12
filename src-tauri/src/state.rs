//! 应用状态：SQLite 连接（异步互斥守卫）+ 共享 HTTP client + SMTC 句柄。
//! 锁从不跨 .await 持有。

use crate::media::MediaHandle;
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Async callers hand the entire synchronous DB scope to a blocking worker.
/// Connection mutex acquisition, FIFO writer wait and the atomic operation stay
/// on that worker; the result returns only after commit/rollback and guard drop.
/// HTTP and application events remain in the caller after this await.
pub(crate) async fn database_work<T: Send + 'static>(
    database: Arc<Mutex<Connection>>,
    work: impl FnOnce(&mut Connection) -> crate::error::AppResult<T> + Send + 'static,
) -> crate::error::AppResult<T> {
    tokio::task::spawn_blocking(move || {
        let mut conn = database.blocking_lock();
        work(&mut conn)
    })
    .await
    .map_err(|_| crate::error::AppError::internal("数据库工作线程未完成"))?
}

/// Maintenance is opportunistic: acquire the connection once without waiting.
/// The synchronous work may also use the core's nonblocking writer admission.
pub(crate) async fn try_database_work<T: Send + 'static>(
    database: Arc<Mutex<Connection>>,
    work: impl FnOnce(&mut Connection) -> crate::error::AppResult<T> + Send + 'static,
) -> crate::error::AppResult<Option<T>> {
    tokio::task::spawn_blocking(move || {
        let Ok(mut conn) = database.try_lock() else {
            return Ok(None);
        };
        work(&mut conn).map(Some)
    })
    .await
    .map_err(|_| crate::error::AppError::internal("数据库工作线程未完成"))?
}

pub struct AppState {
    /// 单写连接：所有变更与后台刷新独占持有；访问短且同步。
    /// Arc 包一层：后台调度器需要 clone 出引用，与 tauri::State 并存。
    pub db: Arc<Mutex<Connection>>,
    /// 共享 HTTP client（连接池）；clone 廉价
    pub http: Client,
    pub images: Arc<crate::image_requests::ImageRequests>,
    /// SMTC 系统媒体控制投递端（专用线程持有 MediaControls）
    pub media: MediaHandle,
    /// Independent state/body lanes; neither holds the DB lock during HTTP.
    pub local_sync: Arc<crate::local_sync::runtime::LocalRuntime>,
    pub sync_runtime: Arc<crate::sync::runtime::SyncRuntime>,
    pub cover_backfill: Arc<Mutex<crate::scheduler::CoverBackfillState>>,
}

impl AppState {
    pub fn new(db: Connection, http: Client, media: MediaHandle) -> Self {
        let db = Arc::new(Mutex::new(db));
        let sync_runtime = crate::sync::runtime::for_db(&db);
        Self {
            db,
            http,
            images: Arc::new(Default::default()),
            media,
            local_sync: Arc::new(Default::default()),
            sync_runtime,
            cover_backfill: Arc::new(Mutex::new(crate::scheduler::CoverBackfillState::new())),
        }
    }
}

#[cfg(test)]
mod database_work_tests {
    use super::*;
    use crate::error::AppError;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn database() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE intent(id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    async fn cancellation_keeps_started_transaction() {
        let db = database();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
        let write = tokio::spawn(database_work(db.clone(), move |conn| {
            let tx = crate::db::write_transaction(conn)?;
            tx.execute("INSERT INTO intent VALUES(1,'flag')", [])?;
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            tx.execute("INSERT INTO intent VALUES(2,'outbox')", [])?;
            tx.commit()?;
            committed_tx.send(()).unwrap();
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();

        // The SQL scope is deliberately blocked. Both a timer and another task
        // must make progress while the connection and writer permit are held.
        let heartbeat = tokio::spawn(async {
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            3
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), heartbeat)
                .await
                .unwrap()
                .unwrap(),
            3
        );
        write.abort();
        assert!(write.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), committed_rx)
            .await
            .unwrap()
            .unwrap();
        let values = database_work(db.clone(), |conn| {
            let mut stmt = conn.prepare("SELECT value FROM intent ORDER BY id")?;
            let values = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(values)
        })
        .await
        .unwrap();
        assert_eq!(values, ["flag", "outbox"]);
        database_work(db, |conn| {
            let tx = crate::db::write_transaction(conn)?;
            tx.execute("INSERT INTO intent VALUES(3,'later')", [])?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_does_not_cut_transaction_or_block_single_thread_heartbeat() {
        cancellation_keeps_started_transaction().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_does_not_cut_transaction_on_production_runtime() {
        cancellation_keeps_started_transaction().await;
    }

    #[tokio::test]
    async fn sql_failure_rolls_back_and_preserves_business_error_metadata() {
        let db = database();
        let error = database_work(db.clone(), |conn| {
            let tx = crate::db::write_transaction(conn)?;
            tx.execute("INSERT INTO intent VALUES(1,'must rollback')", [])?;
            tx.execute("INSERT INTO missing_table VALUES(1)", [])?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "db");
        assert!(error.message.contains("missing_table"));
        let count = database_work(db.clone(), |conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM intent", [], |row| {
                row.get::<_, i64>(0)
            })?)
        })
        .await
        .unwrap();
        assert_eq!(count, 0);
        let error = database_work::<()>(db, |_| {
            Err(AppError::new("customAuth", "keep exact classification").auth())
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "customAuth");
        assert_eq!(error.message, "keep exact classification");
        assert!(error.is_auth_failure());
    }

    #[tokio::test]
    async fn opportunistic_work_skips_held_connection_without_running_callback() {
        let db = database();
        let held = db.lock().await;
        let ran = Arc::new(AtomicBool::new(false));
        let observed = ran.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            try_database_work(db.clone(), move |_| {
                observed.store(true, Ordering::Release);
                Ok(7)
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, None);
        assert!(!ran.load(Ordering::Acquire));
        drop(held);
        assert_eq!(try_database_work(db, |_| Ok(7)).await.unwrap(), Some(7));
    }
}
