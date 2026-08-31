//! SMTC 系统媒体控制（Windows）：播客播放条接入系统媒体键/音量浮层。
//! MediaControls 内部持有 WinRT COM 对象（非 Send），采用专用线程持有，
//! 命令经 mpsc 通道投递；SMTC 回调通过 Tauri 事件 `player-media` 转发前端。
//! 非 Windows 平台编译为空实现（Linux CI 不引入 souvlaki）。

use crate::error::AppResult;

/// 线程安全的投递端：AppState 持有，命令层调用
#[derive(Clone)]
pub struct MediaHandle {
    #[cfg(windows)]
    tx: Option<std::sync::mpsc::Sender<MediaCmd>>,
}

#[cfg(windows)]
enum MediaCmd {
    /// (标题, 节目名, 时长秒, 进度秒, 播放中)
    Update { title: String, show: String, duration_sec: f64, position_sec: f64, playing: bool },
    Stop,
}

impl MediaHandle {
    #[cfg(windows)]
    pub fn inactive() -> Self {
        Self { tx: None }
    }

    #[cfg(not(windows))]
    pub fn inactive() -> Self {
        Self {}
    }

    #[cfg(windows)]
    pub fn update(&self, title: &str, show: &str, duration_sec: f64, position_sec: f64, playing: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(MediaCmd::Update {
                title: title.to_string(),
                show: show.to_string(),
                duration_sec,
                position_sec,
                playing,
            });
        }
    }

    #[cfg(not(windows))]
    pub fn update(&self, _title: &str, _show: &str, _duration_sec: f64, _position_sec: f64, _playing: bool) {}

    #[cfg(windows)]
    pub fn stop(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(MediaCmd::Stop);
        }
    }

    #[cfg(not(windows))]
    pub fn stop(&self) {}
}

/// 启动 SMTC 专用线程。HWND 取自主窗口（Windows SMTC 要求绑定窗口）。
/// 启动失败（如无窗口句柄）返回 inactive handle，播放条功能不受影响。
#[cfg(windows)]
pub fn spawn_media_thread(app: &tauri::AppHandle) -> MediaHandle {
    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
    use std::time::Duration;
    use tauri::{Emitter, Manager};

    let hwnd = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as usize);
    let Some(hwnd) = hwnd else {
        log::warn!("media: main window hwnd unavailable, SMTC disabled");
        return MediaHandle::inactive();
    };

    let app_handle = app.clone();
    let (tx, rx) = std::sync::mpsc::channel::<MediaCmd>();

    std::thread::Builder::new()
        .name("smtc-media".into())
        .spawn(move || {
            // HWND 是 Win32 句柄（整数值标识，跨线程使用安全），usize 传递绕开裸指针非 Send
            let hwnd = hwnd as *mut core::ffi::c_void;
            let mut controls = match MediaControls::new(PlatformConfig {
                dbus_name: "com.fluxreader.app",
                display_name: "FluxReader",
                hwnd: Some(hwnd),
            }) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("media: SMTC init failed: {e}");
                    return;
                }
            };

            // SMTC 回调 → Tauri 事件 → 前端 store（togglePlayerPlay）
            if let Err(e) = controls.attach(move |event| {
                let action = match event {
                    MediaControlEvent::Play => "play",
                    MediaControlEvent::Pause => "pause",
                    MediaControlEvent::Toggle => "toggle",
                    MediaControlEvent::Stop => "stop",
                    // 单集播放模型无上/下一集；seek/音量交给播放条本体
                    _ => return,
                };
                let _ = app_handle.emit("player-media", action);
            }) {
                log::warn!("media: SMTC attach failed: {e}");
                return;
            }

            // 指令循环：通道关闭（发送端全 drop）即退出线程
            for cmd in rx {
                match cmd {
                    MediaCmd::Update { title, show, duration_sec, position_sec, playing } => {
                        let playback = if playing {
                            MediaPlayback::Playing {
                                progress: Some(MediaPosition(Duration::from_secs_f64(position_sec.max(0.0)))),
                            }
                        } else {
                            MediaPlayback::Paused {
                                progress: Some(MediaPosition(Duration::from_secs_f64(position_sec.max(0.0)))),
                            }
                        };
                        let _ = controls.set_playback(playback);
                        let _ = controls.set_metadata(MediaMetadata {
                            title: Some(&title),
                            artist: Some(&show),
                            album: None,
                            cover_url: None,
                            duration: Some(Duration::from_secs_f64(duration_sec.max(0.0))),
                        });
                    }
                    MediaCmd::Stop => {
                        let _ = controls.set_playback(MediaPlayback::Stopped);
                    }
                }
            }
        })
        .map(|_| MediaHandle { tx: Some(tx) })
        .unwrap_or_else(|e| {
            log::warn!("media: spawn thread failed: {e}");
            MediaHandle::inactive()
        })
}

#[cfg(not(windows))]
pub fn spawn_media_thread(_app: &tauri::AppHandle) -> MediaHandle {
    MediaHandle::inactive()
}

/* ============================================================
   IPC 命令
   ============================================================ */

/// 播放状态/元数据同步（PlayerBar 节流调用）
#[tauri::command]
pub fn media_update_full(
    state: tauri::State<'_, crate::state::AppState>,
    title: String,
    show: String,
    duration_sec: f64,
    position_sec: f64,
    playing: bool,
) -> AppResult<()> {
    state.media.update(&title, &show, duration_sec, position_sec, playing);
    Ok(())
}

/// 播放结束/关闭播放条
#[tauri::command]
pub fn media_stop(state: tauri::State<'_, crate::state::AppState>) -> AppResult<()> {
    state.media.stop();
    Ok(())
}
