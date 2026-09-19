<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-040 · 前端缺陷批一：按 id 摘要态（F4）+ 搜索打开标读（F7）+ 全部已读视图口径（F8）+ 搜索竞态（F20）

**状态**：done

**目标**：修复四个已定位缺陷。(1) F4：summaryGenerating 是全局单布尔，为任一文章生成摘要时所有空摘要卡片同时显示「正在生成…」；改为按 id 的 summarizingIds 集合，卡片与 Reader 按自身 id 判定。(2) F7：anchorToArticle（搜索/命令面板打开）不执行「打开时标已读」，与列表点开行为分叉；改为复用 selectArticle 的 markReadOnOpen 逻辑。(3) F8：markAllRead 只传订阅范围，在收藏/今天视图下会把范围内全部文章（含未显示的）标已读，与 toast 文案不符；后端 mark_all_read 增加 view 过滤参数（all/unread/today/starred），前端传入当前视图。(4) F20：Overlays 搜索结果 Promise 无代际守卫，慢查询返回可覆盖新查询结果；加 alive/代际守卫。

**依赖**：TASK-039
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/**, tools/**, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- 摘要生成中状态按 id 隔离（新增断言：A 生成中不影响 B 卡片判定）
- anchorToArticle 打开文章后按 markReadOnOpen 标已读（新增断言）
- mark_all_read 支持视图过滤：收藏视图只影响收藏文章（Rust 测试或新断言覆盖）
- 搜索结果竞态有代际守卫（代码路径 + 审查核对）
- lint 0/0、前端回归与 cargo test 全绿

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线：lint 0/0、前端回归 21/21、cargo test 119/0。行为变更仅限四处缺陷修复（按 id 摘要态、搜索打开标读、全部已读视图口径、搜索竞态守卫），均为 FINDINGS-REQ-007.md 已确认缺陷（REQ-007 范围）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-defect-batch2-20260915
- 保留：既有 21 项前端回归与 Rust 全量测试；不得回归；验证：frontend, rust-test
- 补充：按 id 摘要态与搜索打开标读断言；F4/F7 此前无覆盖；验证：frontend

## 执行与恢复

- 首次开始：2026-09-15T18:07:10.832732Z
- 原截止时间：2026-09-15T22:07:10.832732Z
- 当前截止时间：2026-09-16T05:41:23.579006Z
- 时钟：按墙钟计：额度 480 分钟，写入阶段已用约 325 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T18:07:10.913006Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T23:30:33.158403Z：Out-of-scope changes: .worker-result-040.json；下一步：先核对已有文件及原始日志，再处理 scope；不要新建任务或重置预算
- 2026-09-16T01:41:24.097948Z：依据新决定追加预算；原始时钟与失败记录保留；下一步：先核对已有成果，再按原任务范围继续
- 2026-09-16T01:50:42.653946Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-16T01:52:42.239130Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-16T02:07:32.877881Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-16T02:21:01.000867Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-040.json)

- [RUN-7f8179e9998644c6b7e7b94d6b40c0e7](../runs/RUN-7f8179e9998644c6b7e7b94d6b40c0e7.json)
- [RUN-c31ac3ef851841159bce712f63a7ab74](../runs/RUN-c31ac3ef851841159bce712f63a7ab74.json)
- [RUN-5cbf0b3dff6440b2b074f6b2cdc7cef4](../runs/RUN-5cbf0b3dff6440b2b074f6b2cdc7cef4.json)
- [RUN-be295d52d6ac40058ef9410ef43ce3bd](../runs/RUN-be295d52d6ac40058ef9410ef43ce3bd.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
