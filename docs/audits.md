# 审计记录

每轮审计、证伪与修复基线的结论摘要。**原始输出留在 `tmp/`（不入库）**；本页只登记可追溯的摘要、基线与复跑入口，避免机器清理后断档。规则见 [维护与文档纪律](maintenance.md) 第 2 节。

| 日期 | 范围 | 基线 | 结论 | 条目与缺口 | 证据位置 | 处置状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-11-14 | 全项目：同步协议、数据层、前端、测试工具链、WebDAV/local_sync、AI；7 轮、≥27 个子代理 | `d4a460d`（工作树干净） | **不通过**：9 条重要项待修 | 101 条（重要 9 / 次要 55 / 信息 37）+ 9 决策 + 6 已推迟 + 8 勿动 + 20 条前轮纠正；缺口：`db/outbox.rs`、`commands/sync.rs`、`commands/settings.rs` 未覆盖 | `tmp/audit-2026-11-14/最终审计报告.md`、`修复优化指导.md`、`tmp/audit-falsify-2026-11-14/`（工作底稿与证伪报告） | 待修复 |

## 修复路线

当前路线：`tmp/audit-route-2026-11-14/路线方案.md`（v2，工作文件，不入库）。约束性内容以 [维护与文档纪律](maintenance.md) §3/§9/§11 为准；路线只是排期，不替代规则。

## 复跑入口

- 前端全量状态回归：`npm run test:frontend`（2026-11-14 本机 1516/1516 通过）。
- 组件回归：`npm run test:components`（Node 24 下因 reporter 文本断言失败，见报告 N-8；Node 22 未复跑）。
- 审计条目复核方式：按报告 §1 的位置列 grep/read；静态结论的触发前提写在条目内。

## 覆盖矩阵（累计表；每轮只更新行，不整体替换）

深度口径：**逐行** = 关键路径逐函数核对；**关键段** = 只核公开入口、错误路径与边界；**结构级** = 只核模块边界与调用关系。空着的格子写"未覆盖"，不留白。

| 域 | 最后审计日期 | 基线 | 深度 | 证据/位置 |
| --- | --- | --- | --- | --- |
| 同步协议（`sync/`、`greader.rs`、`fever.rs`、`sync_http.rs`） | 2026-11-14 | `d4a460d` | 逐行 | 报告 §1.1/§1.2；四轮独立复核 |
| 数据层（`db/` 的 articles/feeds/folders/sync_map/writer/contents/backup） | 2026-11-14 | `d4a460d` | 逐行（核心）/关键段（备份） | 报告 §1.1/§1.2 |
| 数据层（`db/outbox.rs`、`db/body_retention.rs`、`db/url_norm.rs` 全量） | 2026-11-14 | `d4a460d` | 未覆盖 / 抽样 | 仅核过入口与注释 |
| 前端 store 与关键组件（internals、slices、Timeline、Overlays、Reader、api） | 2026-11-14 | `d4a460d` | 逐行 | 报告 §1.2 前端条目 |
| 前端（`PlayerBar`、`GalleryLayout`、`useProseImages`、`imageProxy` 全量） | 2026-11-14 | `d4a460d` | 结构级 | 未逐行 |
| 测试工具链（`tools/` 的 runner、断言框架、夹具；`ui-layout` 中段） | 2026-11-14 | `d4a460d` | 逐行（框架）/结构级（`ui-layout` 中段） | 报告 §1.2/§1.3 |
| WebDAV 与本地同步（`webdav_transport.rs`、`local_sync/`） | 2026-11-14 | `d4a460d` | 关键段（新增一轮） | 报告 §2 N-1；本轮才补 |
| AI 全链路（`ai.rs`、`ai_segments.rs`、`commands/ai.rs`、AI store） | 2026-11-14 | `d4a460d` | 逐行（新增一轮） | 报告 §1.2 的 X 条目 |
| 调度器与抓取（`scheduler.rs`、`ingestion/`） | 2026-11-14 | `d4a460d` | 结构级 | 只核了节奏与收体边界 |
| 命令层（`commands/settings.rs`、`commands/sync.rs` 全量） | 2026-11-14 | `d4a460d` | 未覆盖 | 只核了被引用的函数 |
| 文档与注释一致性 | 2026-11-14 | `d4a460d` | 抽样（发现 20 条纠正） | 报告 §6 |

**结论**：不是"全覆盖"。下一轮应按"未覆盖 + 最久未碰"排序补齐，而不是重跑已逐行的域。

### 下一轮覆盖优先级

1. `db/outbox.rs`、`commands/sync.rs`、`commands/settings.rs`（未覆盖，且是订阅写回与设置写入的主干）。
2. `ingestion/staged.rs`、`db/body_retention.rs`（正文保留与暂存，和 N-1/X-26 的对象生命周期相关）。
3. 前端 `PlayerBar`、`GalleryLayout`、`imageProxy` 全量（只做过结构级）。
4. 文档全量梳理（路线见 `tmp/audit-route-2026-11-14/`）。
