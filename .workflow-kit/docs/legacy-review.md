# legacy_review：旧流程资料核对与处置

性质：workflow-kit 接入后的旧资料核对记录（只读考古，未改动任何业务代码）。
记录时间：2026-09-14（本地）。
记录者：当前主会话（总控 / manager）。
接入回执：`integration.connected = true`，layout=isolated，入口 `WORKFLOW-KIT.md`。
Git 基准：HEAD `7f4509e3b981986057becb4352348bcd06224280`，分支 `main`，最新标签 `v0.13.0`。

本文件是 `legacy_review` 的正文依据；对应字段将在 `onboard` 时写入 `tasks/BRIEF.json`。
本次只做读取、整理与评估：未修改产品/测试/CI 代码，未安装依赖、未运行应用测试、未提交、未合并、未发布。

---

## 1. reviewed_paths（已读来源）

binding 声明的 `legacy_sources`（必须全部覆盖）：

- `AGENTS.md`（顶部已加 workflow-kit 入口，原文保留）
- `CLAUDE.md`（顶部已加 workflow-kit 入口，原文保留）
- `tasks/PROJECT.json`
- `tasks/EXECUTION-POLICY.json`
- `docs/HANDOFF.md`
- `docs/PROCESS.md`
- `docs/EXECUTION-CONTRACT.md`
- `docs/PRODUCT.md`
- `docs/FEATURES.md`
- `docs/ARCHITECTURE.md`

本次为形成结论另行读取：

- `WORKFLOW-KIT.md`、`.workflow-kit/binding.json`、`.workflow-kit/tasks/PROJECT.json`、`.workflow-kit/tasks/PROJECT_STATE.md`
- `docs/BASELINE.md`、`docs/ISSUES.md`
- `docs/prompts/MANAGER-RESUME.md`
- `docs/runs/M2-module-disposition-20260913.md`
- `tasks/items/TASK-022.json`、`tasks/items/TASK-023.json`；`tasks/items/TASK-024..029.json`（标题/状态）
- `.agents/notes/` 目录清单（8 篇 proposed/implemented）
- Git 状态与历史（`git status --porcelain`、`git log`、`git tag`）
- 只读代码度量（行数、SQL 模式分布、目录结构；脚本为临时测量，未写入仓库）

未读取（不存在或未触及）：旧 README 已被 BATCH-001/002 修订；`docs/DATA-MODEL.md`、`docs/API.md`、`docs/TEST-PLAN.md`、`docs/USER-FLOWS.md`、`docs/RELEASE-ROLLBACK.md`、`docs/DEVICE-HANDOFF.md`、`docs/CLAUDE-WORKER.md` 本轮未逐字读取（评估不依赖其内容，标记为未核对）。

---

## 2. preserved_constraints（必须继续保留的约束）

### 2.1 业务 / 功能

- 核心能力保留：`REQ-LAYOUT-001`（文章/社交/画廊/播客/通知 五布局）、`REQ-SYNC-001`（Google Reader / Fever，面向 FreshRSS / Miniflux）、`REQ-AI-001`（AI 摘要）、`REQ-AI-002`（AI 翻译）。
- `OPT-001～010` 全部为“必须保留”范围（DEC-008 / DEC-009）；OPT 前缀是历史清单编号，**不得**据此前缀把已确认保留的能力当作可选或可删除。
- 可以调整内部实现，**不得**借重构缩减上述能力。

### 2.2 数据边界

- `OPT-004` 收敛范围：只同步**订阅源 + 客户端设置**，允许服务地址/模型等**非敏感**连接配置；**不同步**文章内容、媒体文件、已读/收藏状态、AI 生成结果。
- **排除** API Key、密码等敏感凭据；不得直接序列化整份 `settings` / `ai_config`；导入只应用允许字段，不得整体替换清空本地未同步凭据。
- `DEC-004` 仅免除旧 SQLite 数据/设置/账号的兼容要求，**不授权删除真实用户数据**；不访问真实应用数据库，不使用真实账号跑同步测试。

### 2.3 安全

- 凭据保护（`credentials.rs` / Windows DPAPI）、HTML 清洗（`sanitize.rs` / ammonia）、账号数据边界属于共享安全边界，任何相关变更都需评估。
- 不读取、不传递真实服务凭据；凭据不写入项目缓存、提示词或交接文档。
- 不使用权限绕过（如 `bypassPermissions`）解决权限拒绝。

### 2.4 兼容与技术结构

- 保留现有 `src/`、`src-tauri/`、`tools/` 结构，直到具体迁移任务获准；不得以整理目录为由开始业务重构。
- 不随任务附带升级依赖、全仓格式化或重写历史迁移；不引入未批准的新依赖或大版本升级。
- 破坏性架构 / 数据库 / 外部契约调整需用户批准；服务端 × 协议 × 版本 × 操作能力分别记录，不从“协议实现存在”推导兼容性已验收。

### 2.5 流程与授权

- 需求、验收标准、基线预期、门禁规则、状态批准字段属受控内容，不得为让实现通过而自行降低。
- 每任务最多 3 轮修复（质量门）；合并与发布分别由用户确认；发布标签也不得绕过发布确认。
- 保留原任务身份、预算与证据，不重建任务/批次绕过计数；旧批准不自动扩展到新工作流的新工作。
- 启动包的应用测试状态按“绑定具体源码版本的运行报告”为准；PASS/FAIL/NOT_RUN/SKIPPED/BLOCKED/NOT_APPLICABLE 分开报告，读取测试文件不等于通过。

---

## 3. task_disposition（旧任务处置）

- 旧任务源 `tasks/items/TASK-001～029.json`、`tasks/batches/BATCH-001～003.json`、`tasks/runs/*.json` **保留原位**，不迁移、不删除、不自动转换为新流程队列。
- 与 `tasks/items/` 并存的新流程目录 `.workflow-kit/tasks/` 只承载新流程状态；两者互不覆盖。
- **实际进度（以 Git 为准，HEAD `7f4509e`）**：
  - 已 `verified` 并合入：`TASK-017`（commands.rs 13 处直接 SQL → db.rs，BATCH-005）、`TASK-022`（sync.rs 17 处直接 SQL → db.rs，BATCH-007，merge `0ebd904`）。
  - `ready` 待办：`TASK-023`（db.rs 内部模块化）、`TASK-024`（sync.rs 内部分层）、`TASK-025`（commands.rs 命令组拆分）、`TASK-026`（store.ts 拆 slice）、`TASK-027`（OPT-004 `merge_app_settings` 子字段白名单）、`TASK-028`（GReader ClientLogin 双格式兼容）、`TASK-029`（Fever 端点路径兼容）。
  - `v0.13.0` 已发布（tag 存在）。
- 历史 M0/M1 报告中的 NOT_RUN/BLOCKED 保留为历史证据，**不得**当作当前状态。
- **旧 `tasks/PROJECT.json` 已过时**：其 `updated_on=2026-09-13`、`current_batch=BATCH-003`、`next_action` 仍写“M2 处置+试点已合并（a0ce278）/TASK-021~024 是否继续”，与 Git 事实（TASK-022 已合并、`v0.13.0` 已发布、队列为 TASK-023~029）不一致。本记录以 Git 与 `tasks/items/` 为准。
- 新流程不重置旧任务预算；TASK-023~029 的预算/授权沿用旧 `EXECUTION-POLICY`，是否迁入新流程队列待用户决定。

---

## 4. workflow_resolution（当前流程与角色）

- 当前流程入口：根 `WORKFLOW-KIT.md`（isolated 布局，工作流文件在 `.workflow-kit/`）。旧 `AGENTS.md` / `CLAUDE.md` 原文保留，入口前置；原文副本在 `.workflow-kit/legacy/`。
- 当前主会话身份：**总控（manager）**。只有收到明确 `task_id` / `run_id` 执行包时才作为 Worker。
- 旧 `CLAUDE.md` 把会话写成“Claude Code 执行器”，**不是**本会话的角色定义；以用户当前明确选择为准。
- 代码执行器：旧 `EXECUTION-POLICY` 记为 `claude_code_cli`；新流程下 `execution.coder` / `execution.reviewer` / `review_mode` 待 `onboard` 确认，不自动沿用。
- 事实源分工：新流程状态以 `.workflow-kit/tasks/PROJECT.json` 为准；产品与业务约束继续引用 `docs/PRODUCT.md`、`docs/FEATURES.md` 等原文档，不另建一套事实。
- 工作流工具不覆盖宿主系统约束，也不授予业务代码、付费调用或发布权限；当前 `PROJECT.stage = intake`。

---

## 5. unresolved_conflicts（未决冲突）— 已由 2026-09-14 用户决定解决

用户本会话选择「渐进重构」并确认：首个试点 = `db.rs` 模块化（沿用旧 TASK-023）；旧队列 TASK-023~029 迁入新流程继续（保留 ID、身份与预算）；新流程下沿用 DEC-013 的合并授权（CI 全绿后可自主合并，发布仍需单独确认）。据此三项旧流程冲突全部解除：

1. **角色冲突 → 已解决**：当前会话为总控（manager），非代码执行器；执行器定为 `claude-code-cli`，审查为 `current_agent` 新上下文的独立审查（`review_mode=independent_required`）。
2. **任务源冲突 → 已解决**：TASK-023~029 迁入新流程管理，保留原任务 ID 与预算；旧 `tasks/items` 保留原位不删除。
3. **预算 / 合并授权冲突 → 已解决**：`batch_rollover=allowed`、`financial.mode=none`、`merge/push/pull_request/commit=allowed`、`release=ask`。表示限制：工作流要求任务墙钟与批次任务数为明确正整数，取 DEC-011 原值 90 分钟 / 每批 3 个任务，用自动续批与 `extend` 承接 DEC-013 的「不限」意图。

仍开放但**不构成旧流程/角色冲突**的观察项（已记入 BRIEF 的 `outstanding_items`，不阻塞本次纯内部移动式重构）：

4. **产品未决问题**：`Q-COMPAT-001`（FreshRSS/Miniflux × GReader/Fever 组合与版本范围）、`Q-BEHAVIOR-001`（冲突/删除订阅/断开账号/协议切换行为）、`Q-QUALITY-001`（数据规模、性能目标、可降级范围）。
5. **文档与实测漂移**：`docs/ISSUES.md`（ISSUE-008 行）与 `docs/ARCHITECTURE.md`（“sync.rs 仍有 17 处直接 SQL”）与实测不符——`sync.rs` 直接 SQL = 0（TASK-022 已合并）。属文档同步缺口，非代码缺陷；修正需单独授权。

`unresolved_conflicts` 因此为空，`authority.code` 可在 onboarding 中启用。

---

## 6. onboard 已写入字段（2026-09-14 已 onboard，决定 DEC-9873537950da450da003e9bb04006117）

```json
{
  "legacy_review": {
    "reviewed_paths": [
      "AGENTS.md", "CLAUDE.md", "tasks/PROJECT.json", "tasks/EXECUTION-POLICY.json",
      "docs/HANDOFF.md", "docs/PROCESS.md", "docs/EXECUTION-CONTRACT.md",
      "docs/PRODUCT.md", "docs/FEATURES.md", "docs/ARCHITECTURE.md",
      "WORKFLOW-KIT.md", ".workflow-kit/binding.json", ".workflow-kit/tasks/PROJECT.json",
      "docs/BASELINE.md", "docs/ISSUES.md", "docs/prompts/MANAGER-RESUME.md",
      "docs/runs/M2-module-disposition-20260913.md",
      "tasks/items/TASK-022.json", "tasks/items/TASK-023.json"
    ],
    "preserved_constraints": [
      "核心 4 项（五布局 / GReader-Fever 面向 FreshRSS-Miniflux / AI 摘要 / AI 翻译）与 OPT-001~010 全部必须保留，不得借重构缩减功能",
      "OPT-004 只同步订阅源与客户端设置，允许地址/模型等非敏感配置，排除 API Key/密码等凭据；不序列化整份 settings/ai_config",
      "DEC-004 免除旧数据兼容，但不授权删除真实数据；不访问真实应用数据库、不用真实账号跑同步测试",
      "凭据保护（DPAPI）、HTML 清洗（ammonia）、账号数据边界为共享安全边界，相关变更必须评估",
      "保留 src/、src-tauri/、tools/ 结构；不引入未批准依赖/大版本升级；破坏性契约变更需用户批准",
      "需求/验收/门禁/授权字段受控，不得为通过而降低；每任务 3 轮修复；合并与发布分别由用户确认",
      "服务端×协议×版本×操作能力分别记录，不从实现存在推导兼容性已验收"
    ],
    "task_disposition": "旧 tasks/items、tasks/batches、tasks/runs 保留原位，不迁移不删除，不自动转为新队列。截至 HEAD 7f4509e：TASK-017、TASK-022 已 verified 并合入（SQL 边界收敛）；TASK-023~029 为 ready 待办；v0.13.0 已发布。旧 tasks/PROJECT.json 已过时（current_batch=BATCH-003、next_action 停留在 TASK-021~024），以 Git 与 tasks/items 为准。新流程不重置旧预算。",
    "workflow_resolution": "当前入口为根 WORKFLOW-KIT.md（isolated 布局）；当前主会话为总控（manager），不是旧 CLAUDE.md 描述的代码执行器。旧 AGENTS.md/CLAUDE.md 原文保留并前置入口，副本在 .workflow-kit/legacy/。执行器 coder/reviewer/review_mode 待 onboard 确认。产品事实继续引用 docs/PRODUCT.md、docs/FEATURES.md。",
    "unresolved_conflicts": []
  }
}
```
