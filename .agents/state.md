# 半成品现状与待办切片

如实记录当前停在何处。**基线**：`dev` @ `5278a64`（文档快照点），业务代码改动未提交、未验证。本页不复制长文，只给路径引用。

## 一、当前真实停点

正在进行的是 **S4b 子批 D8/N2/X11 + X25**（WebDAV 传输层旧协议清退与内部错误归属）：

- 已完成：`webdav_transport.rs` 删除旧 probe / LOCK / checkpoint / MOVE 路径（约 −933 行）；新增 `src-tauri/src/local_sync/session_tests.rs` 与 `src-tauri/tests/webdav_library_e2e.rs`；`webdav_transport_e2e.rs` 中被承接的两个 mock 及独占 helper 已删除。
- **已完成（本机门禁）**：`cargo fmt --all -- --check`、`cargo check --all-targets`、`cargo clippy --all-targets -- -D warnings`、`cargo test --no-fail-fast` 全部 exit 0（基线 `04df1a1`，2026-11-23 本机运行）。`session_tests` 8 项 `x25_*` 与 `webdav_library_e2e` 3 项均实际执行通过。
- **已完成（独立只读审计）**：2026-11-23 全新只读上下文核 `04df1a1` 原件，判定「可以收口」，阻断 0 / 重要 0 / 次要 3（非阻塞）；原件 `tmp/audit-2026-11-23/s4b-d8-x25-review.md`，摘要登记在 `docs/audits.md`。
- **仍未完成**：CI 未运行（Actions 因额度用尽被手动禁用）；真实 WsgiDAV 实机用例未跑（旧 MOVE 用例已随协议退役，缺口见 `docs/roadmap.md`）；真实服务端 / 容量 / 原生 WebView2 全未验证。
- 本地跑 Rust 门禁时 `TEMP`/`TMP` 必须指到**工作区内可写目录**（如 `tmp/rctest`）：指到 `C:\WINDOWS\TEMP` 或会话 scratch 时，`llvm-rc` 与 `clang-cl` 会因无法创建临时文件而失败。门禁命令本身不改，属环境前置。
- 原始证据与授权边界：`.agents/archive/legacy-workflow/handoffs/2026-11-22-top-handoff.md`（归档，仅追溯用）。

本批已提交并推送：`04df1a1`（业务改动，10 文件 +785/−1367）与 `387cc51`（roadmap 缺口登记）。

## 二、剩余工作拆为垂直切片

每个切片都能独立演示 + 独立验收。切片草案见 `.agents/specs/`（一篇一个切片，只写验收口径，不写实现）。

| # | 切片 | 能演示什么 | 验收口径（草案） |
| --- | --- | --- | --- |
| 1 | 收口 D8/X25 在飞改动 | 传输层不再有旧 probe/lease/MOVE 生产路径，错误归属正确 | 定向 `cargo check/clippy/test` 绿 + 旧 ignored WsgiDAV 测试按约定跑通 + 全新只读独审 |
| 2 | 退订收藏界面闭环 | 退订后仍能读收藏、计数正确、新设备恢复可见 | headless 部分已覆盖并通过（`tools/component-tests/reading-session.test.mjs`）。真实宿主发现并**已修复**一个缺陷：添加订阅源后新文章入库、计数变了、列表仍空 —— 根因是 `reloadFromBackend` 的 refresh 分支（为后台同步保成员）不重捕获成员，而 addFeed 走的正是它。现由 `reloadFromBackend({ recapture: true })` 重新捕获，回归 `tools/component-tests/add-feed-refresh.test.mjs`（已做变异验证：去掉 recapture 两条用例即红），真机复验同一操作 t+3s 即出现文章。**仍缺**：退订后 Reader 显示原来源名一步未在真实宿主走完；换机恢复未验 |
| 3 | 正文保留控件真实 Edge 验证 | 默认永久/1–3650 天、原子保存、失败回滚在真实宿主可用 | **已在真实 WebView2 验证通过**（隔离数据根 + CDP 驱动）：`dataMode=tauri` 控件才渲染；默认「永久保留」；切「限定天数」出现输入框且默认 30；输入 0 / 3651 → `aria-invalid=true`，3650 → 通过；Enter 提交后隔离库 `readBodyRetentionDays=90` 落盘。未覆盖：保存失败回滚（需注入后端失败）、真实键盘 Tab 顺序 |
| 4 | 列表宽度口径统一 | 校验区间与 UI 夹取一致，不再"能存但显示被夹" | 二选一拍板（放宽 UI 到 560 或收紧校验到 420）后补回归 |

## 三、仍有效的未完成需求（来自 roadmap）

- 真实宿主事件链、真实 FreshRSS/Miniflux 部署组合未验。
- WebDAV 远端物理删除关闭；单目录 256 对象 / XML 256 KiB / actor 64 上限仍生效。
- 真实 WebView2 连续阅读、AI 自动触发真实链、换机恢复原生链未验。
- 50k distinct-body 未验证；整库 state 应用的 retention 瓶颈未修。
- 原生 Windows 安装器 / DPI / 冷启未验；真实 symlink 用例仍 `#[ignore]`。
- `docs/contracts.md` 三处事实漂移待你裁决（outbox 事务边界、`p/*` 投影值域、`url_norm` 非 UTF-8 有损）。

完整条目与验证边界见 [docs/roadmap.md](../docs/roadmap.md)；不要在本页重复维护。
