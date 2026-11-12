//! Tauri IPC 命令面：前端 store 经 invoke 调用这里。
//! 每个命令短小：拿锁 → db:: 类型化函数 → 返回 Serialize 行类型。
//!
//! TASK-044：按既有章节边界拆为领域子模块（纯搬运，不改行为）；
//! 子模块经 `pub use` 重导出，使 `crate::commands::<fn>` 路径保持不变，
//! 故 lib.rs 的 invoke_handler 与 tests/ 的既有引用均无需改动。

use crate::db;
use crate::error::AppResult;
use crate::state::AppState;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

/* ============================================================
模块根共享项
以下两项被子模块共用，故保留在模块根而非下沉到某个领域模块。
============================================================ */

/// 读 app_settings JSON 里的 smartDedup 开关（默认关：保持既有抓取行为）。
pub(crate) fn read_dedup_flag(conn: &rusqlite::Connection) -> bool {
    /* TASK-068：收口为类型化助手（默认值语义不变） */
    db::app_settings_bool(conn, "smartDedup", false)
}

/* ============================================================
即时状态推送调度（防抖合批）
set_read/set_starred/mark_all_read 入队后调 schedule_state_push：
- 每库有界邮箱与单消费者，空闲等待仍可收到下一操作
- 800ms 防抖：快速滚动批量标读只发一次 PUT
- 失败保留队列，5秒至5分钟指数退避重试；服务端Retry-After另设请求冷却
============================================================ */

type StatePushRegistry = std::collections::HashMap<
    usize,
    (
        std::sync::Weak<tokio::sync::Mutex<rusqlite::Connection>>,
        tokio::sync::mpsc::Sender<()>,
    ),
>;
static STATE_PUSH_WORKERS: OnceLock<StdMutex<StatePushRegistry>> = OnceLock::new();
const STATE_PUSH_DEBOUNCE_MS: u64 = 800;

struct StatePushRegistration {
    key: usize,
    sender: tokio::sync::mpsc::Sender<()>,
}

impl Drop for StatePushRegistration {
    fn drop(&mut self) {
        if let Some(registry) = STATE_PUSH_WORKERS.get() {
            let mut workers = registry.lock().unwrap_or_else(|e| e.into_inner());
            // tokio 1.54.1 Sender::same_channel distinguishes a replacement worker.
            if workers
                .get(&self.key)
                .is_some_and(|(_, tx)| tx.same_channel(&self.sender))
            {
                workers.remove(&self.key);
            }
        }
    }
}

/// One persistent bounded mailbox per database. The receiver remains alive while idle,
/// so there is no "decided to exit / clear flying" handoff window.
#[doc(hidden)]
pub fn schedule_state_push_for_db(
    database: &Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    http: &reqwest::Client,
) {
    let key = Arc::as_ptr(database) as usize;
    let mut workers = STATE_PUSH_WORKERS
        .get_or_init(|| StdMutex::new(Default::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    workers.retain(|_, (db, tx)| db.strong_count() > 0 && !tx.is_closed());
    if let Some((_, sender)) = workers.get(&key) {
        let _ = sender.try_send(());
        return;
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let db = Arc::downgrade(database);
    let runtime = crate::sync::runtime::for_db(database);
    let http = http.clone();
    workers.insert(key, (db.clone(), sender.clone()));
    let _ = sender.try_send(());
    tauri::async_runtime::spawn(async move {
        let _registration = StatePushRegistration { key, sender };
        let mut retry_delay = std::time::Duration::from_secs(5);
        let mut retry = false;
        loop {
            if retry {
                tokio::select! {
                    _=runtime.wait_for_stop()=>return,
                    message=receiver.recv()=>if message.is_none(){return;},
                    _=tokio::time::sleep(retry_delay)=>{},
                }
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(300));
            } else {
                tokio::select! {
                    _=runtime.wait_for_stop()=>return,
                    message=receiver.recv()=>if message.is_none(){return;},
                    _=tokio::time::sleep(std::time::Duration::from_secs(30))=>{
                        if db.strong_count()==0{return;} else {continue;}
                    },
                }
            }
            tokio::select! {
                _=runtime.wait_for_stop()=>return,
                _=tokio::time::sleep(std::time::Duration::from_millis(STATE_PUSH_DEBOUNCE_MS))=>{},
            }
            while receiver.try_recv().is_ok() {}
            let Some(database) = db.upgrade() else {
                return;
            };
            crate::sync::push_states_now(&database, &http).await;
            retry = crate::state::database_work(database.clone(), |conn| {
                Ok(
                    match (
                        crate::sync::current_account(conn),
                        db::sync_queue_stats(conn),
                    ) {
                        (Ok(Some(_)), Ok(stats)) => stats.waiting > 0,
                        _ => false,
                    },
                )
            })
            .await
            .unwrap_or(false);
            if !retry {
                retry_delay = std::time::Duration::from_secs(5);
            }
        }
    });
}

pub(crate) fn schedule_state_push(state: &AppState) {
    state.local_sync.notify_local_commit();
    schedule_state_push_for_db(&state.db, &state.http);
}

/// 后端同步凭据是否已配置（folders/articles/sync 三处共用，故留在模块根）。
/// OPT-014 R1：读取/解密失败 → Err——不得吞成「未配置」（那会让入队/退订等
/// 决策把不可用凭据当离线，错误被静默）。
pub(crate) fn sync_configured(conn: &rusqlite::Connection) -> AppResult<bool> {
    Ok(crate::sync::read_credentials(conn)?.is_some())
}

/// 订阅写回协议门（sync_local_feeds 与 add_feed 共用）：Fever 没有订阅写回端点。
pub(crate) fn subscription_write_back_supported(protocol: &str) -> bool {
    protocol == "greader"
}

/* ============================================================
领域子模块声明与重导出
============================================================ */
mod ai;
mod articles;
mod backup;
mod folders;
mod opml;
mod settings;
mod sync;

pub use ai::*;
pub use articles::*;
pub use backup::*;
pub use folders::*;
pub use opml::*;
pub use settings::*;
pub use sync::*;
