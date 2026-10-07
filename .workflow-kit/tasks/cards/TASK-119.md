<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-119 · 请求与计数过期覆盖——迟到响应不得覆盖新状态（审计 P2-4）

**状态**：done

**目标**：修复第三方审计 P2-4（tmp/audit-20261007/REVIEW.md 第 4 节，探针 probes.mjs P5/P6 场景）：①同 scope/view/sort 两次筛选请求，新请求先返回、旧请求后返回，最终列表回到旧结果——现守卫只比较查询参数（QueryScope 收口了谓词但没消除同查询旧版本窗口，A→B→A 也不设防）。②全部已读成功后请求计数（快照 0）→ 用户改回未读 → 旧计数响应返回整体替换 → 文章未读但计数 0（新对账链路引入的窗口）。审计要求：每个查询实例维护 requestId/generation，首屏、续页、刷新、导航共享同一套过期判断；计数使用请求版本并与待确认操作合并。修法要求：①查询实例代际：模块级 Map<queryKey, generation>（queryKey=pageKey×视图口径——复用 TASK-117 的键族，设计说明入注释）；每次发起（reloadFromBackend/reloadFilteredEntries/loadMoreArticles 首页/anchorToArticle）bump 对应 queryKey 代际并随响应携带；落地仅当代际==现值才应用，否则丢弃；**统一收口**——reloadGeneration 既有模块级计数与新机制合并（避免双代际并存语义打架；loadMore 续页携带其首页代际，续页响应只允许 ≥ 首页代际且 scopeKey/游标三元组守卫保留）。②计数过期：reconcileCounts 请求发起时记录序号（或代际），落地仅当「期间无本地读/藏写入」才应用（pendingOps 计数器：任何 flipEntryFlag/markEntriesRead/Bulk/markCurrentViewAllRead bump），否则丢弃并再请求一次（一次即可，仍过期则放弃依赖下次 reload——注释说明）。③回归 t119-* ≥5 条（探针场景转真实行为回归）：A→B→A 旧响应丢弃；同查询旧版本先发后至不覆盖；计数竞态（改回未读后旧计数不应用且重取一次）；续页旧代际丢弃；守卫统一后既有场景（reloadGeneration 保护的范围切换）仍绿。既有断言零弱化（reloadGeneration 相关断言按统一语义更新附理由）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① A→B→A 旧响应丢弃（探针场景转回归，t119-*）
- ② 同查询旧版本先发后至不覆盖新结果
- ③ 计数竞态：期间有本地写则旧计数不应用且重取一次
- ④ 续页旧代际丢弃；统一后既有 reloadGeneration 保护场景不回退
- ⑤ 门禁全绿：frontend（t119-* 新增）、lint/build/cargo_fmt 不回退；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@feae9cd（TASK-118 后）：frontend 685/685、CI 全绿。本卡修复审计 P2-4（迟到响应覆盖），属正确性修复。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：t119-* 迟到响应回归（A→B→A/同查询旧版本/计数竞态/续页代际）；审计 P2-4 修复需操作序列场景锁定（DEC-gate-adjust ①）；验证：frontend
- 适配：reloadGeneration 相关既有断言；统一进查询代际机制，语义等价、实现形态变化，更新附理由；验证：frontend
- 保留：其余既有 frontend/cargo fmt/lint/build 断言；不回退证据；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T05:30:55.674119Z
- 原截止时间：2026-10-07T09:30:55.674119Z
- 当前截止时间：2026-10-07T09:30:55.674119Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 60 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T05:30:56.285614Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T06:31:08.770859Z：编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T06:31:47.221066Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T06:52:40.888438Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-119.json)

- [RUN-99881df8ef684304a72f208322dbc6ed](../runs/RUN-99881df8ef684304a72f208322dbc6ed.json)
- [RUN-5880f0b894da46d48586c230cf7709bc](../runs/RUN-5880f0b894da46d48586c230cf7709bc.json)
- [RUN-dfe3d62ba96747d3ac194b649235d62b](../runs/RUN-dfe3d62ba96747d3ac194b649235d62b.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
