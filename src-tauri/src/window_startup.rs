//! OPT-015：主窗口冷启动「恢复几何 → 显示」的唯一协调点。
//!
//! 用户反馈（F21/F23）：冷启动时窗口先以配置默认位置居中显示，随后才跳到
//! 上次保存的位置——window-state 插件在窗口创建后自动恢复（其恢复尾部会按
//! 保存的 VISIBLE 直接 show/focus），跳变发生在窗口已可见之后。
//!
//! 方案：tauri.conf.json 主窗口 `visible:false` 创建；插件注册时对 main
//! 显式 `skip_initial_state("main")`；本模块在 app setup 阶段（窗口已创建、
//! 尚未展示）一次性恢复几何并显式显示。时序依据（锁定版本 tauri 2.11.5 /
//! tauri-plugin-window-state 2.4.1，均已源码核对）：
//!
//! 1. 插件在 App::build 阶段完成初始化（状态缓存从磁盘读入）；
//! 2. 配置窗口在用户 setup 钩子**之前**同步创建（tauri `app.rs::setup()`：
//!    先 `WebviewWindowBuilder::from_config(..).build()?` 再调用户 setup），
//!    创建触发的插件 `on_window_ready` 对 main 命中 skip 名单 → 插件**不做**
//!    自动恢复；但事件监听照常注册（Moved/Resized/CloseRequested 照常写缓存、
//!    退出时照常保存——跳过只跳「初始恢复」，不影响状态记忆功能）；
//! 3. 插件全生命周期仅有两条恢复入口：`on_window_ready` 的自动恢复（已 skip）
//!    与 `restore_state` IPC 命令（前端从不调用）。因此本模块的显式恢复是
//!    全生命周期唯一一次恢复，不存在「随后再恢复」的第二次跳动。
//!
//! 恢复标志排除 `VISIBLE`：插件恢复尾段的 `should_show → show/focus` 不会
//! 触发，显示时机完全由本模块的 show 单点决定——上次退出时保存的
//! visible=false（如从托盘隐藏态退出）也因此不会让手动启动以不可见开始。
//!
//! 边界：恢复失败（状态损坏等）回退居中——窗口本就在配置位置（center:true）
//! 创建，居中保证任何显示器配置下都落在有效可见区域；旧显示器丢失时插件
//! 不会应用位置（其按 `available_monitors().intersects` 过滤），窗口保持
//! 配置居中位。显示失败向上报错（调用方记日志；应用仍可经托盘唤起，不 panic）。
//! 已知事实：保存态为最大化/全屏时，插件的 maximize/fullscreen 在 Windows
//! 上可能提前让隐藏窗口可见——但几何恢复先于它们执行，不产生位置跳变，
//! show 仍是显式激活点。
//!
//! 实机边界：本模块顺序由单元测试锁定；真实窗口的无闪冷启动需打包 Windows
//! 实例验收（OPT-015 验收清单第 4 项，由主控隔离验收），单测不冒充实机验证。
//!
//! Note: 决策、替代方案与证据边界见
//! .agents/notes/implemented/bug-fix/2026-10-08-窗口首帧恢复与媒体命令幂等.md

use tauri_plugin_window_state::{StateFlags, WindowExt};

/// 恢复标志：全部状态但排除 VISIBLE。
///
/// 排除 VISIBLE 有两重意义：
/// - 插件 restore 尾段「should_show → show/focus」不触发，显示时机单点；
/// - 上次退出保存的 visible=false（托盘隐藏态退出）不会让手动启动以
///   不可见开始——手动启动永远显示。
pub fn restore_flags() -> StateFlags {
    StateFlags::all() - StateFlags::VISIBLE
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

/// 生产路径：对主窗口执行「恢复几何（排除 VISIBLE）→ 失败回退居中 →
/// 显示并聚焦」。app setup 内调用一次；调用时窗口已创建未展示。
pub fn restore_and_show(win: &tauri::WebviewWindow) -> Result<(), String> {
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
        || {
            win.show().map_err(|e| format!("显示主窗口失败：{e}"))?;
            let _ = win.set_focus();
            Ok(())
        },
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
    fn restore_flags_exclude_visible_and_keep_geometry() {
        let flags = restore_flags();
        assert!(
            !flags.contains(StateFlags::VISIBLE),
            "VISIBLE 必须排除：显示时机由显式 show 单点控制，保存的 visible=false 不得让手动启动变隐藏"
        );
        for flag in [
            StateFlags::SIZE,
            StateFlags::POSITION,
            StateFlags::MAXIMIZED,
            StateFlags::FULLSCREEN,
            StateFlags::DECORATIONS,
        ] {
            assert!(flags.contains(flag), "几何/外观标志必须保留：{flag:?}");
        }
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
