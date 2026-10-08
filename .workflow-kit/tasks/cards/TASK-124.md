<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-124 · 同步状态变化事件——四态展示在真实操作链路上生效（审计 P2-6）

**状态**：done

**目标**：修复第三方审计 P2-6（tmp/audit-20261007/REVIEW.md 第 6 节，探针 P8 场景）：①统计刷新集中在 reloadFromBackend——普通标读/收藏入队与即时推送出队没有刷新事件，探针实测本地写入后队列 1 项而 pill 仍「后端已同步」；②认证/端点构建失败在 build_client 阶段提前返回，未进 exec_push，attempts/last_error 不记录（push.rs:177 一带）。审计要求：本地事务提交、推送确认/失败后发轻量事件，前端按同一状态源更新；区分「未配置/认证失败/网络失败」，保留 lastPushSuccess 真实语义。修法要求：①Rust：a) 状态变更事务提交后（set_read/starred_with_enqueue、set_read_bulk、mark_all_read、push 成功 prune 后——复用既有 schedule_state_push 调用点附近）发轻量 Tauri 事件 sync-queue-changed，payload={waiting,failed,last_error}（直接查 sync_queue_stats，查询 <1ms 量级）；push 完成（成功/失败）后同样发。b) build_client 失败（认证/端点）在 push_states_now 与 states_phase 处捕获：发 sync-queue-changed（failed 计数照旧=attempts>0 行数）+ 可选一条 last_error 摘要（认证失败文案），或在 SyncStatusInfo 增 push_health 字段——设计由你定，理由入注释；区分「未配置（静默，不发事件不报警）」「认证失败」「网络失败」。②前端：App.tsx 监听 sync-queue-changed → 更新 store syncWaiting/syncFailed/lastError（与 reloadFromBackend 刷新点并存，同一状态源）；侧栏 pill 与 SyncTab 摘要卡自动跟随。③回归 t124-* ≥5 条（探针 P8 本体转真实行为回归）：本地标读入队→pill「等待同步 1 条」（探针本体）；推送成功出队→回到「后端已同步」；推送失败→「· 部分失败」+last_error；认证失败区分呈现；未配置静默（不报警不刷状态）。既有断言零弱化（t116-* 13 条语义保持——刷新点新增而非替换）。④Rust 测试 ≥2 条（事件 payload 正确性/认证失败标记；CI 执行）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs, src-tauri/src

## 验收标准

- ① 本地标读入队→pill「等待同步 N 条」（探针 P8 本体转回归，t124-*）
- ② 推送成功→恢复已同步态；推送失败→部分失败+last_error
- ③ 认证失败区分呈现；未配置静默（不报警）
- ④ Rust 事件 payload/失败标记测试 ≥2 条（CI 执行）
- ⑤ 门禁全绿：frontend（t124-* 新增）、lint/build/cargo_fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005；clippy 盲区按 113/121 先例静态自查+必要时的真库探针）
- ⑥ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@TASK-123 后（4f040fc）：frontend 750/750、CI 全绿。本卡为四态展示的真实链路接通，属行为增强。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：t124-* 回归（入队即时呈现/出队恢复/失败呈现/认证区分/未配置静默）+ Rust 事件测试；审计 P2-6 修复需真实链路场景锁定（DEC-gate-adjust ①）；验证：cargo_fmt, frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言（t116-* 语义保持）；不回退证据；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T14:54:52.199033Z
- 原截止时间：2026-10-07T18:54:52.199033Z
- 当前截止时间：2026-10-07T23:37:54.413157Z
- 时钟：按墙钟计：额度 300 分钟，写入阶段已用约 42 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T22:39:24.423003Z：阻塞已处置（review_failure）：CI rust job clippy -D warnings 失败：4 处 doc_lazy_continuation（push.rs:204/205/336/337，文档注释列表续行未缩进）。修复 = 注释缩进对齐，无行为变化。R0 复审已 PASS，本处置仅针对 CI 门禁复验。；下一步：begin 重新实现
- 2026-10-07T22:40:25.705296Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T22:43:44.195049Z：Worker requests manager action; inspect the result；下一步：处理执行者提出的请求，再 unblock 后 begin；不要新建任务或重置预算
- 2026-10-07T22:44:19.459826Z：阻塞已处置（action_required）：R2 修复已落地（push.rs 两处空行），local cargo fmt 通过。首次 finish 因我误把 CI 环境说明放入 unresolved_items 而 block(action_required)。重新 begin 以正确记录候选。；下一步：begin 重新实现
- 2026-10-07T22:45:23.688741Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T22:46:23.638357Z：编码结果已记录，差异范围已核对：src-tauri/src/sync/push.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T22:47:05.185545Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T22:56:58.516276Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-124.json)

- [RUN-89b898f7466e4c08816b3eb281661233](../runs/RUN-89b898f7466e4c08816b3eb281661233.json)
- [RUN-f46c49bf50b242bf89905fb2fbe58f37](../runs/RUN-f46c49bf50b242bf89905fb2fbe58f37.json)
- [RUN-4bd5c5d72be545f59d346a322d7139ab](../runs/RUN-4bd5c5d72be545f59d346a322d7139ab.json)
- [RUN-f7f1ee7aca654a88aec3f937118bb33c](../runs/RUN-f7f1ee7aca654a88aec3f937118bb33c.json)
- [RUN-ffbb4cf89dc2460e9cfec6bba18ee28d](../runs/RUN-ffbb4cf89dc2460e9cfec6bba18ee28d.json)
- [RUN-28f834d2351f4f67a8b31c30705294ba](../runs/RUN-28f834d2351f4f67a8b31c30705294ba.json)
- [RUN-b581fc12078a45989e3ee29b0952bbdb](../runs/RUN-b581fc12078a45989e3ee29b0952bbdb.json)
- [RUN-6be63f0f0b6a4a8eb79702e66613d8ce](../runs/RUN-6be63f0f0b6a4a8eb79702e66613d8ce.json)
- [RUN-c5788168f4404eceb7aa90f2297a6d8c](../runs/RUN-c5788168f4404eceb7aa90f2297a6d8c.json)
- [RUN-64854e14b4714187b47f94222e4d8ea6](../runs/RUN-64854e14b4714187b47f94222e4d8ea6.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
