# OPT-015Q：生产自动建窗路径下的冷启位置恢复

Status: verified-local
父卡 OPT-015。用户 2026-10-15 复测：窗口挪动位置后，下次启动**先出现在屏幕中央、再跳到上次关闭时的位置**。OPT-015/015N/015P 的既有证据与提交（8d863ae、af09d62、OPT-015N 八场景）均未覆盖“Tauri 按 config 自动建窗”这一生产创建路径，因此不能据旧证据宣称已修。

## 已确认事实（主控源码核对）

- `src-tauri/tauri.conf.json` main：`center: true`、`visible: false`、`decorations: false`、1400x900。
- `src-tauri/src/lib.rs`：插件 `.skip_initial_state("main")`；setup 里调用一次 `window_startup::restore_and_show(&win)`。
- `src-tauri/src/window_startup.rs`：`restore_flags() = all - VISIBLE - MAXIMIZED`；`restore_and_show` = `run_sequence(restore, fallback=center, final_show)`。
- 插件 2.4.1：`skip_initial_state` 只跳过 `on_window_ready` 的初始 `restore_state`；插件 setup 仍从磁盘 `load_saved_window_states` 到内存 cache，仍注册 `Moved`/`Resized` 监听更新 cache，`RunEvent::Exit` 仍以 all flags `save_window_state` 落盘。
- 旧 OPT-015N 驱动在运行时 `context.config_mut().app.windows.clear()` 后由用户在 setup 手工 `from_config` 建窗，**不覆盖 Tauri 生产自动建窗路径**；8 场景全绿与用户实测矛盾，说明差异点在创建/恢复的真实时序。

## 目标

在生产真实路径（Tauri 按 config 自动创建 main + setup 真实调用恢复）上用隔离原生驱动复现“先在屏幕中央、再到保存位置”，并修复到：**首次可见（IsWindowVisible 0→1）时的几何就是保存位置/尺寸；首次可见之前不出现居中可见帧；退出时插件保存文件不得被居中/默认几何覆盖。**

## 非目标

- 不以删除 `center: true` 换绿（未经验证的“减少一次居中”不算修复）。
- 不引入 sleep/延时、不关闭动画、不放宽断言。
- 不 patch 上游 crate，不改 Cargo 依赖/features。
- 不缩小或关闭插件保存/恢复功能。
- 不触碰正式 `com.fluxreader.app` 用户数据、DB、窗口状态文件。

## 写集

- `src-tauri/src/window_startup.rs`
- `src-tauri/src/lib.rs`（仅必要启动接线）
- `.agents/notes/implemented/bug-fix/2026-10-08-窗口首帧恢复与媒体命令幂等.md`（就地更新结论）
- `tmp/optimization-20261008/OPT-015Q/`（驱动、runner、原始证据；不进主线）

## 必须交付

1. **生产路径红证据**：隔离驱动不清空 config windows，让 Tauri 自动建窗；测“保存非居中位置 + 启动”，记录：
   - 插件 cache 初始磁盘值；
   - `restore_and_show` 调用前窗口 position/size；
   - 恢复期间 `Moved`/`Resized`/显示命令的事件顺序；
   - 首次可见瞬间的窗口矩形（是否仍在居中位）；
   - 退出保存文件内容是否被覆盖成居中/默认值。
   若首次可见就是保存位置，必须给出反例解释（为何用户仍见跳动），不能直接判绿。
2. **行为反例先红后绿**：红证据必须来自当前 HEAD 的真实冷启序列；修复后同一驱动同一场景转绿。
3. **修复**：确保首次可见即保存几何；处理“创建期居中/默认事件写入插件 cache 并在退出落盘覆盖正确位置”的时序风险；无记录/损坏/offscreen/最大化/全屏兜底与 OPT-015N/015P 既有行为不回归。
4. **回归**：现有 `window_startup` 单测、OPT-015N 八场景、OPT-015P 最大化首显不得回归；新增生产路径场景纳入隔离驱动。

## 门禁

`npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`、`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`；隔离原生驱动单独运行并把原始证据写回本卡进度。不跑真实账号/DB/付费接口；不提交、不推送、不新建 worktree。

## 验收边界

隔离驱动的 identifier、window-state 文件、webview 数据目录均指向 `tmp/optimization-20261008/OPT-015Q/`；报告须写明隔离对结论的影响。物理拔屏、动态 DPI、多显示器热插拔不在本卡自动化范围，保持既有缺口记录。

## 进度

- 2026-10-15：立卡；差集驱动取证（R1 红证据未复现，R1 审计「部分可信」指出隔离口径失实）。
- 2026-10-15 R2（编码会话 `code-opt015q-20261015` 复用）：按整改方案 (b) 收口——auto 臂 UDF 口径修正为落 `%LOCALAPPDATA%\<隔离 identifier>`、冷/热移入未排除、措辞修正、残留清理（LOCAL 32/278.99MB + APPDATA 59/0MB → 0/0）、按最终源码重跑。
- R2 主差集 run `R-20261015-091053-50124`：6 场景 × 2 模式 = 12 run 全绿；OPT-015N 回归 8/8 PASS（`R-20261015-091140-8888`）；auto+生产插件集探针绿（`R-20261015-091219-35500`）。source SHA `234a9e78…`（P3 轮撤回注释后）、R2 run 绑定 exe `56bab854…`。
- 独立 Codex 只读复审：条件通过，P0/P1/P2 无遗留；P3 措辞级三项已处置（驱动注释/文案 + RED-EVIDENCE 措辞统一），P3-3 维持现有边界口径不改文件。
- **结论：dev HEAD 的 auto 生产建窗路径未复现「先居中再跳」；红证据不成立，按任务卡「不做无依据的修复」，生产代码零改动。** 用户现象最可能来自未含修复的发版构建（v0.17.1 早于 `8d863ae`/`af09d62`），属推测（有旁证），构建归属未证实。
- 证据：`tmp/optimization-20261008/OPT-015Q/{RED-EVIDENCE.md,RESULT.md,source/,runner/,runs/}`（tmp 不进主线）；隔离边界与取舍落盘 `.agents/notes/implemented/testing/2026-10-15-窗口启动隔离驱动的证据边界.md`。

### 差集驱动（2026-10-15，主控）

为确定性区分“生产自动建窗路径 vs OPT-015N 手工建窗路径”，隔离驱动采用**差集**设计：同一 driver / 同一 WEBVIEW 数据目录 / 同一隔离 state 文件 / 同一最小页面，仅切换 `build_auto` 一个开关：

- `manual`：复刻 OPT-015N（清空 config windows + setup 手工 `from_config` 建窗）——必须绿。
- `auto`：不清空 config windows，让 Tauri 按 config 自动创建 main，再在 setup 调同一 `restore_and_show` ——预期红。

若 `manual` 绿而 `auto` 红，即锁定差异在自动建窗路径，而不是 Cargo.lock、WebView2 数据目录或隔离方式造成的伪影；若两者都红，则需改口径。`windows.clear()` 在 Tauri 2.11.5 是 `pub fn`，driver 可直接调用。

驱动同时记录：插件 state 文件初值、`restore_and_show` 前窗口矩形、`Moved`/`Resized`/`SHOWWINDOW` 轨迹、首次可见（0→1）瞬间矩形，以及进程退出后的 state 文件内容（判断是否被居中值覆盖）。

## 待派发

- 编码会话：`code-opt015q-20261015`（新开，不复用/不 resume）。
- 编码完成后：全新只读审计 `audit-opt015q-r1-20261015`（`--tools "Read,Grep,Glob"`，无 resume），再由未参与实现的 Codex 子代理复审。
