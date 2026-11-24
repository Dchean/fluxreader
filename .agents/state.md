# 半成品现状与待办切片

如实记录当前停在何处。**基线**：`dev` @ `c745e8e`；本批（切片 5：outbox 入队原子性 / 设置投影写边界 / `url_norm` 非 UTF-8 边界 / 正文清理失败可见 / `listWidth` 上限 420）已本机门禁通过，待提交。本页不复制长文，只给路径引用。

## 一、当前真实停点

**切片 1–4 已收口**：v6 三层文档换轨、D8/X25 清退（`04df1a1`，独立只读审计通过：阻断 0 / 重要 0 / 次要 3，登记 `docs/audits.md`）、退订收藏闭环（`af173b3`、`d427c56`）、正文保留控件真实宿主验证、列表宽度定档 280–420（`e045d23`）。

**切片 5（本批）已完成并本机门禁通过**：

- outbox 三个入队函数（`enqueue_subscribe` / `enqueue_unsubscribe` / `enqueue_edit`）收进 `with_write_savepoint`：替换旧意图 → 清兼容 `add_feed` → 写新意图三步整体成立。回归 `t_opt009_tests::enqueue_failure_rolls_back_replaced_intent` 做过变异验证（去掉保存点即红）。
- `local_sync/database.rs` 的 `apply_projection` 对每个 `p/*` Set 复跑 `validate_setting`：非法值拒绝该键、`log::warn!` 并写诊断，不静默夹到合法范围。回归 `projection_write_boundary_rejects_invalid_setting_values`（含变异红）。
- `db/url_norm.rs` 的 `normalize_url` 对含非 UTF-8 百分号字节的 query 不重建、保留原始字节。回归 `non_utf8_percent_bytes_never_collide_and_stay_verbatim`（修前 `%FF` 与 `%FE` 同键，实测红）。
- 正文清理失败可见：连续 3 次失败登记 `retention_last_failure` 并发 `retention-error`；成功一轮清标记并发 `retention-recovered`；设置页「正文保留」卡片显示失败并可「立即重试」（`retention_health` / `retention_retry`）。`scheduler.rs` 原来只写 debug 日志的吞错改为计数 + warn + 诊断。
- `db/settings.rs` 的 `listWidth` 上限 560 → 420，与前端 `LIST_WIDTH_RANGE` 同源；同步/备份恢复写不进 500。

**门禁（本机，基线为本批工作树）**：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --no-fail-fast`（全量含集成）全部 exit 0；`npm run lint`、`npm run build` 通过。

- 本地跑 Rust 门禁时 `TEMP`/`TMP` 必须指到**工作区内可写目录**（如 `tmp/rctest`）：指到 `C:\WINDOWS\TEMP` 或会话 scratch 时，`llvm-rc` 与 `clang-cl` 会因无法创建临时文件而失败。门禁命令本身不改，属环境前置。
- CI 未运行（Actions 因额度用尽被手动禁用）；真实 WsgiDAV / FreshRSS / Miniflux 与换机原生链全未验证。

## 二、剩余工作拆为垂直切片

| # | 切片 | 能演示什么 | 状态 |
| --- | --- | --- | --- |
| 1 | 收口 D8/X25 在飞改动 | 传输层不再有旧 probe/lease/MOVE 生产路径，错误归属正确 | **已收口**（`04df1a1`，审计通过） |
| 2 | 退订收藏界面闭环 | 退订后仍能读收藏、计数正确、新设备恢复可见 | 已收口（真实宿主两轮）；**仍缺**换机恢复组合 |
| 3 | 正文保留控件真实 Edge 验证 | 默认永久/1–3650 天、原子保存、失败回滚在真实宿主可用 | **已完成**（真实 WebView2 + CDP + 写锁故障注入） |
| 4 | 列表宽度口径统一 | 校验区间与 UI 夹取一致，不再「能存但显示被夹」 | **已收口** 280–420；后端上限本批同步为 420 |
| 5 | 同步与清理的边界收口（本批） | 入队原子、写边界校验、无损归一、失败可见 | 本机门禁 + 真机验证通过（阈值、事件、提示、重试清除） |

## 三、仍有效的未完成需求（来自 roadmap）

- 真实宿主事件链、真实 FreshRSS/Miniflux 部署组合未验。
- WebDAV 远端物理删除关闭；单目录 256 对象 / XML 256 KiB / actor 64 上限仍生效。
- 真实 WebView2 连续阅读、AI 自动触发真实链、换机恢复原生链未验。
- 50k distinct-body 未验证；整库 state 应用的 retention 瓶颈未修。
- 原生 Windows 安装器 / DPI / 冷启未验；真实 symlink 用例仍 `#[ignore]`。
- 正文清理连续失败阈值、`retention-error` 事件与「立即重试」已真机验证（隔离数据根：设置非法 → 连续 4 次失败 → 卡片提示 → 重试清除标记）；真实用户库与原生安装器仍属未验范围。
- `url_norm` 的非 UTF-8 链接与设置投影的持续拒绝已登记为**已知边界**（见 `docs/roadmap.md`），不是待办。

完整条目与验证边界见 [docs/roadmap.md](../docs/roadmap.md)；不要在本页重复维护。