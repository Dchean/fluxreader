<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-120 · TASK-117 延续：修复新增代码的 clippy 警告（CI 失败处置，行为零变化保持）

**状态**：cancelled

**目标**：延续任务（continuation_of=TASK-117 候选 577efdd5...详见 items/TASK-117.json evidence）：dev@b7947e9 起 CI rust job clippy（--all-targets -- -D warnings）失败（fmt 过/test 跳过；117/118/119 三提交同态连续失败）。修复面=TASK-117 引入的 Rust 代码（db/articles.rs 的 ArticleQuery 新字段/KEYSET_PREDICATE 常量/keyset 谓词组装/ORDER 常量、migrations.rs v17、commands/articles.rs ArticleListArgs、dedup_tests.rs、db/articles.rs 内新测试模块）。方法：TASK-113 先例——clippy-driver 抛置片段实证（本机 clippy 1.98.1 与 CI 同 release）+ 与仓内既有同形态代码比对；逐一修复（等价重写，不改行为/不删有效断言/不动 keyset 语义）。若片段实证无法定位，输出「已排除清单」并停止（等待主控取得 CI 日志）。禁止 cargo test/clippy/build/check（本机必败且浪费）。

**依赖**：TASK-117
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src

## 验收标准

- ① clippy 触发点修复：触发点清单（file:line+lint 名+修法+依据）与已排除清单
- ② CI rust job 全绿（cargo fmt/clippy/test 含 keyset 3 条测试）——推送后以 CI 结论为准
- ③ TASK-117 既有测试与断言零弱化；行为零变化保持
- ④ 独立审查（全新子代理）PASS findings=0
- ⑤ 本地门禁：cargo fmt、lint、build、frontend 不回退

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@7a3d0f4（117/118/119 前端全部通过；rust job clippy 失败）。本卡修复 clippy 触发点并经 CI 复验。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 适配：触发 clippy 的等价重写；等价重写不弱化保护；CI 全绿为验收；验证：cargo_fmt, lint, build, frontend
- 保留：TASK-117 既有断言与本地四门禁；行为零变化证明；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T07:12:10.513761Z
- 原截止时间：2026-10-07T11:12:10.513761Z
- 当前截止时间：2026-10-07T11:12:10.513761Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 29 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-10-07T07:12:11.826333Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T07:41:15.982889Z：Out-of-scope changes: src-tauri/tests/ingestion_e2e.rs (allowed: src-tauri/src)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-10-07T07:41:53.399948Z：阻塞已处置（scope）：scope 偏离裁定（主控接受）：修复点 src-tauri/tests/ingestion_e2e.rs:95 超出卡面 src/**，但 E0063 硬编译错误下 src 内无修法（结构体字面量穷尽性校验）；该 2 行属 TASK-117 自身遗漏的补齐（与 117 对 dedup_tests.rs 同款补法），测试语义零变化（None/None=既有 OFFSET 路径）。worker 执行中已通报，主控裁定接受并纳入验收审查范围。；下一步：begin 重新实现
- 2026-10-07T07:42:42.009732Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T07:43:11.961458Z：Out-of-scope changes: src-tauri/tests/ingestion_e2e.rs (allowed: src-tauri/src)；下一步：核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin；不要新建任务或重置预算
- 2026-10-07T07:44:54.751542Z：任务已取消：卡面 allowed_paths 规格错误（漏 src-tauri/tests/**，主控立卡疏漏）：E0063 修复点在 tests/ 必然越界且 unblock 不能改冻结范围。取消后以修正 allowed_paths 重建 TASK-121（修复已在工作树，形式化收编+验证审查照常）。120 的诊断工作（E0063 定罪+12 组疑点排除）记录保留有效。；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-120.json)

- [RUN-f89fef2b4f6e402b88ef33e7b4b1583e](../runs/RUN-f89fef2b4f6e402b88ef33e7b4b1583e.json)
- [RUN-68abfe95361247a1922108c635178a0e](../runs/RUN-68abfe95361247a1922108c635178a0e.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
