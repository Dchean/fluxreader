<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-044 · 后端单体拆分一：commands.rs 按领域拆分为子模块（可维护性）

**状态**：done

**目标**：按 BRIEF 验收目标「后端 commands.rs/sync.rs 拆分完成，拆分过程测试不回归」，沿用 db.rs 试点模式（领域子模块 + 就地测试模块），把 src-tauri/src/commands.rs（1157 行）按既有章节边界拆分为 commands/ 子模块，纯搬运不改行为。目标结构：commands/mod.rs 保留模块声明与共享辅助（即时状态推送调度、公共 import 重导出）；按现有章节注释拆出 folders.rs（Folders/Feeds）、articles.rs（Articles + 刷新）、settings.rs（Settings + 全文提取 + 图片代理）、opml.rs（OPML 导入导出）、sync.rs（后端同步）、ai.rs（AI 引擎）。所有 #[tauri::command] 函数名、签名、可见性、lib.rs 的 invoke_handler 注册列表保持不变；仅调整文件归属与模块路径。

**依赖**：无
**参考方案**：REF-003
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task044.md, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- commands.rs 已拆为 commands/ 子模块，单文件均不超过 400 行；mod.rs 只保留模块声明与共享辅助
- 所有 #[tauri::command] 函数名与签名逐字不变（可用 git diff 证明仅移动、无重写）
- lib.rs 的 invoke_handler 注册列表逐字未变
- cargo test 全量保持 120 passed / 0 failed（与拆分前基线一致）
- cargo fmt --all -- --check 与 cargo clippy --all-targets -- -D warnings 零告警
- cargo test 的 ignored 数不增加（不得用 #[ignore] 掩盖问题）
- 无行为改动：无新增依赖、无逻辑重写、无错误文案变更

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 拆分前基线（本会话实跑）：cargo test 合计 120 passed / 0 failed / 23 ignored（完整日志存 baseline-rust-tests-2026-09-17.log）；cargo fmt --all -- --check 与 cargo clippy --all-targets -- -D warnings 此前 CI 已复跑通过。本任务为纯文件搬运（模块拆分），不改变任何函数签名与行为；判据是同一套测试在拆分后仍全绿且 ignored 数不增。前端回归与本任务无关（未改前端）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task044.md
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：src-tauri/tests 下 20 个 e2e 套件与 src/ 内联测试模块；拆分必须证明对外行为未变；这些测试是唯一的行为契约保护，必须逐条保持通过且不得删改断言；验证：cargo_test
- 保留：cargo fmt 与 cargo clippy 门禁；拆分产生新文件，仍需格式与 lint 达标；clippy 的 -D warnings 可捕获模块边界导致的可见性/未使用告警；验证：cargo_fmt, cargo_clippy

## 执行与恢复

- 首次开始：2026-09-17T05:55:13.882335Z
- 原截止时间：2026-09-17T09:55:13.882335Z
- 当前截止时间：2026-09-17T09:55:13.882335Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 11 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T05:55:13.947252Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T06:06:03.647502Z：Out-of-scope changes: src-tauri/src/commands.rs, src-tauri/src/commands/ai.rs, src-tauri/src/commands/articles.rs, src-tauri/src/commands/folders.rs, src-tauri/src/commands/mod.rs, src-tauri/src/commands/opml.rs, src-tauri/src/commands/settings.rs, src-tauri/src/commands/sync.rs；下一步：先核对已有文件及原始日志，再处理 scope；不要新建任务或重置预算
- 2026-09-17T06:08:35.167432Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T06:08:46.236348Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T06:08:49.993927Z：[WinError 5] 拒绝访问。: 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-303404165dfb4e98b953dd6b5971ccb9.json.957d756fc3d34c5dadd332327160297d.tmp' -> 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-303404165dfb4e98b953dd6b5971ccb9.json'；下一步：先核对已有文件及原始日志，再处理 environment；不要新建任务或重置预算
- 2026-09-17T06:13:01.339150Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T06:35:20.825872Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T06:42:25.009110Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-044.json)

- [RUN-697b5cc69ebe4310b4c0a7d3dc0818c6](../runs/RUN-697b5cc69ebe4310b4c0a7d3dc0818c6.json)
- [RUN-56062e1e5c0b4c63babf738fc02b49cd](../runs/RUN-56062e1e5c0b4c63babf738fc02b49cd.json)
- [RUN-303404165dfb4e98b953dd6b5971ccb9](../runs/RUN-303404165dfb4e98b953dd6b5971ccb9.json)
- [RUN-148d59b826aa4e22b57b4de93bad3978](../runs/RUN-148d59b826aa4e22b57b4de93bad3978.json)
- [RUN-99dc89f06091432db82439b04cdfe69d](../runs/RUN-99dc89f06091432db82439b04cdfe69d.json)
- [RUN-73de1b583d12486697e705275bd0cd75](../runs/RUN-73de1b583d12486697e705275bd0cd75.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
