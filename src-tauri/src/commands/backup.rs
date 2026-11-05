//! 手动数据库备份与启动备份警告的 IPC 入口。

use crate::db;
use crate::error::AppResult;
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn create_database_backup(state: State<'_, AppState>) -> AppResult<db::BackupInfo> {
    let conn = state.db.lock().await;
    db::create_manual_backup(&conn)
}

/// 读取并消费迁移前备份失败的一次性警告。成功读取后删除，避免每次启动重复提示。
#[tauri::command]
pub async fn take_startup_backup_warning(state: State<'_, AppState>) -> AppResult<Option<String>> {
    let conn = state.db.lock().await;
    db::take_startup_backup_warning(&conn)
}
