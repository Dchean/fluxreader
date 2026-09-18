//! Tauri IPC 命令面：前端 store 经 invoke 调用这里。
//! 每个命令短小：拿锁 → db:: 类型化函数 → 返回 Serialize 行类型。
//!
//! TASK-044：按既有章节边界拆为领域子模块（纯搬运，不改行为）；
//! 子模块经 `pub use` 重导出，使 `crate::commands::<fn>` 路径保持不变，
//! 故 lib.rs 的 invoke_handler 与 tests/ 的既有引用均无需改动。

use crate::db;
use crate::state::AppState;
use std::sync::atomic::{AtomicBool, Ordering};

/* ============================================================
模块根共享项
以下两项被子模块共用，故保留在模块根而非下沉到某个领域模块。
============================================================ */

/// 读 app_settings JSON 里的 smartDedup 开关（默认关：保持既有抓取行为）。
pub(crate) fn read_dedup_flag(conn: &rusqlite::Connection) -> bool {
    db::get_setting(conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("smartDedup").and_then(|f| f.as_bool()))
        .unwrap_or(false)
}

/* ============================================================
即时状态推送调度（防抖合批）
set_read/set_starred/mark_all_read 入队后调 schedule_state_push：
- AtomicBool 防重入：已有一个推送任务在飞时只标记"再来一轮"
- 800ms 防抖：快速滚动批量标读只发一次 PUT
- 失败静默（队列保留）→ 下次变更或下轮同步自动重推
============================================================ */

static STATE_PUSH_FLYING: AtomicBool = AtomicBool::new(false);
static STATE_PUSH_PENDING: AtomicBool = AtomicBool::new(false);
const STATE_PUSH_DEBOUNCE_MS: u64 = 800;

pub(crate) fn schedule_state_push(state: &AppState) {
    STATE_PUSH_PENDING.store(true, Ordering::SeqCst);
    if STATE_PUSH_FLYING.swap(true, Ordering::SeqCst) {
        return; // 已有任务在飞：它收尾时会看到 PENDING 再跑一轮
    }
    let db = state.db.clone();
    let http = state.http.clone();
    tauri::async_runtime::spawn(async move {
        // Drop 守卫：循环任何出口（含 push_states_now 内部未来可能出现的
        // panic 展开）都复位 FLYING——否则一次 panic 后推送永久静默
        struct FlyingGuard;
        impl Drop for FlyingGuard {
            fn drop(&mut self) {
                STATE_PUSH_FLYING.store(false, Ordering::SeqCst);
            }
        }
        let _guard = FlyingGuard;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(STATE_PUSH_DEBOUNCE_MS)).await;
            if !STATE_PUSH_PENDING.swap(false, Ordering::SeqCst) {
                break;
            }
            crate::sync::push_states_now(&db, &http).await;
            // push 期间又入队 → 继续循环；否则退出并放行下一个调度
            if !STATE_PUSH_PENDING.load(Ordering::SeqCst) {
                break;
            }
        }
    });
}

/// 后端同步凭据是否已配置（folders/articles/sync 三处共用，故留在模块根）。
pub(crate) fn sync_configured(conn: &rusqlite::Connection) -> bool {
    crate::sync::read_credentials(conn).is_some()
}

/* ============================================================
领域子模块声明与重导出
============================================================ */
mod ai;
mod articles;
mod folders;
mod opml;
mod settings;
mod sync;

pub use ai::*;
pub use articles::*;
pub use folders::*;
pub use opml::*;
pub use settings::*;
pub use sync::*;
