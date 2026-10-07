<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-118 · 跨字段操作版本与回滚统一——收藏不再使读状态回滚失效（审计 P1-2）

**状态**：done

**目标**：修复第三方审计 P1-2（tmp/audit-20261007/REVIEW.md 第 2 节，探针 probes.mjs P2）：entryMutationVersion 是文章级共享版本，收藏（flipEntryFlag isStarred）与标读共用同一版本——全部已读在途时用户收藏该文，收藏 bump 版本使读状态回滚被跳过（审计实测：应 isRead=false/unread=1，实际 isRead=true/isStarred=true/unread=0）。修法要求：①版本键改 articleId×field（isRead/isStarred 各自单调，flipEntryFlag 按旗标类型 bump 对应字段；markEntriesRead bump isRead；mergeSnapshotEntries 后端行级真值两字段都 bump——快照整体替换使两字段在途声明都失效，语义注释说明）；nav.ts markCurrentViewAllRead 失败回滚只查 isRead 字段版本。②回滚统一（审计相邻缺口：『所有乐观写入都有统一版本回滚』尚未成立）：a) optimisticEntryFlagToggle 值比较改版本守卫（同一协调器）；b) markEntriesReadBulk（滚动批量标读）失败增加回滚——失败时按版本守卫恢复被翻转行并回补计数（对照 markCurrentViewAllRead 的回滚形态）；c) 打开即标读（selectArticle 的 markReadOnOpen→set_read）失败增加回滚（现仅 toast）。三路径共用同一回滚助手（恢复读态+按行回补 unread），单条与批量不得各写一份。③失败注入回归 t118-* ≥5 条（审计探针场景转真实行为回归，DEC-gate-adjust 纪律）：全部已读在途+收藏+失败 → 读状态恢复且收藏保留（探针本体）；跨字段版本隔离真值表；bulk 中途失败回滚；打开即标读失败回滚；连续两次全部已读的版本快照正确性。④Rust 零改动（纯前端）；不改 keyset 分页（TASK-117 刚落地）。既有断言零弱化（t104-rollback-guard-versioned 两断言锁的守卫升级为 per-field 后语义仍须成立——更新附理由）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs

## 验收标准

- ① 全部已读在途+收藏+失败：读状态恢复未读、收藏保留（审计探针本体转回归，t118-*）
- ② 跨字段版本隔离：isRead/isStarred 版本互不干扰（真值表断言）
- ③ 回滚统一：单条 toggle/批量标读/打开即标读三路径失败都走同一回滚助手（行为断言）
- ④ mergeSnapshotEntries 两字段 bump 语义保持（t104-snapshot-voids-rollback-claim 原样过）
- ⑤ 门禁全绿：frontend（t118-* 新增）、lint/build/cargo_fmt 不回退；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑥ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@b7947e9（TASK-117 后）：frontend 671/671、CI 全绿。本卡修复审计 P1-2 正确性缺陷（跨字段版本共享使回滚失效），属行为修复。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：t118-* 操作序列回归（探针本体/跨字段真值表/bulk 回滚/打开即标读回滚）；审计 P1-2 正确性修复需操作序列场景锁定（DEC-gate-adjust ①）；验证：frontend
- 适配：t104-rollback-guard-versioned 等锁文章级版本语义的断言；版本键升级为 per-field，守卫语义保持、实现形态变化，更新附理由；验证：frontend
- 保留：其余既有 frontend/cargo fmt/lint/build 断言；不回退证据；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T04:19:13.299854Z
- 原截止时间：2026-10-07T08:19:13.299854Z
- 当前截止时间：2026-10-07T08:19:13.299854Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 41 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T04:19:14.200107Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T05:00:31.728042Z：编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T05:01:08.738451Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T05:26:25.563606Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-118.json)

- [RUN-41ef013eb61e4569b6617d0f7800b473](../runs/RUN-41ef013eb61e4569b6617d0f7800b473.json)
- [RUN-7509ff497f1349129793937b1470f602](../runs/RUN-7509ff497f1349129793937b1470f602.json)
- [RUN-8f5290f4a25349f5921f6b56700f8ff5](../runs/RUN-8f5290f4a25349f5921f6b56700f8ff5.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
