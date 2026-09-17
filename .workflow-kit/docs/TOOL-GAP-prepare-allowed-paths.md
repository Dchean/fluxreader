# 工具缺口：prepare 静默忽略 spec 里的 scope.allowed_paths，回退成目录式 snapshot_paths

日期：2026-09-17。范围：仅工具脚本与任务准备流程，不含业务代码。

## 现象

TASK-043 的 `prepare` 成功、`begin` 成功、编码完成、三门禁全绿，
但 `finish` 一次性判定**全部 8 个改动文件越界**：

```
{"ok": false, "failure_kind": "scope",
 "error": "Out-of-scope changes: src/components/ContextMenu.tsx, src/components/PlayerBar.tsx,
           src/components/SettingsModal.tsx, src/components/Sidebar.tsx,
           src/components/Timeline.tsx, src/components/primitives.tsx,
           src/store.ts, src/styles/base.css"}
```

## 根因（两步叠加）

### 1) prepare 只从 spec 的**顶层**读 allowed_paths

`workflow_runtime.py:435`：

```python
task["scope"]["allowed_paths"] = copy.deepcopy(specification.get("allowed_paths", paths))
```

而 `tasks/templates/TASK.json` 把该字段放在 **`scope.allowed_paths`**（嵌套）。
Agent 按模板形状写 spec（`"scope": { "allowed_paths": [...] }`）是**最自然**的写法，
但会被静默忽略——没有报错、没有警告，直接走默认值 `paths`（即 `snapshot_paths`）。

TASK-041/042 之所以没事，只是因为它们的 spec 恰好把 `allowed_paths` 写在了顶层。

### 2) snapshot_paths 里的目录根无法匹配具体文件

`snapshot_paths` 为覆盖候选快照，通常写成**目录**（如 `src/components`、`src/styles`）。
而 `finish` 的越界判定用 `matches()`（`workflow_runtime.py:209`）：

```python
def matches(path, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) or path == pattern.rstrip("/**") for pattern in patterns)
```

实测：

```
matches("src/components/Timeline.tsx", ["src/components"])  ->  False
matches("src/components/Timeline.tsx", ["src/**"])          ->  True
```

于是回退得到的 allowed_paths 对**任何**文件改动都不匹配 → 全部越界。

## 为什么难以自救

`failure_kind=scope` **不在** `RETRYABLE`（`{"test_failure","review_failure"}`），
也不在 `begin` 的放行集合（`RETRYABLE | {network, interrupted, environment, budget}`）：

```
begin -> {"ok": false, "errors": ["Resolve this blocker explicitly before resuming"]}
```

与 [protocol 无恢复入口](TOOL-GAP-protocol-recovery.md) 完全同类：
`block` 写了 checkpoint 后，没有任何 CLI 命令能清除 `scope`。

更麻烦的是**批次被连带卡住**：`new_batch` 要求当前批次任务全部处于
`{verified, done, cancelled}`（`workflow_runtime.py:1590`），
而 CLI **没有 cancel 命令**。因此一个被 `scope` 卡住的任务会同时冻结整个批次续接。

## 本次的处置（等待 owner 授权）

按项目先例，解阻塞需要 owner 明确授权的**台账订正**：

1. `TASK-043.json` 的 `scope.allowed_paths` 由目录式
   `[contract, baseline, "src/App.tsx", "src/components", "src/styles", "tools/frontend-regression.mjs"]`
   改为 glob 式 `["src/**", "tools/frontend-regression.mjs"]`；
2. 重算 `input.definition_digest`（`scope` 属受控字段，见 `project_workflow.py:216`）；
3. 清除 `failure_kind` / `blockers`，状态回到 `ready`；
4. 在 `evidence.ledger_correction` 与 `DECISIONS.json` 留痕，保留原始失败记录。

**边界**：订正只修「我写错嵌套层导致工具回退」这一处，
**不放松任何门禁**——被改动的 8 个文件全部在 `src/**` 内（任务本意就是改 `src/**`），
契约与基线仍在 `protected_paths` 中不可写，`verify` / `review` 仍需照常重跑。

## 建议的工具修复（未实施）

择一即可，且都应保留旧记录：

1. **优先**：让 `prepare` 同时接受顶层与 `scope.allowed_paths`（后者优先），
   或在发现 spec 里有 `scope.allowed_paths` 却未生效时**报错退出**而不是静默回退；
2. 让 `matches()` 支持目录前缀匹配（`src/components` 命中其下所有文件），
   避免候选快照根与授权路径语义混用；
3. 最低限度：把 `scope` 纳入 `begin` 的放行集合，并让 `next` / `progress`
   把 `scope` 标注为「无 CLI 恢复入口，需 owner 决定」；
4. 补一个 `cancel` 命令，使单个卡死任务不至于冻结整个批次续接。
