# OPT-015P：原生最大化首显闪烁修复

Status: verified-local
父卡OPT015。实际隔离trace已证最大化5ms显示→10ms隐藏→24ms显示，不能仅“排除VISIBLE”宣称隐藏恢复。

## 已知根因

锁定tao0.35.3 WindowFlags::apply_diff 当MAXIMIZED变化调用ShowWindow(SW_MAXIMIZE)，但缓存VISIBLE仍false，随后SW_HIDE；生产最后show再次显示。几何不居中跳不等于没有闪烁。

## 需求（必须实际native验证）

正常状态仍恢复几何后首显；最大化必须首次可见就最大化，之后不隐藏/再显示。内部Tauri/Tao状态与Win32状态一致，后续unmaximize/托盘/保存不损坏。不能固定sleep/关闭动画/放宽driver断言解决。Saved visiblefalse不抑制手动启动；无记录/损坏/offscreen兜底维持。

## 可行方向（执行者据真实测试验证，不照猜测宣称）

几何恢复排除可提前显示的MAXIMIZED，读取同一plugin filename的已保存max/full标记（只本应用状态），在Windows最终显示阶段通过原生ShowWindow(SW_SHOWMAXIMIZED)一次性首显；WM_SIZE会更新Tao cached MAXIMIZED，随后win.show同步VISIBLE时不再走隐藏分支。须仔细实际验证后续state，非Windows仍正常兼容。直接native不是必选，若有更小正确方式可说明；不要patch整个上游crate。Fullscreen需独立评估是否同类早显，不能自作主张不恢复来换绿。

## 写集

- src-tauri/src/window_startup.rs
- src-tauri/src/lib.rs仅必须相关启动衔接（其他IPC不改）
- .agents/notes/implemented/bug-fix/2026-10-08-窗口首帧恢复与媒体命令幂等.md（纠正排除VISIBLE即唯一显示假设）
- tmp/optimization-20261008/OPT-015P/RESULT.md和最小补充测试

不改example/runner（另CodeBuddy正修驱动firstvisibility严格验收，读即可）；不更改Cargo依赖/features，若需原生FFI用现有windows或最小extern并#[cfg(windows)]明确安全边界。不操作正式用户窗口/状态。

## 验证

先模块测试与编译，再借tmp/OPT015N严格driver更新后按固定场景实际跑，driver未完成先记录代码验证不能宣称native通过。最终主控跑6+fullscreen实际首显序列/状态，独立review真实差异。所有只有native调用测试通过不够，必须消费生产restore_and_show。无个人memory/Git/代理。

## 代码与原生证据

Confucius R2 PASS，完整plugin schema校验与最大化首显顺序闭环；主控严格driver run R-20261009-030727-46284 六场景全PASS，max首见已最大化且无hide/re-show；状态损坏9单测通过。fullscreens新增场景仍在执行，物理DPI/拔屏/SMTC边界未冒称完成。
