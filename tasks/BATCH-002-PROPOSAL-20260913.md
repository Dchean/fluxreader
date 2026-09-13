# BATCH-002 任务提案（待用户确认额度与范围）

> **状态更新 2026-09-13：用户答复『全部批准』，本批已获授权执行（见 PROJECT.pending_approvals APPROVAL-BATCH-002-001）；A-2 亦获批（APPROVAL-HOSTED-CI-001）。R-06 用户表示记不清，已按 OWNER_CANNOT_RECALL 如实记录并以 TASK-009 脱敏补救。**

提案时间：2026-09-13。提案人：管理 agent。**本文件不是批准**；BATCH-001 已满额（3/3），新批次需用户明确授权后才可派发。

预算沿用既定值：每任务最多 3 轮修复 + 90 分钟墙钟，每批最多 3 个任务，不设费用上限。三个候选任务都是小任务，预计一个任务远用不满预算。

## 候选任务

### P2-1 为 CI 变更补 Note 反向追溯注释（R-04 残留）

- **目标**：`.github/workflows/ci.yml` 的 frontend 作业新增步骤处补一行注释：`# Note: 将前端回归接入 CI 的理由与备选 — 见 .agents/notes/proposed/testing/2026-09-13-frontend-regression-in-ci.md`。
- **背景**：AGENTS.md 要求代码核心入口保留 Note 反向追溯注释；TASK-005 的 CI 改动漏了这行（独立审查 R-04 指出）。
- **允许路径**：`.github/workflows/ci.yml`（仅注释行；release.yml 不在范围）。
- **验收**：git diff 仅含该注释行；YAML 语义不变（无步骤/触发器/权限变化）；注释中的 Note 路径真实存在。
- **验证**：管理 agent 独立核对 diff + `npm run lint` + `npm run test:frontend`（复用既有门禁，不新增检查）。
- **风险**：极低。纯注释，无行为变化。

### P2-2 修复契约文件 $schema 键（对应批次报告 A-3，需用户确认）

- **目标**：从 `docs/contracts/worker-result.schema.json` 移除第 2 行的 `"$schema"` 键，使本机 Claude Code 2.1.270 的 `--json-schema` 能直接接受该契约文件；随之删除"每次派发生成去掉 $schema 的运行期副本"这一绕过步骤。
- **背景**：这是真实的仓库缺陷（BLOCKER-A 残留）；$schema 键是 JSON Schema 的可选注解，移除不改变本契约的约束语义（required/类型/enum/additionalProperties 均不变，独立审查第 89 行已核实过三者差异仅此键）。
- **允许路径**：`docs/contracts/worker-result.schema.json`（受保护契约文件，因此单列并需用户确认）；配套的项目 Note（proposed/process）。
- **验收**：契约文件被 CLI `--json-schema` 直接接受（真实调用探测 1 次，退出 0 + 结构化输出）；required 与类型约束逐项不变（修改前后 JSON Schema 语义 diff 仅 $schema 键）；Note 按技能格式通过两个校验器。
- **验证**：管理 agent 用一个无害探测 prompt 实测 CLI 接受度，并核对约束未变。
- **风险**：低。契约文件属受控内容，但变更方向是"让工具接受既有约束"，不是放宽任何约束。
- **用户确认点**：因 docs/contracts/** 在受保护清单内，此项按 A-3 既有待决事项处理——批准本任务即视为批准 A-3。

### P2-3 R-06 凭据残留处置（依用户对 R-06 的答复而定）

- **目标**：对 `.cache/runs/BATCH-001/` 下三个含非空 `ANTHROPIC_AUTH_TOKEN` 的文件（probe-settings.json、TASK-004/attempt-1/run-settings.json、TASK-005/attempt-1/run-settings.json）做脱敏副本：副本中令牌值替换为 `<REDACTED>`，保留原件于本机受控位置，登记原件/副本哈希对应关系。env 键名、模型名等非敏感元数据全部保留，不损失证据价值。
- **背景**：独立审查 R-06 确认令牌已落盘三个本机文件；无论用户是否追认该复制动作，脱敏副本都降低后续任何交接/证据引用的暴露面（DEVICE-HANDOFF 的脱敏移交模式）。
- **允许路径**：`.cache/runs/**`（全部被 .gitignore 排除）+ 收尾记录追加。
- **验收**：副本中无令牌明文（管理 agent 以非空布尔校验，不打印值）；对应关系与哈希入册；原件按用户答复决定保留或删除。
- **验证**：管理 agent 机械执行并核对。
- **风险**：极低；不改变任何证据结论。
- **依赖**：不阻塞——无论 R-06 答复如何都可执行；若用户要求删除原件，则在同任务内一并处理。

## 不在本批的事项（后续批次/用户决定）

| 事项 | 原因 |
| --- | --- |
| A-1 合并本批变更 | 用户按功能/里程碑集中验收；合并前需 A-2 |
| A-2 托管 CI 真实证据 | 需要推送分支到远端触发 CI——外部动作，由用户决定方式（如批准推送 `ci/e2e-proof` 分支） |
| Reader/Overlays 3 条 hooks lint 警告 | 修复会波及 effect 依赖与渲染时序，属行为敏感改动，需先立 proposed Note 分析，单独批次 |
| 技能副本 3 条 lint 警告 | 技能目录约定"原样复制"，不在任务范围；如实记录为残留噪声 |
| OPT-004 非敏感配置同步实现 | 范围已定（DEC-008/009）但需先产出字段清单与验证方案（proposed Note + 任务定义），建议作为 BATCH-003 首个任务 |
| 真实桌面 UI E2E / 真实服务兼容性 | ISSUE-003/007，需要真实环境与账号，高风险边界，用户决定 |

## 需要用户回答的三件事

1. **BATCH-002 额度**：是否批准以上 3 个任务作为 BATCH-002（沿用 3 轮/90 分钟/任务预算）？可只批准其中若干。
2. **R-06 确切范围**：以下三项当年批准了哪些——① 使用既有第三方网关；② 将认证令牌复制到项目 .cache/；③ 回退既存未提交 TASK-004 改动。（答复将记入收尾记录的授权依据）
3. **A-2 托管 CI**：是否批准以某种方式（如推送专用分支）取得托管 CI 真实证据？不批准则合并决策继续挂起。
