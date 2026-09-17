# TASK-046 基线（2026-09-17）：流程工具缺口修复前

## 缺口是什么（已两次致阻塞）

`workflow_runtime.py:435`：

```python
task["scope"]["allowed_paths"] = copy.deepcopy(specification.get("allowed_paths", paths))
```

只从 spec 的**顶层**读 `allowed_paths`。而本项目自带模板
`.workflow-kit/tasks/templates/TASK.json` 把它放在 **`scope.allowed_paths`（嵌套）**。
按模板形状书写 spec 是**最自然**的写法，却会被**静默忽略**——
没有报错、没有警告，直接回退为默认值 `paths`（即 `snapshot_paths`）。

而 `snapshot_paths` 为覆盖候选快照通常写成**目录**（如 `src-tauri/src`），
`finish` 的越界判定用 `matches()`（`workflow_runtime.py:209`，纯 fnmatch）：

```
matches("src-tauri/src/commands/ai.rs", ["src-tauri/src"])  ->  False
matches("src-tauri/src/commands/ai.rs", ["src-tauri/src/**"]) ->  True
```

于是回退得到的 allowed_paths 对**任何**文件改动都不匹配 → 全部越界。

## 两次真实命中

| 任务 | 误判越界的文件数 | 解锁方式 |
| --- | --- | --- |
| TASK-043 | 8（`src/**`） | owner 授权记录级订正（DEC-task043-ledger-fix-20260917） |
| TASK-044 | 8（`src-tauri/src/commands*`） | owner 既有授权记录级订正（DEC-toolfix-extend-deadlock-20260915） |

**为什么难自救**：`failure_kind=scope` 不在 `RETRYABLE`（`{"test_failure","review_failure"}`），
也不在 `begin` 的放行集合（`RETRYABLE | {network, interrupted, environment, budget}`）。
实测确认 CLI 无恢复入口：

```
recover --run <id> --source …  -> {"ok": false, "errors": ["Run is already closed; use next"]}
recover --source …             -> {"ok": false, "errors": ["No orphan controller lock found…"]}
begin                          -> {"ok": false, "errors": ["Resolve this blocker explicitly before resuming"]}
```

更麻烦的是**批次被连带卡住**：`new_batch` 要求当前批次任务全部处于
`{verified, done, cancelled}`，而 CLI **没有 cancel 命令**。

## 本次要做的（缺口文档第 1 项，优先项）

让 `prepare`：

1. **同时接受**顶层 `allowed_paths` 与 `scope.allowed_paths`；
2. 两者**同时存在且不一致**时**报错退出**，说明冲突，不静默择一；
3. 两者都不存在时**保持现有行为**（回退 `snapshot_paths`），不破坏既有 spec 兼容性。

**刻意不做**（记录取舍）：

- 第 2 项建议（让 `matches()` 支持目录前缀）——会**放宽越界判定语义**，本任务不采纳；
- 第 3 项建议（把 `scope` 加入 begin 放行集合）——属放宽门禁，不采纳；
- 第 4 项建议（补 `cancel` 命令）——独立功能，另立任务。

## 基线状态

```
python .workflow-kit/scripts/project_workflow.py check --root .
=> {"ok": true, "errors": [], "warnings": [], "counts": {"tasks": 16, "runs": 91, "batches": 6}}
```

既有任务记录一律**不改**：本任务只修工具行为，不为任何历史任务改写授权
（历史订正已由 owner 单独授权完成）。

## 回归自测计划（新增）

新增 `.workflow-kit/scripts/tests/test_allowed_paths.py`，覆盖四种 spec 写法：

| 场景 | 期望 |
| --- | --- |
| 仅顶层 `allowed_paths` | 采用顶层值 |
| 仅 `scope.allowed_paths` | **采用嵌套值**（当前会错误回退，即本次修复点） |
| 两者一致 | 采用该值 |
| 两者冲突 | **报错退出**，不静默择一 |
| 两者都无 | 回退 `snapshot_paths`（保持兼容） |

该缺口已两次静默致阻塞，故必须留下可复跑的回归测试。
