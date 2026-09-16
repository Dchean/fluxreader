# 当前交接

状态入口：[PROJECT](../tasks/PROJECT.json)、[POLICY](../tasks/POLICY.json)、[DECISIONS](../tasks/DECISIONS.json)。
任务状态只从 `.workflow-kit/tasks/items/` 读取；本文件不另抄一套通过状态与预算数值。

## 当前状态（2026-09-16，本地 09-17）

- 阶段：`delivery`（分步实施）。活动批次 `BATCH-7cbc4d36369a4c4d8f5a7dcfeac9f898`，含 TASK-041、TASK-042。
- 14 张任务卡：**13 张已 done**（TASK-029..041），TASK-042 尚在「修复 → 验证 → 审查」循环内。
- 本地 `main` 领先 `origin/main` 2 个提交（`fb46404` REQ-005 实现、`22bb1a7` TASK-042 交接）；
  POLICY `push = "ask"`，**未推送**。
- 测试界面按 owner 要求只出现在右侧屏幕（屏幕 2）。

## TASK-042（REQ-005 交互动画）——唯一未收口项

1. 第 1 轮实现（CSS 与 JS 双侧、`.list-entering` 死代码修复、`prefers-reduced-motion`）。
2. 独立审查 FAIL（3 条）→ 第 1 轮修复轮（播放消失方向、下拉菜单 from 态、AI/通知 display 过渡）。
3. 该修复轮的 `finish` 因 **worker 声明漏列被删除的 `src/hooks/useEnteringClass.ts`** 被判
   `failure_kind=protocol`；工具对 `protocol` 没有恢复入口，详见
   [工具缺口：protocol 无恢复入口](TOOL-GAP-protocol-recovery.md)。
4. owner 授权**台账订正 #1**：只补全声明并闭合该 run，源码与候选未动；订正前后候选摘要一致
   （`f3f07446…`）。
5. `verify` 通过 → 第 2 轮独立审查 FAIL（社交/通知译文块仍是 display 硬切；设置页分类分组
   展开无过渡且交互报告断言不实；`useEnteringClass` 的 `animationend` 未校验 `target`）。
6. 第 2 轮修复完成 → `finish` → `verify` 通过（候选 `059880b6…`）→ 第 2 轮独立审查**结论 PASS**，
   但记录时触发
   [工具缺陷：审查证据自锁](TOOL-GAP-review-evidence-self-lock.md)——报告把
   `.workflow-kit/tasks/items/TASK-042.json` 列为证据，而工具在算完摘要后自己改写该文件。
7. owner 授权**台账订正 #2**：恢复审查阶段状态，并让审查者在不改结论、不重跑门禁的前提下
   重发报告（仅移除该台账路径）。当前：**待用修正报告重新记录审查**，随后 commit 与验收。
8. 预算：`DEC-84488ccd38b2403599258ae3cf3e24f6` 追加 180 分钟，截止 `2026-09-16T19:40:24Z`；
   累计修复轮 2（上限 7 = 策略 4 + 扩展 3）。

## 未完成

- TASK-042：用修正后的报告记录审查（`review`）→ commit → owner 验收（`accept`）。
- **模块拆分尚未开工**：`src-tauri/src/commands.rs` / `sync.rs`、`src/components/SettingsModal.tsx`
  与 `src/store.ts` 的域拆分（沿用 `db.rs` 试点模式）。
- REQ-001..REQ-008 在 REQ 层的最终验收汇总。
- 运维待办：轮换 `test` 账号在后端的密码——历史重写只能让数据不再公开可见，不能收回已发布内容。
- 仓库卫生：根目录 `.mimosa/`（约 4.2MB）是宿主会话钩子状态，属机器本地数据，**不要提交**。
  本次未改 `.gitignore`：TASK-042 仍可能再进修复轮，改非 `src/**` 文件会触发 out-of-scope 拦截。

## 两个已记录的工具缺口（都给流程留下过死锁）

- [protocol 无恢复入口](TOOL-GAP-protocol-recovery.md)：worker 声明与观测差异不一致时，
  `block` 写下 checkpoint 后 `finish` 重跑在结构上已不可达。
- [审查证据自锁](TOOL-GAP-review-evidence-self-lock.md)：审查报告引用 `tasks/items/**` 时，
  工具会先绑定再改写该文件，必然自我失效。

## 恢复流程

见 [工作流交接规则](workflow/HANDOFF.md)。接手时先核对全体相关 run 与实际差异，
不要回退或重新实现已完成的修改。
