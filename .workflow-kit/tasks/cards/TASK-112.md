<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-112 · 协议对账冲突政策显式化 + 服务端×协议兼容矩阵文档（二阶段④，完成标准「不同协议行为有明确说明」）

**状态**：done

**目标**：把协议间对账差异从「实现分支里的隐性事实」提升为显式冲突政策并文档化（审计二阶段完成标准；行为零变化）。现况事实（探查在案）：GR 轻量对账 reconcile_reader_state（src-tauri/src/sync/greader_pull.rs:247-276）读状态单向 read-wins（不反向复活未读）、starred 双向；Fever reconcile_fever_state（fever_pull.rs:186-218）读状态双向（unread 集合命中即恢复未读，前提注释 fever_pull.rs:180-182「Miniflux 按 URL 去重、Fever 视角无跨源副本」）；两侧共享 pending 保护（pending_ids 命中即跳过）与「失败≠空集合」守卫（GR :164-182、Fever reconcile_ok :53-62/159-162）；增量游标：GR 起点候选游标+幂等合并（greader_pull.rs:60-67、189-198，REQ-103 失败守卫），时间过滤列随服务端实现而异（mock_greader 为 changed_at>=ot，真实 Miniflux main 映射 published_at 严格大于——greader_pull.rs:60-64 注释在案）；Fever last_id 按已合并条目 max 推进（fever_pull.rs:145-151）。修法要求：①政策显式化：新增单一政策定义点（建议 src-tauri/src/sync/conflict_policy.rs：每个「操作×协议」一条政策常量/函数+文档注释，说明选择理由与历史依据），reconcile_reader_state/reconcile_fever_state 引用政策定义（行为逐列不变——这是收口不是改行为；diff 必须可证明行为零变化）；同文副本读状态传播政策（DEC-refactor-roadmap 第 6 条：保持广播现状）一并写入政策点注释。②新增 docs/sync-compat-matrix.md（仓库根新建 docs/ 目录）：面向维护者/用户的兼容矩阵——协议（GR/Fever）×操作（标读/取消标读/收藏/取消收藏/对账方向/增量游标语义/时间过滤列）差异表；服务端（FreshRSS/Miniflux）已知实现差异（引用 greader_pull.rs:60-64 既有注释事实，注明「外部源码来自 dev 分支查阅时点，不代表部署服务版本」）；冲突政策表（哪个协议哪个方向单向/双向+理由）；同文副本传播政策；已知限制（游标变更时间假设不能单独保证晚到旧文章——审计结论）；不写教程性冗余。③修正过时注释：db/articles.rs:186「前端 ARTICLES_PAGE_SIZE=100000」与前端实际 500 不符（100000 是 TASK-110 废除前的 reloadFilteredEntries 字面量），按当前事实改写。④测试：核查 GR 单向/Fever 双向、pending 保护、失败守卫的既有测试覆盖（探查显示 sync_phases_e2e/pull_cursor_e2e/star_reconcile_truncation_e2e 等；缺哪条补哪条 ≥1 条）；cargo test 由 CI 执行。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src, docs

## 验收标准

- ① 政策单点：GR/Fever 对账差异在单一政策定义点显式化，reconcile 函数引用之，行为零变化（diff 逐列可证 + 既有 e2e 全过）
- ② docs/sync-compat-matrix.md 上线：矩阵含协议×操作差异、服务端实现差异（带时点声明）、冲突政策表、副本传播政策、已知限制
- ③ db/articles.rs:186 过时注释修正
- ④ 对账方向的测试锁定齐备（缺补 ≥1 条）；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑤ 门禁全绿：cargo fmt、lint、build、frontend 不回退
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@二阶段③后基线：frontend 全绿、CI rust job 全绿。本卡行为零变化（政策显式化+文档+注释修正），新测试仅在 CI 执行。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：对账方向政策锁定测试（如缺）；政策显式化需有测试锚；执行器为 CI rust job 的 cargo test（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend
- 保留：既有 e2e/单元测试与本地四门禁；行为零变化的证明；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-06T03:04:44.594082Z
- 原截止时间：2026-10-06T07:04:44.594082Z
- 当前截止时间：2026-10-06T07:04:44.594082Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 26 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-06T03:04:45.233047Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-06T03:31:27.001535Z：编码结果已记录，差异范围已核对：docs/sync-compat-matrix.md, src-tauri/src/db/articles.rs, src-tauri/src/sync/conflict_policy.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/mod.rs, src-tauri/src/sync/push.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-06T03:31:57.997937Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-06T03:53:44.191889Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-112.json)

- [RUN-aeb2ae41525347e0b4f703ccb4b3fff3](../runs/RUN-aeb2ae41525347e0b4f703ccb4b3fff3.json)
- [RUN-027e6fd907e84c71ad747ae527b0c7eb](../runs/RUN-027e6fd907e84c71ad747ae527b0c7eb.json)
- [RUN-e509e13d0b884268b9fee0a4c65a7c7b](../runs/RUN-e509e13d0b884268b9fee0a4c65a7c7b.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
