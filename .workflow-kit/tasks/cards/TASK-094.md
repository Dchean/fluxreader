<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-094 · 三布局列表可达性：列表查询加布局维度 + 不足一屏自动续拉 + 可执行的加载提示（REQ-107）

**状态**：verified

**目标**：修 REQ-107（基线 .workflow-kit/tasks/evidence/TASK-094-baseline-reachability.md）：画廊/播客/通知布局在后端仍有内容时列表不可滚动、用户到不了条目，且提示「滚动加载更多」不可执行。根因：list_articles/article_index 的查询没有布局维度，后端按全局分页，前端再按布局过滤，稀疏布局首批只剩几条撑不满容器，onScroll 不触发；空列表补拉仅在 items 为 0 时生效。方向（主控确定）：① 后端 ArticleListArgs 增加可选 layout 字段（snake_case，与前端一致），list_articles 与 article_index 复用 mark_all_read / list_unread_ids_scoped 中已审过的同一段布局谓词（抽为一处共享 SQL 片段或 helper，三处共用，避免第四次复制）；② 前端 scopeQueryArgs 传 layout，分页游标键与视图缓存键都含布局（保证 R7）；③ 兜底：列表非空但未撑满视口且未到底时自动续拉（有上限，防止死循环），不可滚动时哨兵改为可点击「加载更多」；④ 补 Rust 与前端断言（修前失败、修后通过）并用真机 harness 取证。

**依赖**：TASK-093
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-107-REACHABILITY.md
**界面检查**：R1.gallery-reach-all, R2.podcast-reach-all, R3.notification-reach-all, R4.article-social-unchanged, R5.hint-executable, R6.empty-state-consistent, R7.layout-switch-cursor
**修改范围**：src/**, src-tauri/src/**, src-tauri/tests/**, tools/frontend-regression.mjs

## 验收标准

- ① 后端：list_articles / article_index 支持可选 layout，谓词与 mark_all_read 同源（一处定义）；layout=None 时行为逐字不变；Rust 测试覆盖 feed 级覆盖、分类兜底、layout=None 三种情形与 article_index 位置对齐
- ② 前端：请求带 layout，游标键与缓存键含布局；切换布局不串游标（断言：各布局首批 offset=0，切回不重复不跳页）
- ③ 兜底：列表未撑满视口且未到底时会继续加载直到撑满或到底（有调用上限）；不可滚动时提示可点击且点击会加载；真到底显示「没有更多了」
- ④ 真机 UI 证据：用 tmp/audit-r3/harness 复现修前 s7b B1 数据形态，修后画廊/播客/通知三布局渲染卡片数 == 后端该布局条目数，文章/社交不回退；截图与交互报告
- ⑤ 门禁全绿且不回退：cargo test 通过数 ≥ 依赖任务验证后的数目、0 failed、ignored 不增；fmt/clippy/lint/build exit 0；frontend 通过数不少于依赖任务验证后的数目且全部通过

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：FAIL；门禁全绿，但真机 s7b B1 实测画廊 5/44、播客 6/119、通知 1/20 且容器不可滚动（取证时含已修复的 P0-1，须在当前代码重新复现）；既有测试对「按布局分页」与「不足一屏续拉」零覆盖。
- 基线证据：.workflow-kit/tasks/evidence/TASK-094-baseline-reachability.md
- 需求决定：DEC-next-batch-covers-reachability-20260923
- 保留：src-tauri 既有测试（含 ArticleListArgs snake_case 反序列化、mark_all_read_layout_isolation）；layout=None 行为不变；mark_all_read 语义不变（只是谓词改为共享）；验证：cargo_test
- 补充：Rust：list_articles/article_index 的 layout 过滤（feed 覆盖/分类兜底/None）与位置对齐；新查询维度需要成对断言；验证：cargo_test
- 补充：前端：请求带 layout、游标与缓存键含布局、不足一屏续拉有上限、哨兵可点击；REQ-107 验收；验证：frontend
- 保留：前端既有断言、lint、build、fmt、clippy；不回退证据；验证：frontend, lint, build, cargo_fmt, cargo_clippy

## 执行与恢复

- 首次开始：2026-09-28T08:06:42.920516Z
- 原截止时间：2026-09-28T12:06:42.920516Z
- 当前截止时间：2026-09-28T12:06:42.920516Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 113 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T08:06:43.488662Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T10:00:14.024056Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/articles.rs, src-tauri/src/db/articles.rs, src-tauri/src/db/dedup_tests.rs, src-tauri/src/db/sync_map.rs, src-tauri/tests/ingestion_e2e.rs, src/components/Timeline.tsx, src/components/timelineRefill.ts, src/lib/api.ts, src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T10:00:32.115399Z：Required gate failed: cargo_test；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-28T10:01:36.651645Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T10:16:12.268488Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-094.json)

- [RUN-eb0c044d87714f63af7a344b424ada18](../runs/RUN-eb0c044d87714f63af7a344b424ada18.json)
- [RUN-fb141ccb6b6c46058f675e32e6c5f92a](../runs/RUN-fb141ccb6b6c46058f675e32e6c5f92a.json)
- [RUN-659123b4e2ea4c1ba8725a34cda6d2d6](../runs/RUN-659123b4e2ea4c1ba8725a34cda6d2d6.json)
- [RUN-d950656abe0943d984d27810525ea8ca](../runs/RUN-d950656abe0943d984d27810525ea8ca.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
