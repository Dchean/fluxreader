# Codex CLI 作为执行器

管理者先确认此执行器是用户选择的，然后在目标机器只读核对 `codex --version`、`codex --help`、`codex exec --help`，并确认已有登录方式、模型选择、工作目录、配置/MCP/规则来源和宿主进程控制能力。不要复制某台机器的绝对路径或令牌。

官方非交互文档已于 2026-09-13 查阅：[Non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)。参数可能变化，执行前以本机能力和当前官方说明核对；本包没有实际启动模型验证目标环境。

## 任务调用形状

用程序参数数组传递，不把任务内容拼接为 shell 命令。以下是经过能力核对后组装的形状，尖括号须替换为本次运行的实际值：

```text
codex exec
  --sandbox workspace-write
  --json
  --output-schema <worker-result.schema.json文件路径>
  --output-last-message <本次运行的worker-result.json路径>
  -
```

完整任务包通过 UTF-8 stdin 传入；cwd 指向已核对的候选工作区。审批策略使用宿主或 CLI 支持的非交互配置，遇到权限问题返回管理者，不自动改为 danger-full-access、忽略规则或绕过批准。是否使用 ephemeral、显式模型或配置隔离，按本机支持和用户既有选择决定。

`--json` 的 stdout 是事件 JSONL，不是最终结果 JSON。保存原始事件和 stderr；`--output-last-message` 保存最终结构化输出。两个文件用途不同，不能把整个事件流当成一份 worker-result 解析。

`--output-schema` 接收文件路径。使用 [worker schema](../contracts/worker-result.schema.json)，结束后再由管理者验证任务 ID、运行 ID、结构、范围和实际结果。不要把 schema 成功当成代码正确。

默认复用已有认证。若自动化需要新的 API 认证，必须按项目权限准备，不能静默改变计费方式。不要让普通产品测试继承模型凭据环境。

新建空项目没有 Git 时，先确认是否允许本地初始化 Git 或采用明确快照；不能悄悄以跳过 Git 检查解决未知工作目录的问题。

## 首次接入验收

使用已授权、低风险、可验证的小任务检验真实调用、最终结果、超时/取消和权限拒绝；只跑 --help 或查看登录状态不算闭环。失败保留原始结果，同一任务继续累计时间和尝试。工作区写权限不等于逐任务路径白名单，管理者仍需检查全部差异。
