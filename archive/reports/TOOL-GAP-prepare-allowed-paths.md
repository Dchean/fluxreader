# 工具缺口：prepare 静默忽略 spec 里的 scope.allowed_paths，回退成目录式 snapshot_paths

日期：2026-09-17（**2026-09-21 订正**：第 1 项修复曾被升级覆盖、当前由工程侧守卫兜底，
详见文末「建议的工具修复」；结论已按 git 与实跑核实，不再沿用原文的「已实施」记载）。
范围：仅工具脚本与任务准备流程，不含业务代码。

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

实测（**2026-09-17 当时的版本**）：

```
matches("src/components/Timeline.tsx", ["src/components"])  ->  False
matches("src/components/Timeline.tsx", ["src/**"])          ->  True
```

于是回退得到的 allowed_paths 对**任何**文件改动都不匹配 → 全部越界。

**2026-09-21 订正**：上游升级后的 `matches()` **已支持目录前缀匹配**
（实测 `matches("src-tauri/src/ai.rs", ["src-tauri/src"]) == True`），
故上述「目录名匹配不到具体文件 → 全部越界」的死锁前提在当前版本**已不成立**。
但缺口本身没有消失：只写嵌套时 spec 声明的范围仍会被**静默丢弃**并落成
`snapshot_paths`（即「范围声明被静默篡改」），这才是工程侧守卫要拦的问题。

## 为什么难以自救

`failure_kind=scope` **不在** `RETRYABLE`（`{"test_failure","review_failure"}`），
也不在 `begin` 的放行集合（`RETRYABLE | {network, interrupted, environment, budget}`）：

```
begin -> {"ok": false, "errors": ["Resolve this blocker explicitly before resuming"]}
```

与 [protocol 无恢复入口](TOOL-GAP-protocol-recovery.md) 同类：`scope` 需要**显式处置**
才能继续（**2026-09-21 订正**：当时本文件称「没有任何 CLI 命令能清除 `scope`」，
该说法已过时——现在有两条受控出口：`unblock --task … --source … --note …` 回 `ready`
（须给出真实来源说明），或 `cancel --task … --source … --reason …` 终止任务身份。
二者都要求留下真实依据，不是静默清除。）

更麻烦的是**批次被连带卡住**：`new_batch` 要求当前批次任务全部处于
`{verified, done, cancelled}`，因此一个被 `scope` 卡住的任务会同时冻结整个批次续接。
（**订正**：原文称「CLI 没有 cancel 命令」——该说法已过时，`cancel` 现已是正式命令；
本条风险因此降级为「需显式处置」，而非「无法脱困」。）

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

## 建议的工具修复

### 第 1 项 —— **曾实施，但已被升级覆盖；现由工程侧守卫兜底**（2026-09-21 订正）

**状态订正（原文写「已实施」已不成立，据 git 与实跑核实）**：

| 时间 | 事件 | 证据 |
| --- | --- | --- |
| 2026-09-17 | 修复实施：`prepare` 同时接受顶层与 `scope.allowed_paths`，冲突则报错；实现落在新纯函数 `resolve_allowed_paths(specification, fallback)` | commit `a267b1e` |
| 2026-09-18 | **修复被整段删除**：`rebind --upgrade-tools` 升级 workflow-kit 至 2026-09-18.3 时，用上游引擎覆盖了本地补丁，还原为 `copy.deepcopy(specification.get("allowed_paths", paths))` | commit `6fd382e`（diff 中 `-def resolve_allowed_paths`、`-task["scope"]["allowed_paths"] = resolve_allowed_paths(...)` → `+...get("allowed_paths", paths)`） |
| 2026-09-21 | 核实：`resolve_allowed_paths` 已不存在；回归测试仍在原地且**静默失败**（实测 0/7、exit 1、全部 `AttributeError: no attribute 'resolve_allowed_paths'`） | 本文件订正依据 |

原文的参数语义（历史记载，供对照）：两处都给且不一致 → 报错（不静默择一）；
只给嵌套 / 只给顶层 → 用给出的那个（嵌套优先）；都没给 → 回退 `snapshot_paths`。

**当前引擎的真实行为（2026-09-21 实测）**：只读**顶层** `allowed_paths`；
只写嵌套时静默回退 `snapshot_paths`。严重性需如实下调：升级后的 `matches()` **已支持目录前缀**
（`matches("src-tauri/src/ai.rs", ["src-tauri/src"]) == True`），故原文「目录名匹配不到具体文件
→ 全部误判越界」的死锁前提**在当前版本已不成立**；但**范围声明被静默篡改**依然存在
（仅写嵌套时记录里落成 `snapshot_paths`），这才是仍需防的问题。

#### 现方案（TASK-080，2026-09-21）：工程侧前置守卫，不动引擎

引擎脚本位于任务模板默认 `protected_paths` 内（`.workflow-kit/scripts/**`），任何任务都不得修改；
且本地引擎补丁会被 `rebind --upgrade-tools` 反复覆盖（上表已证）。故改为**项目自有**的
`tools/` 下交付守卫，升级不会覆盖：

```text
python tools/task-spec-guard.py <spec.json> ...   # 校验；非零退出码 = 拒绝，别交给 prepare
python tools/task-spec-guard-test.py              # 13 例自测（单元 + 端到端退出码），0 = 通过
```

守卫拒绝以下形态并给出可操作指引：`allowed_paths` 只写在嵌套 `scope.allowed_paths`
（会被静默忽略）、顶层与嵌套同时给出且不一致、缺失/空/含非字符串元素、`snapshot_paths` 非法。
放行时打印「记录后 allowed_paths」以便核对。已用历史真实形态回放验证：
TASK-043 的失败形态被拦下（exit 1），而 TASK-070、TASK-080 的真实 spec 正常放行（exit 0）。

**回归覆盖的去向**：`.workflow-kit/scripts/tests/test_allowed_paths.py` 位于受保护的
`.workflow-kit/scripts/**` 内，任何任务不得修改，故其引擎依赖无法就地修复。其 7 项覆盖意图
（仅顶层 / 仅嵌套 / 一致 / 冲突 / 都无 / `scope` 非 dict / 深拷贝隔离）已由
`tools/task-spec-guard-test.py` 承接，并额外增加端到端退出码检查。该引擎侧测试当前处于
**已知损坏**状态（0/7），其状态如实记录于此，不再假装通过。

**升级注意事项（新增，避免同类复发）**：`rebind --upgrade-tools` 会用上游引擎覆盖项目本地
对 `.workflow-kit/scripts/**` 的一切补丁。凡依赖「本地引擎补丁」的防护都不可靠，必须改由
**工程侧（`tools/`）** 或**记录级/流程级**手段承载，否则会在下一次升级时静默失效——
本次即为实例（`a267b1e` → `6fd382e`）。

连带变更（历史记载）：`workflow_runtime.py` 是受管文件，当时按先例（提交 `61563e6`）
刷新了 `binding.json` 的 `managed_files[".workflow-kit/scripts/workflow_runtime.py"]`
与 `tool_digest`（`04957ba0…` → `78ebdc48…`；`82c680b4…` → `531d13a8…`），
否则 `start` 会报 `integration_needs_repair`。该次刷新随后被 `6fd382e` 的升级一并覆盖。

### 第 2–4 项 —— 逐项状态（2026-09-21 订正后）

2. **让 `matches()` 支持目录前缀匹配** —— **本项目不采纳**：会放宽越界判定语义
   （目录名将命中其下全部文件），属降低门禁强度，与「只修入参读取」的边界冲突。
   *注（2026-09-21）：上游升级后该行为已默认存在（见上文实测），非本项目主动采纳；
   它虽消除了「全部误判越界」的死锁，但也确实放宽了语义，值得后续单独评估。*
3. **把 `scope` 纳入 `begin` 的放行集合** —— **本项目不采纳**：同样属放宽门禁；
   `scope` 现由 `unblock` 显式处置（要求给出真实来源与说明），而非静默放行。
4. **补 `cancel` 命令** —— **已饱和，不再是缺口**（2026-09-21 订正）：
   `cancel --task … --source … --reason …` 已是引擎正式命令（实测
   `project_workflow.py cancel --help` 退出码 0），本文件上文「为什么难以自救」一节
   关于「CLI 没有 cancel 命令」的记述为此已过时。故「单个卡死任务冻结整批续接」这一风险
   不再成立——受 `scope` 阻塞的任务现在既可 `unblock` 回 `ready` 继续，也可 `cancel`
   终止任务身份，两者都要求留下真实依据。

> **本节结论（供后续维护者）**：`scope` 阻塞**有受控出口**（`unblock` / `cancel`，
> 均需显式来源），不存在「无法脱困」的状态；本缺口真正仍待防的是
> **spec 范围声明被静默丢弃**，已由工程侧守卫 `tools/task-spec-guard.py` 兜底（见上）。

