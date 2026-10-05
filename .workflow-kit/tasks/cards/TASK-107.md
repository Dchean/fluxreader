<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-107 · 全部已读与标读计数一致性——按后端实际影响数对账、失败不假成功（REQ-003）

**状态**：done

**目标**：修复外部审计探针复现的计数缺口（AUDIT-20261005-core-consistency.md）：①src/store/slices/nav.ts markCurrentViewAllRead 乐观调用 markEntriesRead(ids) 只对已加载条目扣计数，而后端 apply_mark_all_read 处理整个范围（探针：范围 600 条未读、前端加载 1 条，后端成功后界面仍显示 599 条未读）。api.markAllRead 已返回实际标读条数（usize）但前端忽略返回值。修法要求：成功路径以返回条数校验并以 api.feedCounts() 重取全量计数（与 reloadFromBackend 同一计数来源），替代「只按已加载条目推算」；本地已加载条目的已读态允许乐观先行但失败必须回滚（保存原 read 态，catch 恢复并 toast），不允许「计数已扣、状态已改」的假成功；范围语义（feed/folder/starredOnly/sinceMs/layout）不动。②单条标读与计数一致性核查（审计「标读后数字、显示不一致」缺口）：核查 setRead/markEntriesRead 路径对 feedCounts.unread 的扣减与后端口径是否一致——含同文副本去重场景（本地主条目标读、副本计数归属）与跨布局不可见条目；发现不一致即修复并以断言锁定，确认一致的路径在 progress 写明核查结论，不得静默跳过。③t104-* 断言 ≥4 条：全部已读成功后计数=后端口径（600/1 探针场景转断言）、失败回滚不假成功、单条标读计数一致、范围外布局计数不受影响。coder 开工前先用探针/测试实证单条路径现状，据实决定修复面。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 全部已读成功后计数以后端为准：600 未读/加载 1 条场景，后端成功 600 后界面未读=0（不再残留 599）（t104-markall-count-authoritative）
- ② 失败不假成功：markAllRead 拒绝/异常时本地已读态与计数回滚，toast 可见（t104-markall-failure-rollback）
- ③ 单条标读计数一致：标读后 feedCounts 与实际状态一致（含同文副本不重复扣减），核查结论逐路径入 progress（t104-single-read-count）
- ④ 范围可解释：当前范围全部已读不影响其他范围/布局的计数（t104-scope-isolation）
- ⑤ 门禁全绿不回退：frontend 全过（t104-* 新增）、lint/build exit 0、cargo test/fmt/clippy 不回退（cargo test/clippy 由 CI rust job 承担：DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@30cd2bc（v0.16.2 后）：外部审计实测 frontend 527/527；cargo 本机预检通过。本卡修复计数对账与失败回滚，属行为修复。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005, DEC-local-cargo-gate-20261005
- 补充：t104-* 断言（计数以远端为准、失败回滚、单条一致、范围隔离）；审计探针场景转断言，防回退；验证：frontend
- 保留：既有 frontend/cargo/fmt/clippy/lint/build 断言；不回退证据；cargo test/clippy 由 CI rust job 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-05T08:39:08.986075Z
- 原截止时间：2026-10-05T12:39:08.986075Z
- 当前截止时间：2026-10-05T12:39:08.986075Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 90 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-05T10:27:42.270421Z：编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T10:28:11.219907Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-05T10:45:48.243130Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-05T10:46:00.477508Z：阻塞已处置（review_failure）：R1 审查 PASS 附 3 条低危：#2 mergeSnapshotEntries bump 无断言覆盖是真实缺口（R2 补断言）；#1/#3 为自愈型角例（双失败窗口、affected=0 短路），修之恐引入更差行为，按 findings=可执行缺陷口径交下轮审查裁定，分析保留在报告 summary 供 owner 查阅。修复轮 2/4。；下一步：begin 重新实现
- 2026-10-05T10:46:35.595039Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T11:15:11.037331Z：编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T11:15:52.368523Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-05T11:40:03.108222Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-107.json)

- [RUN-adcdaa59ef97462a88a84feaa8aa77d3](../runs/RUN-adcdaa59ef97462a88a84feaa8aa77d3.json)
- [RUN-8e7605b3dc36498cb7f5a734ddbf6f7d](../runs/RUN-8e7605b3dc36498cb7f5a734ddbf6f7d.json)
- [RUN-957cf2cab1424c1c8738ad7f052b722c](../runs/RUN-957cf2cab1424c1c8738ad7f052b722c.json)
- [RUN-fe014c746ccb4c9e92aa1e9044bb7ff0](../runs/RUN-fe014c746ccb4c9e92aa1e9044bb7ff0.json)
- [RUN-1b42a07bda04456fa2f416a18e769d21](../runs/RUN-1b42a07bda04456fa2f416a18e769d21.json)
- [RUN-92a428e81d2741a8b32a35d91d612881](../runs/RUN-92a428e81d2741a8b32a35d91d612881.json)
- [RUN-b56aea5e4bdb46638cddb0c1189562f1](../runs/RUN-b56aea5e4bdb46638cddb0c1189562f1.json)
- [RUN-01d0dfb259214dfa81cb0526bad200ad](../runs/RUN-01d0dfb259214dfa81cb0526bad200ad.json)
- [RUN-fa8f9caac50842b193d9385f18b2c054](../runs/RUN-fa8f9caac50842b193d9385f18b2c054.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
