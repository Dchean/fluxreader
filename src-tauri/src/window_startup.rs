//! OPT-015 / OPT-015P：主窗口冷启动「恢复几何 → 显示」的唯一协调点。
//!
//! 用户反馈（F23）：冷启动先居中再跳位置。隔离原生驱动（OPT-015N R1，严格首见
//! 判据）进一步实测：保存态为最大化时还有可见性闪跳——5ms 显示 → 10ms 隐藏 →
//! 24ms 再显示（几何不跳，但可见性闪跳）。根因（锁定 tao 0.35.3 源码核对）：
//! `WindowFlags::apply_diff`（window_state.rs）在变化含 MAXIMIZED（或 new 含
//! MAXIMIZED）时调用 `ShowWindow(SW_MAXIMIZE)`（L377-387），而 new 不含 VISIBLE
//! 时又立即调用 `ShowWindow(SW_HIDE)`（L420-424）——插件恢复调用 `maximize()`
//! 时缓存 VISIBLE=false，于是「先 SW_MAXIMIZE 显示、随即 SW_HIDE 隐藏」，最后
//! 生产 `win.show()` 才真正显示。仅排除 VISIBLE **不能**阻止该路径（OPT-015
//! 原假设已由 OPT-015P 订正）。
//!
//! 修复（不 patch 上游 crate）：
//! 1. `restore_flags()` 再排除 MAXIMIZED：插件恢复不再触发上述 ShowWindow 路径。
//!    位置不受影响——插件 POSITION 分支按状态里的 `maximized` 字段选 prev_x/prev_y
//!    （与恢复标志无关），最大化改到最终显示阶段单独处理。
//! 2. 从插件同一 filename 的状态文件（只本应用状态，只读）解析 main 的
//!    maximized/fullscreen 标记；缺失/损坏/缺字段 → 按未保存兜底（普通显示）。
//! 3. Windows 上若保存为最大化（且非全屏）：最终显示阶段先原生
//!    `ShowWindow(SW_SHOWMAXIMIZED)`——显示与最大化一次完成，首次可见即最大化、
//!    无先普通后最大化的中间帧；随后 `win.show()` 同步缓存 VISIBLE（apply_diff 的
//!    new 必含 VISIBLE → 不会触发 SW_HIDE；缓存 MAXIMIZED 若已被 WM_SIZE 就地
//!    更新（event_loop.rs L1245-1247）为 true，该次 apply_diff 还会**再发一次
//!    SW_MAXIMIZE**——Tao 可能重复下发 SW_SHOW/SW_MAXIMIZE，均是对已最大化可见
//!    窗口的幂等显示命令，不隐藏/不复位/不改变几何），最后 `win.maximize()` 对齐
//!    缓存 MAXIMIZED（已对准则为空 diff）。三步均不产生 hide/re-show。
//!
//! 插件生命周期与时序（“不发生二次恢复”的锁，沿用 OPT-015 结论）：
//! 1. 插件在 App::build 阶段完成初始化（状态缓存从磁盘读入），先于用户 setup；
//! 2. 配置窗口在用户 setup 钩子**之前**同步创建（tauri `app.rs::setup()`：
//!    先 `WebviewWindowBuilder::from_config(..).build()?` 再调用户 setup），
//!    创建触发的插件 `on_window_ready` 对 main 命中 skip 名单 → 插件**不做**
//!    自动恢复；但事件监听/退出保存照常注册（跳过只跳「初始恢复」）；
//! 3. 插件全生命周期仅有两条恢复入口：`on_window_ready` 的自动恢复（已 skip）
//!    与 `restore_state` IPC 命令（前端从不调用）。本模块的显式恢复是唯一一次。
//!
//! Fullscreen 独立评估（OPT-015P 要求；不得“不恢复换绿”）：
//! tao `set_fullscreen` 在 old==new 时直接返回（window.rs L682-684）——保存
//! fullscreen=false（None==None）即无副作用；转全屏分支仅经 `set_window_flags`
//! 应用 MARKER 位 → apply_diff：new 不含 VISIBLE → 仅 SW_HIDE（对已隐藏窗口
//! no-op），随后 `SetWindowPos(SWP_ASYNCWINDOWPOS | SWP_NOZORDER)`（无
//! SWP_SHOWWINDOW）只在隐藏态落位（window.rs L761-807）——**无同类提前显示**。
//! 因此 FULLSCREEN 保留在插件恢复内，最终显示走普通 show 即为全屏首显。
//! 若 maximized 与 fullscreen 同时为真（不可达组合），按全屏优先走普通显示，
//! 不叠加原生最大化（见 `wants_native_maximized_first`）。
//!
//! 边界：恢复失败（状态损坏等）回退居中后仍显示；显示失败向上报错（调用方记
//! 日志；应用仍可经托盘唤起，不 panic）。保存的 visible=false 不影响手动启动
//! （VISIBLE 始终排除）。旧显示器丢失/无记录/损坏 JSON 均维持原有兜底。
//!
//! 实机边界：模块单测只锁协调顺序/回退/标记解析与决策；真实首次可见序列由
//! OPT-015N 严格驱动（真实 HWND 消息级观测）验收，单测不冒充原生证据。
//!
//! Note: 决策、替代方案与证据边界见
//! .agents/notes/implemented/bug-fix/2026-10-08-窗口首帧恢复与媒体命令幂等.md

use tauri::Manager;
use tauri_plugin_window_state::{AppHandleExt, StateFlags, WindowExt};

/// 恢复标志：尺寸/位置/装饰/全屏，但排除 VISIBLE 与 MAXIMIZED。
///
/// - 排除 VISIBLE：插件恢复尾段的 show/focus 不触发，显示时机单点；保存的
///   visible=false（托盘隐藏态退出）不得让手动启动以不可见开始。
/// - 排除 MAXIMIZED：tao 0.35.3 apply_diff 在最大化恢复时先 ShowWindow(SW_MAXIMIZE)
///   再因缓存 VISIBLE=false 紧接 SW_HIDE——隐藏恢复窗口被提前显示又隐藏
///   （OPT-015P 实测 5ms/10ms/24ms 闪烁）。最大化改由最终显示阶段的原生
///   SW_SHOWMAXIMIZED 与首显合一（见 `final_show`）。
pub fn restore_flags() -> StateFlags {
    StateFlags::all() - StateFlags::VISIBLE - StateFlags::MAXIMIZED
}

/// 插件状态文件里 main 的已保存标记（OPT-015P，只读私有解析）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SavedFlags {
    maximized: bool,
    fullscreen: bool,
}

impl SavedFlags {
    const fn none() -> Self {
        Self {
            maximized: false,
            fullscreen: false,
        }
    }
}

/// 插件 2.4.1 `WindowState` 的私有镜像（OPT-015P R2）：字段名/类型/必填性逐一
/// 对齐其 serde derive——`width/height: u32`、`x/y/prev_x/prev_y: i32`、
/// `maximized/visible/decorated/fullscreen: bool`，**全部 required**（无
/// `#[serde(default)]`），与插件一致地忽略未知字段。
///
/// 为什么连不读取的字段也要声明：插件的 `load_saved_window_states` 是
/// `serde_json::from_reader::<HashMap<String, WindowState>>` 的**整体**反序列化
/// ——任一 label 的任一必填字段缺失/类型不符都会让整表 Err → 空缓存 → 无记录
/// 兜底。本解析必须同语义：半截记录（如只有 maximized/fullscreen 两个布尔）
/// 在插件眼里是损坏文件，绝不能在本侧被采信而启用原生最大化。
/// 字段仅用于 schema 校验、不读取其他值，故标注 allow(dead_code)。
#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct PluginWindowState {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    prev_x: i32,
    prev_y: i32,
    maximized: bool,
    visible: bool,
    decorated: bool,
    fullscreen: bool,
}

/// 只读解析插件 2.4.1 状态 JSON（`{ "<label>": WindowState }`）里 main 的
/// maximized/fullscreen。语义与插件整体反序列化一致：整表按
/// `HashMap<String, PluginWindowState>` 解析，任一 label 无效/顶层非对象
/// → 整体按未保存兜底（`SavedFlags::none()`）；整表合法才取 main 的
/// max/full；无 main 条目同样兜底。全程无平台/cfg 分支差异。
fn saved_flags_from_json(raw: &str) -> SavedFlags {
    let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, PluginWindowState>>(raw)
    else {
        return SavedFlags::none();
    };
    match map.get("main") {
        Some(s) => SavedFlags {
            maximized: s.maximized,
            fullscreen: s.fullscreen,
        },
        None => SavedFlags::none(),
    }
}

/// 从插件同一 filename 的落盘状态读取 main 标记：不复制插件常量——`app.filename()`
/// 就是插件实际使用/保存的文件名（生产为默认 `.window-state.json`；驱动注入
/// 绝对路径时 join 语义与原插件一致）。文件缺失/不可读 → 未保存兜底。
fn read_saved_flags(app: &tauri::AppHandle) -> SavedFlags {
    let path = match app.path().app_config_dir() {
        Ok(dir) => dir.join(app.filename()),
        Err(_) => return SavedFlags::none(),
    };
    match std::fs::read_to_string(path) {
        Ok(raw) => saved_flags_from_json(&raw),
        Err(_) => SavedFlags::none(),
    }
}

/// 是否走「原生最大化首显」：保存最大化且非全屏。全屏优先（真全屏窗口再叠加
/// 最大化在物理上不可达，且会破坏全屏几何；此组合按全屏普通显示处理）。
/// 纯决策函数（平台无关）；Windows 分支由 `final_show` 的 cfg 门控。
#[cfg_attr(not(windows), allow(dead_code))]
fn wants_native_maximized_first(maximized: bool, fullscreen: bool) -> bool {
    maximized && !fullscreen
}

/// 最小 Win32 FFI（OPT-015P）：仅用于最终显示阶段的原生「显示并最大化」。
/// 不新增 `windows` crate feature（依赖/features 零改动）；`user32` 为系统库，
/// 进程本已链接（tao 使用）。
///
/// 安全边界：`ShowWindow` 的 `hwnd` 必须是**本进程内**有效窗口句柄（来自
/// `WebviewWindow::hwnd()`，且仅在窗口所属线程调用）；只发送该窗口的显示命令，
/// 不跨进程、不注入、不改样式/布局；对隐藏窗口 `SW_SHOWMAXIMIZED` 使其
/// 一次完成显示与最大化。
#[cfg(windows)]
mod win32_ffi {
    pub const SW_SHOWMAXIMIZED: i32 = 3;

    #[link(name = "user32")]
    extern "system" {
        pub fn ShowWindow(hwnd: *mut core::ffi::c_void, n_cmd_show: i32) -> i32;
    }
}

/// 原生首显最大化：`ShowWindow(SW_SHOWMAXIMIZED)`。失败仅可能是拿不到 HWND
/// （返回 Err 由调用方回退普通显示）；ShowWindow 的返回值（此前可见性）不可作
/// 失败判据——本场景调用前窗口必然隐藏，返回 0 属正常。
#[cfg(windows)]
fn show_maximized_native(win: &tauri::WebviewWindow) -> Result<(), String> {
    let hwnd = win.hwnd().map_err(|e| format!("获取窗口句柄失败：{e}"))?.0;
    // SAFETY: 见 win32_ffi 模块注释——本进程有效 HWND，仅显示命令。
    unsafe {
        win32_ffi::ShowWindow(hwnd, win32_ffi::SW_SHOWMAXIMIZED);
    }
    Ok(())
}

/// 启动协调助手：恢复 →（失败时回退）→ 显示。返回显示结果。
///
/// 顺序契约（由本模块单测锁定，无 sleep、无真实窗口）：
/// - 恢复成功：restore → show；
/// - 恢复失败：restore → fallback → show（恢复失败不阻断显示，回退保证
///   窗口仍出现在有效可见位置）；
/// - 显示失败：Err 原样上抛（调用方负责记录/上报）。
pub fn run_sequence<R, F, S>(restore: R, fallback: F, show: S) -> Result<(), String>
where
    R: FnOnce() -> Result<(), String>,
    F: FnOnce(),
    S: FnOnce() -> Result<(), String>,
{
    if let Err(e) = restore() {
        log::warn!("window-startup: 恢复窗口几何失败，回退居中：{e}");
        fallback();
    }
    show()
}

/// 最终显示：保存最大化时走原生「显示并最大化」，否则常规 show。
fn final_show(win: &tauri::WebviewWindow, saved: &SavedFlags) -> Result<(), String> {
    #[cfg(windows)]
    if wants_native_maximized_first(saved.maximized, saved.fullscreen) {
        match show_maximized_native(win) {
            Ok(()) => {
                // 对齐 tao 缓存：show() 置缓存 VISIBLE=true（new 必含 VISIBLE →
                // 不会再发 SW_HIDE；若缓存 MAXIMIZED 已由 WM_SIZE 对齐，还会再发一次
                // SW_MAXIMIZE——no-op，不隐藏/不复位）；maximize() 置缓存 MAXIMIZED=true
                // （已对齐则空 diff）。两者对已最大化可见窗口均为幂等显示命令。
                win.show().map_err(|e| format!("显示主窗口失败：{e}"))?;
                if let Err(e) = win.maximize() {
                    log::warn!("window-startup: 对齐最大化内部状态失败（窗口已显示，不阻断）：{e}");
                }
                let _ = win.set_focus();
                return Ok(());
            }
            Err(e) => {
                log::warn!("window-startup: 原生最大化首显不可用，回退常规显示：{e}");
                // 落到下方普通 show（几何已恢复、窗口仍隐藏）
            }
        }
    }
    #[cfg(not(windows))]
    let _ = saved; // 非 Windows：标记仅 Windows 决策用，保持「恢复几何后 show」语义

    win.show().map_err(|e| format!("显示主窗口失败：{e}"))?;
    let _ = win.set_focus();
    Ok(())
}

/// 生产路径：对主窗口执行「恢复几何（排除 VISIBLE/MAXIMIZED）→ 失败回退居中 →
/// 显示（保存最大化时 Windows 原生 SW_SHOWMAXIMIZED 与首显合一）并聚焦」。
/// app setup 内调用一次；调用时窗口已创建未展示。
pub fn restore_and_show(win: &tauri::WebviewWindow) -> Result<(), String> {
    let saved = read_saved_flags(win.app_handle());
    run_sequence(
        || {
            win.restore_state(restore_flags())
                .map_err(|e| format!("恢复窗口状态失败：{e}"))
        },
        || {
            // 回退：居中。覆盖「恢复半途失败已部分改动位置」的场景，保证
            // 最终位置仍落在可见区域（首次/无记录时窗口本就在配置居中位）。
            let _ = win.center();
        },
        || final_show(win, &saved),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 记录调用次序的执行器：restore/show 的结果可注入。
    fn run_recorded(restore_ok: bool, show_ok: bool) -> (Vec<&'static str>, Result<(), String>) {
        let calls = RefCell::new(Vec::new());
        let result = run_sequence(
            || {
                calls.borrow_mut().push("restore");
                if restore_ok {
                    Ok(())
                } else {
                    Err("恢复失败".into())
                }
            },
            || calls.borrow_mut().push("fallback"),
            || {
                calls.borrow_mut().push("show");
                if show_ok {
                    Ok(())
                } else {
                    Err("显示失败".into())
                }
            },
        );
        (calls.into_inner(), result)
    }

    #[test]
    fn startup_sequence_restore_then_show_exactly_once() {
        let (calls, result) = run_recorded(true, true);
        assert_eq!(
            calls,
            vec!["restore", "show"],
            "恢复成功后必须直接显示，不经回退"
        );
        assert_eq!(
            calls.iter().filter(|c| **c == "restore").count(),
            1,
            "恢复必须恰好执行一次（不重复恢复）"
        );
        assert!(result.is_ok());
    }

    #[test]
    fn startup_restore_error_falls_back_then_shows() {
        let (calls, result) = run_recorded(false, true);
        assert_eq!(
            calls,
            vec!["restore", "fallback", "show"],
            "恢复失败必须先回退再显示（失败不阻断显示）"
        );
        assert!(result.is_ok(), "恢复失败不算最终失败——回退后仍需显示成功");
    }

    #[test]
    fn startup_show_error_is_reported() {
        let (calls, result) = run_recorded(true, false);
        assert_eq!(calls, vec!["restore", "show"]);
        assert_eq!(result.unwrap_err(), "显示失败", "显示失败必须明确上抛");
    }

    #[test]
    fn startup_show_error_reported_even_after_fallback() {
        let (calls, result) = run_recorded(false, false);
        assert_eq!(calls, vec!["restore", "fallback", "show"]);
        assert!(result.is_err(), "恢复失败 + 显示失败：显示错误仍必须上报");
    }

    #[test]
    fn restore_flags_exclude_visible_and_maximized_keep_geometry() {
        let flags = restore_flags();
        assert!(
            !flags.contains(StateFlags::VISIBLE),
            "VISIBLE 必须排除：显示时机由显式 show 单点控制，保存的 visible=false 不得让手动启动变隐藏"
        );
        assert!(
            !flags.contains(StateFlags::MAXIMIZED),
            "MAXIMIZED 必须排除：tao apply_diff 在最大化恢复时先 ShowWindow(SW_MAXIMIZE) \
             再因缓存 VISIBLE=false 紧接 SW_HIDE（实测 5ms/10ms/24ms 闪烁）；\
             最大化改由原生 SW_SHOWMAXIMIZED 与首显合一（OPT-015P）"
        );
        for flag in [
            StateFlags::SIZE,
            StateFlags::POSITION,
            StateFlags::FULLSCREEN,
            StateFlags::DECORATIONS,
        ] {
            assert!(flags.contains(flag), "几何/外观标志必须保留：{flag:?}");
        }
    }

    /// 完整合法 fixture（插件 2.4.1 WindowState 全部 required 字段；与 OPT-015N
    /// runner 场景状态同形）。长字符串字面量不可折行，rustfmt 稳定。
    const FULL_MAIN_MAX: &str = r#"{"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;
    const FULL_MAIN_MAX_FULL: &str = r#"{"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":true}}"#;
    const FULL_OTHER_PLAIN: &str = r#"{"other":{"width":100,"height":100,"x":0,"y":0,"prev_x":0,"prev_y":0,"maximized":false,"visible":true,"decorated":true,"fullscreen":false}}"#;
    const MAIN_MISSING_FULLSCREEN: &str = r#"{"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false}}"#;
    const MAIN_FULLSCREEN_NUM: &str = r#"{"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":false,"visible":true,"decorated":false,"fullscreen":1}}"#;
    const MAIN_WIDTH_NEG: &str = r#"{"main":{"width":-1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;
    const MAIN_X_STR: &str = r#"{"main":{"width":1600,"height":1100,"x":"200","y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;
    const MAIN_MAXIMIZED_STR: &str = r#"{"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":"yes","visible":true,"decorated":false,"fullscreen":false}}"#;
    const MAP_OTHER_MISSING_WITH_FULL_MAIN: &str = r#"{"other":{"maximized":true},"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;
    const MAP_OTHER_BADTYPE_WITH_FULL_MAIN: &str = r#"{"other":{"width":"bad","height":100,"x":0,"y":0,"prev_x":0,"prev_y":0,"maximized":false,"visible":true,"decorated":true,"fullscreen":false},"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;
    const MAP_OTHER_OK_WITH_FULL_MAIN: &str = r#"{"other":{"width":100,"height":100,"x":0,"y":0,"prev_x":0,"prev_y":0,"maximized":false,"visible":true,"decorated":true,"fullscreen":false},"main":{"width":1600,"height":1100,"x":200,"y":150,"prev_x":200,"prev_y":150,"maximized":true,"visible":true,"decorated":false,"fullscreen":false}}"#;

    #[test]
    fn saved_flags_parse_main_marks_with_safe_fallback() {
        assert_eq!(
            saved_flags_from_json(FULL_MAIN_MAX),
            SavedFlags {
                maximized: true,
                fullscreen: false
            }
        );
        assert_eq!(
            saved_flags_from_json(FULL_MAIN_MAX_FULL),
            SavedFlags {
                maximized: true,
                fullscreen: true
            }
        );
        for bad in [
            "",
            "not json",
            "[]",
            "null",
            "{}",
            r#"{"main":{}}"#,
            r#"{"main":{"maximized":true,"fullscreen":false}}"#,
            MAIN_MAXIMIZED_STR,
        ] {
            assert_eq!(
                saved_flags_from_json(bad),
                SavedFlags::none(),
                "损坏/缺字段/缺 main/类型不符必须按未保存兜底：{bad}"
            );
        }
    }

    /// R2（OPT-015P review P2）：解析必须与插件 2.4.1 `load_saved_window_states`
    /// 的整体反序列化（`HashMap<String, WindowState>`，全部字段 required）语义一致——
    /// 任一 label 的任一必填字段缺失/类型不符都会让**整表**落空（插件同样整体 Err →
    /// 空缓存 → 无记录兜底）。只有完整合法表才取 main 的 max/full；半截记录
    /// （如只有 maximized/fullscreen 两个布尔）绝不能触发原生最大化，否则会破坏
    /// 「损坏记录 → 回退居中」语义。
    #[test]
    fn saved_flags_require_full_plugin_schema_whole_map() {
        // 审查反例：缺 width/height/x/y/prev_*/visible/decorated 的「半截」main——
        // 插件会拒绝整表（空缓存），本解析也必须 none。
        assert_eq!(
            saved_flags_from_json(r#"{"main":{"maximized":true,"fullscreen":false}}"#),
            SavedFlags::none(),
            "缺必填字段的半截记录必须整体按未保存兜底（不得启用原生最大化）"
        );
        // 完整 main 但 fullscreen 缺失 / 类型错 / 数值字段范围或类型错
        for bad in [
            MAIN_MISSING_FULLSCREEN,
            MAIN_FULLSCREEN_NUM,
            MAIN_WIDTH_NEG,
            MAIN_X_STR,
        ] {
            assert_eq!(
                saved_flags_from_json(bad),
                SavedFlags::none(),
                "必填字段缺失/类型不符（与插件 u32/i32/bool 口径一致）→ 整表无效：{bad}"
            );
        }
        // 另一 label 无效（缺字段/类型错）→ 整表 none，即使 main 完整且 maximized=true
        for bad in [
            MAP_OTHER_MISSING_WITH_FULL_MAIN,
            MAP_OTHER_BADTYPE_WITH_FULL_MAIN,
        ] {
            assert_eq!(
                saved_flags_from_json(bad),
                SavedFlags::none(),
                "整表任一 label 无效 → 与插件一致地整体落空：{bad}"
            );
        }
        // 整表全部合法时才使用 main 的 max/full（其他 label 合法完整不干扰）
        assert_eq!(
            saved_flags_from_json(MAP_OTHER_OK_WITH_FULL_MAIN),
            SavedFlags {
                maximized: true,
                fullscreen: false
            },
            "整表全部合法才取 main 的 max/full"
        );
        // 整表合法但无 main → none（不猜其他 label）
        assert_eq!(
            saved_flags_from_json(FULL_OTHER_PLAIN),
            SavedFlags::none(),
            "无 main 条目 → 未保存兜底（其他 label 的标记不适用）"
        );
    }

    #[test]
    fn native_maximized_first_decision_covers_all_combinations() {
        assert!(
            wants_native_maximized_first(true, false),
            "保存最大化：走原生首显"
        );
        assert!(
            !wants_native_maximized_first(false, false),
            "非最大化：普通显示"
        );
        assert!(
            !wants_native_maximized_first(false, true),
            "仅全屏：插件已恢复全屏，普通 show 即全屏首显（无同类早显）"
        );
        assert!(
            !wants_native_maximized_first(true, true),
            "全屏优先：不叠加原生最大化（不可达组合的定义行为）"
        );
    }

    /// 接线锁定（静态证据）：
    /// - 插件注册必须对 main 跳过初始自动恢复（插件唯一自动恢复入口）；
    /// - app setup 必须恰好调用一次本模块恢复入口。
    ///
    /// 两者共同保证「全生命周期只有一次恢复、不发生二次跳动」。真实窗口
    /// 行为由打包实例验收（卡片第 4 项），本断言只锁接线形态。
    #[test]
    fn lib_registration_skips_initial_restore_and_calls_once() {
        let src = include_str!("lib.rs");
        assert!(
            src.contains("skip_initial_state(\"main\")"),
            "window-state 插件必须对 main 跳过初始自动恢复"
        );
        assert_eq!(
            src.matches("window_startup::restore_and_show(&win)")
                .count(),
            1,
            "app setup 必须恰好调用一次 window_startup::restore_and_show(&win)"
        );
    }
}
