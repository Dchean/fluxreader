# Agent Note: 移除 worker-result 契约中的 $schema 键

Status: proposed

## Problem

`docs/contracts/worker-result.schema.json` 第 2 行带有 `"$schema": "https://json-schema.org/draft-07/schema#"`。本机 Claude Code 2.1.270 的 `--json-schema` 参数在解析时直接拒绝该键：

```text
Error: --json-schema is not a valid JSON Schema: no schema with key or ref "https://json-schema.org/draft-07/schema#"
```

这导致 BATCH-001 期间每次派发都要由管理 agent 在运行目录内生成一份去掉该键的临时副本作为参数（见 env-unblock 记录的 BLOCKER-A workaround）。仓库内的契约文件与实际执行工具不兼容，是一个真实缺陷：任何按文件原文调用 CLI 的人都会失败。

## Proposal

删除该 `$schema` 行，其余字节不变。JSON Schema 中 `$schema` 是可选的方言注解；契约的全部约束（required、类型、enum、additionalProperties）保持逐项不变——修改后文件与"原文件去掉该键"深度相等已用脚本核对。删除后 `--json-schema` 可直接接受文件原文，运行期副本步骤作废。

## Alternatives considered

### 不做：继续用运行期去键副本

最强理由是零仓库改动、零回归风险，且 BATCH-001 已证明该绕过可用。

不采用的理由：把一个已知缺陷长期固化成流程步骤。每个新管理会话都要重新发现并复刻这个绕过；交接文档必须一直解释"为什么派发脚本和契约文件不一样"；一旦有人直接按文档原样调用就报错。修复成本是一行删除，收益是消除整个绕过层。

### 换用 CLI 支持的其他 schema 方言声明（如 draft-07 的短 IRI 或去掉版本号）

最强理由是保留方言注解的自描述性。

不采用的理由：本机 CLI 的解析器对 `$schema` 键本身拒绝（不是拒绝 draft-07 语义），任何该键的合法取值都会被拒。试验其他取值没有证据支持能通过，且违反"不为绕过引入未验证的新写法"。

### 把契约从 JSON Schema 换成其他结构化输出机制

最强理由是彻底摆脱工具兼容性问题。

不采用的理由：破坏性变更，超出单行修复的授权范围；EXECUTION-CONTRACT 的输出约定基于 JSON Schema，跨 agent 契约不应随单个 CLI 版本的解析怪癖重写。

## Acceptance criteria

- `docs/contracts/worker-result.schema.json` 不含 `$schema` 键；与原文件去掉该键后深度相等。
- 真实 CLI 探测：`--json-schema` 直接传入该文件原文，调用 `is_error=false`、`terminal_reason=completed`、结构化输出符合契约。
- 两个 Note 校验器通过。

## Risks

- 若未来 CLI 版本恢复支持 `$schema`，缺键也只是回退到"按默认方言解释"，无行为损失。
- 派发脚本若仍引用旧的运行期副本逻辑，会继续工作（副本内容与修复后文件等价）；应随本修复清理对副本路径的依赖，避免两套来源漂移。
