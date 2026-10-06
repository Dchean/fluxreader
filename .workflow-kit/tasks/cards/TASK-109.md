<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-109 · 查询口径统一收口 + merge bump 按真值来源收窄（二阶段①，承接审计块②）

**状态**：done

**目标**：二阶段第一卡：查询口径统一与水合版本守卫的来源精确化。①口径收口：现况三把键维度不对称——scopeQueryArgs（范围×排序×可选 layout，internals.ts:126-138）、scopePageKey（范围×layout，:145-147）、viewCacheKey（layout×view×scope，:90-92）分散定义、多处裸拼；探查实证三处不一致：bootstrap.ts:143 的 scopeKey 在 await 之后读「完成时」状态（注释声称发起时口径，靠 reloadGeneration 间接兜底）、loadMoreArticles 守卫含 sortAtStart 而 reloadFilteredEntries 明示不含排序（bootstrap.ts:291，筛选视图拉全集的历史语义）、markCurrentViewAllRead（nav.ts:218-220）传 layout 却只取 feed/folder。修法要求：新建统一派生入口（如 internals 内 QueryScope 助手或独立模块），三把键与查询参数只从它派生；scopePageKey/viewCacheKey 的字符串形态不得改变（缓存键兼容，避免无谓失效）；上述三处不一致收口为显式命名谓词/参数（如「筛选视图不锁排序」的具名守卫构造器），行为保持但不再靠注释默会；bootstrap.ts:143 改为发起时快照（与 bootstrap.ts:196/267/325 一致），reloadGeneration 守卫保留。②merge bump 收窄（TASK-107 R2 审查裁定的后续微任务，todo 备忘在案）：mergeSnapshotEntries（internals.ts:185-225）增加真值来源参数（如 fromBackend），bootstrap 三处后端快照路径（reloadFromBackend:138/reloadFilteredEntries:296/anchorToArticle:354）传真——bump 生效使在途乐观声明失效；nav 三处缓存恢复（selectLayout:74/selectView:117/selectFeed:162）传假——缓存回放是近期 UI 状态而非后端真值，不 bump 以保留本应正确的在途回滚。R2 审查已确认该修法技术成立（经 reload 落进缓存的行在 bootstrap merge 时已 bump，nav 不 bump 不会重开踩踏缺口；唯一无 bump 的缓存行恰是乐观态本身）。③回归断言 t109-* ≥5 条：口径派生单点性（源级）、发起时快照修正、bump 收窄的三个方向（缓存恢复后在途回滚仍正确恢复——即 R2 裁定角例修后行为、后端快照仍 void 陈旧声明（t104-snapshot-voids-rollback-claim 必须保持通过）、缓存回放不误 void 新声明）、键形态兼容。既有断言零弱化。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 口径单点：三把键与查询参数从统一派生入口产生，三处不一致以显式命名收口（t109-* 源级断言锁定）
- ② bootstrap.ts:143 改为发起时范围快照，reloadGeneration 守卫保留，行为兼容（断言锁定）
- ③ merge bump 收窄：后端三路径 bump、缓存三路径不 bump；t104-snapshot-voids-rollback-claim 等既有断言全过；R2 角例（缓存恢复后在途回滚）修后行为有判别断言
- ④ scopePageKey/viewCacheKey 字符串形态不变（断言锁定键形态）
- ⑤ 门禁全绿：frontend 全过（t109-* 新增）、lint/build exit 0、cargo fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@b93c29f（一阶段验收后）：frontend 561/561、CI rust job cargo test+clippy 全绿（dev@8c1501f 运行）、lint/build/cargo_fmt 本地门禁绿。本卡为口径收口+守卫来源精确化，用户可见行为保持。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：t109-* 断言（口径单点、发起时快照、bump 三方向、键形态兼容）；收口与守卫语义需防回退；验证：frontend
- 保留：既有 frontend/cargo fmt/lint/build 断言（含 t103-*/t104-* 全部）；本地不回退证据；cargo test/clippy 由 CI rust job 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-05T13:30:27.164576Z
- 原截止时间：2026-10-05T17:30:27.164576Z
- 当前截止时间：2026-10-05T17:30:27.164576Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 29 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-05T13:30:27.854419Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T13:59:37.854958Z：编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T14:00:10.763567Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-05T14:11:39.216017Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-109.json)

- [RUN-4ebf8efd86894bc299c3b0269e3b0264](../runs/RUN-4ebf8efd86894bc299c3b0269e3b0264.json)
- [RUN-0268d4be749142a5a10eb5a7331d564a](../runs/RUN-0268d4be749142a5a10eb5a7331d564a.json)
- [RUN-2b950889f0f546358dd561d1c88f5809](../runs/RUN-2b950889f0f546358dd561d1c88f5809.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
