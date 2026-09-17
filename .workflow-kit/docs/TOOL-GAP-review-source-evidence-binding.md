# 工具缺口：审查摘要把「可变产物」按活动内容绑定，被合法改动永久撞坏

日期：2026-09-17。范围：仅工具脚本与其文档，不含业务代码。
授权：owner 2026-09-17「我授权你先修复工作流的这个问题」。

## 现象

TASK-049（把 `src/store.ts` 拆成 Zustand slice，属工作流**明确允许**的后续重构）完成后：

```
check → TASK-043: review evidence changed after approval
```

TASK-043 早已验收完成，其自身源码一行未动。

## 根因

`project_workflow.py` 的 `review_quality_digest()` 逐项处理审查报告里的 `evidence_files`：

```python
if name in candidate_files:
    # Later tasks may legitimately change the source; ...
    recorded[name] = candidate_files[name]          # 候选内 → 绑快照
else:
    recorded[name] = sha256(活动内容)                # 其余 → 绑「当前内容」
```

**只有候选快照内的文件**才享受「绑定当时快照」的保护。而审查者常会把
**候选快照之外**的文件列为证据（它们只是审查时看过的文件）：

| 审查 | 被绑定的可变产物 | 何时被何种合法操作改动 |
| --- | --- | --- |
| TASK-040 | `src-tauri/src/db/{commands_extraction_tests,sync_map}.rs` | 后续任务改同一源码 |
| TASK-041 | `src/mockData.ts` | 同上 |
| TASK-043 | `src/store.ts` | TASK-049 拆分（本次触发点） |
| TASK-044 | `tasks/runs/RUN-99dc89f0….json` | 工具重写运行记录 |
| TASK-045 | `.gitattributes`、`.workflow-kit/binding.json`、`tasks/runs/RUN-2a86101c….json` | 工具升级刷新 binding |

**这是工具设计与工作流自身规则冲突**：工作流既要求「后续任务修改同一源码时依赖最新已验证任务」
（允许改源码），又在获授权的工具升级中合法改写 `binding.json` / 运行记录，
却把这些文件按活动内容冻进历史审查 → 任何一次合法改动都让那条历史审查永久失配。

## 修复（本次实施）

**把绑定范围收敛到「设计上不可变」的证据**：候选快照内的文件 + 记录区
（`<workflow>/tasks/evidence/**`，即审查者产出的报告/日志/截图）+ 该次 `verification_run` 的 JSON。

其余一律**不纳入**篡改判据（`continue`）：源码（`src/` `src-tauri/` `tools/`）、
配置（`.gitattributes` 等）、工具脚本、`binding.json`、其它运行记录
——它们由各自任务的候选快照与 git 约束。

**为什么这不削弱保护**：审查的篡改判据本应落在**工作流记录**上
（谁在何时报告了什么），而这些仍按活动内容强绑定；`ui_review_digest` 本就要求
证据必须在 `tasks/evidence/` 下，两者现在口径一致。

## 连带处置：历史派生摘要的对账

摘要算法变更会让**所有**历史审查的记录值失配（错误数一度从 1 变成 4）。
派生值必须按新规则重算，故做了一次**对账**，且每例都先验证两个前提：

- **P1** 该审查确有被排除的可变产物（否则失配无法由规则变更解释，可能是真实篡改）；
- **P2** 其**仍被绑定**的记录区证据相对 `HEAD` 干净（未改动）——防止把真实篡改洗掉。

共修正 **5 条**：TASK-040、TASK-041、TASK-043、TASK-044、TASK-045。
五例的 P1/P2 全部满足（排除项均为可变产物；被绑定的记录区文件 `git status` 干净）。

### 我自己的失误（如实记录）

对账**第一次跑失败了**：我的诊断脚本漏算了真实函数会**无条件**把
`verification_run` 的 JSON 计入摘要，于是算出的 `now` 是错值，
并把 TASK-044/045 的派生摘要写成了这个错值。
**发现方式**：`check` 仍报这两条失配 → 改用**真实函数本身**重算并做权威对账才收敛。

教训：**派生值必须用产生它的那个函数来重算**，不要在自己写的副本里重实现规则
——副本一旦漏掉一个分量，就会静默写出错值（这与 TASK-044「行数口径」是同一类失误：
自建口径未与权威口径对齐）。

## 边界（刻意不做）

- 不改变「候选内文件绑快照」的既有行为；
- 不放宽 `ui_review_digest`（它本就限定证据须在 `tasks/evidence/` 下）；
- 不为可变产物补「审查当时的内容快照」——那需要新增持久化字段，
  而它们的绑定职责已由各自任务的候选快照承担。

## 回归自测

- 修后 `check` 全绿（21 任务 / 123 运行 / 0 errors / 0 warnings）；
- 反向确认：**记录区**证据仍被强绑定——改动任一被引用的 `tasks/evidence/**` 文件
  会使 `check` 重新报出对应的 `review evidence changed after approval`。

## 连带变更

`project_workflow.py` 是受管文件，按先例（`61563e6`、`a267b1e`）刷新
`binding.json` 的 `managed_files[".workflow-kit/scripts/project_workflow.py"]` 与 `tool_digest`，
否则 `start` 报 `integration_needs_repair`。
