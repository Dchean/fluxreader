# Claude Code CLI 作为执行器

先确认用户选择，再只读核对 `claude --version`、`claude --help`、已有认证和模型，以及当前工作目录的 settings、hooks、MCP、插件与自动加载行为。只记录必要的非敏感配置形状，不输出秘密值。

官方说明已于 2026-09-13 查阅：[Run Claude Code programmatically](https://code.claude.com/docs/en/headless)。本包提供调用约定，没有替目标环境执行登录或模型调用。

## 任务调用形状

```text
claude --print
  --output-format json
  --permission-mode dontAsk
  --tools Read,Edit,Write,Glob,Grep
  --allowedTools Read,Edit,Write,Glob,Grep
  --json-schema <schema的JSON内容>
```

cwd 为获准工作区，完整提示词通过 UTF-8 stdin 提供。`--json-schema` 接收 JSON 内容，与 Codex 的 schema 文件参数不同；用参数数组传递，尤其在 Windows 上避免 .cmd 的二次引号解析。

上面的形状还不是通用沙箱配置。执行前按本机支持补齐隔离与非交互要求：例如受支持的 `--permission-prompts none`、明确的 MCP 配置及 `--strict-mcp-config`、受限制的设置来源或 restricted 模式。检查实际能力，不把某版本特有参数硬套到其他版本；缺少等价隔离时记录阻塞或提交具体替代方案，不静默去掉限制。

官方说明 bare 模式会跳过自动加载，并不读取 OAuth/订阅登录。因此不能为消除环境错误自动加入 `--bare`；只有用户批准相应认证方式且明确提供所需上下文时才能使用。restricted/settings 行为同样需核对，不能把整份用户 env 复制进项目运行目录来“修好登录”。

结果 JSON 的 `structured_output` 承载 [worker schema](../contracts/worker-result.schema.json)；外层 result、is_error、permission_denials、使用量和退出码也要检查。费用字段是估算，不能当作账单。无结构化结果、权限拒绝或超时必须如实报告。

schema 使用可移植的基础约束。CLI 若不支持某关键约束，返回兼容性问题；对注解的适配需保存源 schema 与运行副本，并证明 required、类型、enum 等约束未降级。不要对 schema 执行失败直接删除验证。

## 首次接入验收

用一项已授权小任务验证真实编码、退出、原始结果、差异核对、独立工具验证及超时/取消。现有登录状态不代表 API 调用已成功，Read/Edit 白名单也不代表只允许指定路径。缺少 Bash 时机械命令由管理者按预定义动作执行，不让 worker 临时添加任意命令。
