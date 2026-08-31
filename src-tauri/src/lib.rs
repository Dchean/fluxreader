// pub 供集成测试（tests/）直接复用数据层与抓取管线
pub mod ai;
pub mod commands;
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
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // 窗口状态记忆：尺寸/位置/最大化跨启动保留（npm 包已装，此前 Rust 侧未注册）
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // 开机自启：Windows 注册表 Run 键（设置页开关控制）
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
            Ok(())
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
            commands::update_feed_layout,
            commands::set_feed_ai_flags,
            commands::move_feed,
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
        commands::sync_connect,
        commands::sync_disconnect,
        commands::sync_now,
        commands::sync_status,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
