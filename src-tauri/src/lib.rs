// pub 供集成测试（tests/）直接复用数据层与抓取管线
pub mod ai;
pub mod commands;
pub mod config_sync;
pub mod db;
pub mod error;
pub mod extraction;
pub mod ingestion;
pub mod media;
pub mod miniflux;
pub mod opml;
pub mod sanitize;
pub mod scheduler;
pub mod state;
pub mod sync;

use std::path::PathBuf;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

/// 主窗口显示并聚焦（托盘恢复/二实例唤起共用）
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// 读取 closeToTray 设置（缺省 true：关闭即最小化到托盘）
async fn read_close_to_tray(db: &std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> bool {
    let conn = db.lock().await;
    match crate::db::get_setting(&conn, "app_settings") {
        Ok(Some(raw)) => serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("closeToTray").and_then(|b| b.as_bool()))
            .unwrap_or(true),
        _ => true,
    }
}

/// 首次关闭询问是否已展示过（app_settings.closePromptShown，缺省 false）
async fn read_close_prompt_shown(db: &std::sync::Arc<tokio::sync::Mutex<rusqlite::Connection>>) -> bool {
    let conn = db.lock().await;
    crate::db::get_setting(&conn, "app_settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("closePromptShown").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// 用户在首次关闭询问弹窗里做出选择：
/// remember=true → 持久化 closeToTray + closePromptShown（此后不再问）；
/// remember=false → 仅本次生效（下次关闭再问）。
#[tauri::command]
async fn resolve_close(app: tauri::AppHandle, action: String, remember: bool) -> Result<(), String> {
    let to_tray = action == "tray";
    let db = app.state::<state::AppState>().db.clone();
    if remember {
        let conn = db.lock().await;
        let raw = crate::db::get_setting(&conn, "app_settings")
            .ok()
            .flatten()
            .unwrap_or_else(|| "{}".into());
        let mut v: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));
        v["closeToTray"] = serde_json::json!(to_tray);
        v["closePromptShown"] = serde_json::json!(true);
        let _ = crate::db::set_setting(&conn, "app_settings", &v.to_string());
        /* 设置页的开关镜像同步（前端 bootstrapSettings 恢复，当前会话里
           事件通知前端刷新——见 close-resolved 事件） */
    }
    if let Some(win) = app.get_webview_window("main") {
        if to_tray {
            let _ = win.hide();
        } else {
            app.exit(0);
        }
    }
    let _ = app.emit("close-resolved", to_tray);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例：二实例启动 → 聚焦既有窗口后自行退出。必须第一个注册。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        // 窗口状态记忆：尺寸/位置/最大化跨启动保留
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // 开机自启：Windows 注册表 Run 键（设置页开关控制）
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // 系统通知：新文章到达时 Windows toast（scheduler 触发）
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // 数据库放应用数据目录（LocalAppData/FluxReader/fluxreader.db）
            let app_dir: PathBuf = app
                .path()
                .app_data_dir()
                .expect("app data dir unavailable");
            std::fs::create_dir_all(&app_dir)?;
            let db_path = app_dir.join("fluxreader.db");
            let conn = db::open(&db_path).expect("failed to open sqlite database");
            let http = ingestion::build_client(30);
            // SMTC 媒体控制线程（失败降级为 inactive，不影响播放）
            let media = media::spawn_media_thread(app.handle());

            app.manage(state::AppState::new(conn, http, media));

            // 后台刷新调度循环（读 app_settings 的 autoRefresh/refreshInterval）
            scheduler::spawn_scheduler(app.handle().clone());

            // 系统托盘：显示/刷新全部/退出
            let show = MenuItem::with_id(app, "show", "显示 FluxReader", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "刷新全部订阅", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &refresh, &quit])?;
            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "refresh" => {
                        // 复用调度器的全量刷新路径：抓全部源并广播事件
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let state = app.state::<state::AppState>();
                            let db = state.db.clone();
                            let http = state.http.clone();
                            let (n, f) = scheduler::refresh_all(&db, &http).await;
                            let _ = app.emit(
                                "feeds-updated",
                                serde_json::json!({ "new_articles": n, "failed_feeds": f }),
                            );
                        });
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击托盘图标 → 显示主窗口
                    if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭按钮：首次关闭询问（closePromptShown 未置位 → 弹窗让用户选）；
            // 已选过 → 直接按 closeToTray 设置走（隐藏到托盘 / 退出）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle().clone();
                let win = window.clone();
                api.prevent_close();
                tauri::async_runtime::spawn(async move {
                    let db = app.state::<state::AppState>().db.clone();
                    if !read_close_prompt_shown(&db).await {
                        // 首次：前端弹选择对话框（选完调 resolve_close 执行）
                        let _ = app.emit("close-ask", ());
                        return;
                    }
                    if read_close_to_tray(&db).await {
                        let _ = win.hide();
                    } else {
                        app.exit(0);
                    }
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            // 分类
            commands::list_folders,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::update_folder_layout,
            commands::set_folder_collapsed,
            commands::set_folder_ai_flags,
            // 订阅源
            commands::list_feeds,
            commands::add_feed,
            commands::delete_feed,
            commands::update_feed,
            commands::update_feed_layout,
            commands::set_feed_ai_flags,
            // 条目
            commands::list_articles,
            commands::get_article,
            commands::search_articles,
            commands::set_read,
            commands::set_starred,
            commands::mark_all_read,
            commands::feed_counts,
            // 刷新（直连）
            commands::refresh_feed,
            commands::refresh_all_feeds,
            // 设置
            commands::get_setting,
            commands::set_setting,
        // Miniflux 同步
        commands::sync_test,
        commands::sync_save,
        commands::sync_phase,
        commands::sync_disconnect,
        commands::sync_now,
        commands::sync_status,
        // 缓存清理
        commands::cache_cleanup,
        // 首次关闭询问
        resolve_close,
        // AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi）
        commands::save_ai_config,
        commands::get_ai_config,
        commands::ai_list_models,
        commands::ai_summarize,
        commands::ai_translate,
        // 全文提取
        commands::extract_fulltext,
        // OPML 导入导出
        commands::opml_import,
        commands::opml_export,
        // SMTC 系统媒体控制
        media::media_update_full,
        media::media_stop,
        // 配置同步（Gist / WebDAV）
        config_sync::config_sync_save_credentials,
        config_sync::config_sync_upload,
        config_sync::config_sync_download,
        config_sync::config_sync_apply,
        config_sync::config_sync_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
