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
| 2 | 退订收藏界面闭环 | 退订后仍能读收藏、计数正确、新设备恢复可见 | headless 已覆盖并通过。真实宿主完成两轮：①发现并**已修复**「添加订阅源后计数变了、列表仍空」（`reloadFromBackend({ recapture: true })`，回归 `add-feed-refresh.test.mjs`，做过变异验证）；②**退订后收藏可读已验证**——删源后 `feeds.active=0`、未收藏条目被删、收藏条目保留，收藏计数 1，切「收藏」视图卡片来源标签仍为「复现源」，打开 Reader 的 `reader-col` 显示「复现源 · 保留收藏样本 1 · 正文」。同批修正删除确认框文案（原文写「全部本地条目一并移除」，与后端只删未收藏条目的实际行为不符）。**仍缺**：换机恢复组合未验 |
| 3 | 正文保留控件真实 Edge 验证 | 默认永久/1–3650 天、原子保存、失败回滚在真实宿主可用 | **已完成**（真实 WebView2 + 隔离数据根 + CDP）：`dataMode=tauri` 才渲染；默认「永久保留」；切「限定天数」出现天数输入且默认 30；0 / 3651 → `aria-invalid=true`，3650 通过；Enter 提交后隔离库 `readBodyRetentionDays=90` 落盘。**真实键盘**（CDP `Input.dispatchKeyEvent` 可信事件）：combobox → Tab → 天数输入，Shift+Tab 回到 combobox，焦点始终留在弹窗内。**失败回滚**（另一进程 `BEGIN EXCLUSIVE` 占写锁）：提交 240 时约 9.7s 后出现 toast「设置保存失败：database is locked」，输入框回落到 90，库内仍为 90——不谎报成功 |
| 4 | 列表宽度口径统一 | 校验区间与 UI 夹取一致，不再"能存但显示被夹" | **已收口，定为 280–420 单一区间**：`LIST_WIDTH_RANGE` 作为写入校验、拖拽夹取与 CSS clamp 的共同来源；写入仍严格拒绝越界（421 被拦且不落库），同步/便携带入的历史越界值在读边界 `normalizeStoredSettings` 归一为 420。实测依据见 `docs/roadmap.md` 同名条目；回归在 `tools/frontend-tests/settings-and-feeds.mjs`（D5 段两条） |

## 三、仍有效的未完成需求（来自 roadmap）

- 真实宿主事件链、真实 FreshRSS/Miniflux 部署组合未验。
- WebDAV 远端物理删除关闭；单目录 256 对象 / XML 256 KiB / actor 64 上限仍生效。
- 真实 WebView2 连续阅读、AI 自动触发真实链、换机恢复原生链未验。
- 50k distinct-body 未验证；整库 state 应用的 retention 瓶颈未修。
- 原生 Windows 安装器 / DPI / 冷启未验；真实 symlink 用例仍 `#[ignore]`。
- `docs/contracts.md` 三处事实漂移待你裁决（outbox 事务边界、`p/*` 投影值域、`url_norm` 非 UTF-8 有损）。

完整条目与验证边界见 [docs/roadmap.md](../docs/roadmap.md)；不要在本页重复维护。
