//! 应用状态：SQLite 连接（异步互斥守卫）+ 共享 HTTP client + SMTC 句柄。
//! 锁从不跨 .await 持有。

use crate::github_auth::SharedDeviceFlow;
use crate::media::MediaHandle;
use reqwest::Client;
use rusqlite::Connection;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    /// 单写连接：所有变更与后台刷新独占持有；访问短且同步。
    /// Arc 包一层：后台调度器需要 clone 出引用，与 tauri::State 并存。
    pub db: Arc<Mutex<Connection>>,
    /// 共享 HTTP client（连接池）；clone 廉价
    pub http: Client,
    /// SMTC 系统媒体控制投递端（专用线程持有 MediaControls）
    pub media: MediaHandle,
    /// 进行中的 GitHub 设备流登录（发起 → 轮询完成期间持有；一次性短命态不落库）
    pub github_flow: SharedDeviceFlow,
}

impl AppState {
    pub fn new(db: Connection, http: Client, media: MediaHandle) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            http,
            media,
            github_flow: Arc::new(Mutex::new(None)),
        }
    }
}
