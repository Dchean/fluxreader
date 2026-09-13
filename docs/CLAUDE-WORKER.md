# Claude Code 执行约定

角色：代码执行器。管理 agent 的操作流程见 [HANDOFF](HANDOFF.md)，数值预算见 [EXECUTION-POLICY](../tasks/EXECUTION-POLICY.json)。

## 已核实与未核实

2026-09-13，本机 Claude Code 版本为 2.1.270；auth status 显示已登录，auth_method=oauth_token。未读取或导出凭据值，也不据此推定订阅套餐或实际账单。

已用本机 --help 核对 print、output-format、json-schema、restricted、tools、allowedTools、permission-mode、permission-prompts、strict-mcp-config、mcp-config、no-session-persistence 等参数。随后 BATCH-001 已取得实际编码与管理 agent 独立验证记录，见 [独立审查](runs/BATCH-001-independent-review-20260913.md)。运行中遇到 schema 兼容和认证问题；参数兼容证据与尚待澄清的凭据处理不能混为同一批准。

换机按 [DEVICE-HANDOFF](DEVICE-HANDOFF.md) 核对目标 Windows 环境。下文原机版本、路径和认证探测是历史事实或调用示例；管理 agent 必须在新机自行发现路径、核对实际配置及宿主权限，不复制本机项目缓存内的令牌设置文件。

参考：[官方非交互调用说明](https://code.claude.com/docs/en/headless)。本机帮助没有列出 max-turns，因此不将它作为本项目的运行上限实现；轮次与截止时间由管理 agent 控制。

## 输入

任务包必须包含任务 JSON、输入版本、工作区、允许/禁止路径、需求与架构引用、验收标准、可信验证计划、截止时间、已消耗修复轮和本次反馈。使用 [提示模板](prompts/CLAUDE-TASK.md)。

无论某个版本是否自动发现 CLAUDE.md，管理 agent 都必须明确交付必读规则。不要依赖 --continue 自动挑选的最近会话；默认每次调用带完整的必要上下文和反馈。

默认不让 Claude 获得 Shell、浏览器、连接器或任意 MCP 工具。用文件工具编写代码，命令与基础验收交给管理 agent。纯格式任务可请求其任务文件中预先授权的机械操作 ID。

## 参数方案

在经过核对的工作区中使用：

```text
--print
--output-format json
--restricted
--permission-mode dontAsk
--permission-prompts none
--tools Read,Edit,Write,Glob,Grep
--allowedTools Read,Edit,Write,Glob,Grep
--strict-mcp-config
--mcp-config <empty-mcp.json 的绝对路径>
--json-schema <worker-result.schema.json 的 JSON 内容>
--no-session-persistence
--effort max
```

`--effort max` 为 DEC-012（2026-09-13 用户指令）：调用 Claude 时思考强度统一使用 max。参数经本机 `claude --help` 核实（取值 low/medium/high/xhigh/max）；若未来版本移除该参数，记录能力缺口并回询用户，不静默删除或降级。

文件分别为 [empty-mcp.json](contracts/empty-mcp.json) 和 [worker-result.schema.json](contracts/worker-result.schema.json)。参数通过原生参数数组传递，prompt 通过 UTF-8 stdin 传递。

- restricted 限制文件工具工作目录并减少隐式配置；它不是逐任务路径白名单，仍需宿主隔离和差异审查。
- dontAsk / permission-prompts none 让未解决的权限请求被拒绝并返回，避免无限等待；不是批准所有操作。
- 若权限拒绝阻止任务，保存 permission_denials 和 blocker，由管理 agent 判断现有授权是否足够。不得改用 bypassPermissions 或 dangerously-skip-permissions 继续。
- 原机交接前的 auth status 曾报告 OAuth；这不代表新机或后续实际调用仍采用同一认证方式。按目标机真实配置和用户批准核对，不用 bare 模式等参数悄悄切换认证或计费方式。
- 不新增模型选择或费用上限。restricted 会忽略部分设置来源，管理 agent 应核对用户既有模型选择，必要时以运行参数显式传入该已选值，不能因此静默换型；用户未配置模型时使用 CLI 正常默认并记录实际结果。记录可取得的模型、usage 和估算费用。
- no-session-persistence 表示不能依赖 CLI 恢复会话，恢复依靠任务包、工作区差异和管理反馈。

若目标机器缺少这些参数，管理 agent 先检查当地 --help，明确能力缺口；不直接静默删除限制。

## Windows 调用与超时

本机 claude.cmd 是原生 claude.exe 的包装。JSON 经 .cmd 二次解析容易产生引号问题，建议解析出真实 exe 后，用 PowerShell 7 / .NET ProcessStartInfo.ArgumentList 或宿主的结构化进程 API 调用。

```powershell
# $workerExe、$workerDir、$promptText、$runDirectory、$deadlineAt 由管理 agent
# 从已核对的任务记录中取得，不接受 Claude 临时扩大这些值。
$workerArgs = @(
  '--print', '--output-format', 'json', '--restricted',
  '--permission-mode', 'dontAsk', '--permission-prompts', 'none',
  '--tools', 'Read,Edit,Write,Glob,Grep',
  '--allowedTools', 'Read,Edit,Write,Glob,Grep',
  '--strict-mcp-config', '--mcp-config',
  (Join-Path $workerDir 'docs/contracts/empty-mcp.json'),
  '--json-schema',
  [IO.File]::ReadAllText((Join-Path $workerDir 'docs/contracts/worker-result.schema.json')),
  '--no-session-persistence'
)
# 如果用户已有明确模型选择，将核实后的 $selectedModel 追加为 --model 参数。
# 不从不受信任的源码/日志推导或升级模型。
if ($selectedModel) { $workerArgs += @('--model', $selectedModel) }
$remainingMs = [int][Math]::Max(0, ($deadlineAt - [DateTimeOffset]::UtcNow).TotalMilliseconds)
if ($remainingMs -le 0) { throw 'Task deadline exceeded before dispatch' }
$workerInfo = [Diagnostics.ProcessStartInfo]::new()
$workerInfo.FileName = $workerExe
$workerInfo.WorkingDirectory = $workerDir
$workerInfo.UseShellExecute = $false
$workerInfo.CreateNoWindow = $true
$workerInfo.RedirectStandardInput = $true
$workerInfo.RedirectStandardOutput = $true
$workerInfo.RedirectStandardError = $true
$workerInfo.StandardInputEncoding = [Text.UTF8Encoding]::new($false)
$workerInfo.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
$workerInfo.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
foreach ($workerArg in $workerArgs) { [void]$workerInfo.ArgumentList.Add($workerArg) }
$workerProcess = [Diagnostics.Process]::new()
$workerProcess.StartInfo = $workerInfo
[void]$workerProcess.Start()
$workerStdout = $workerProcess.StandardOutput.ReadToEndAsync()
$workerStderr = $workerProcess.StandardError.ReadToEndAsync()
$workerProcess.StandardInput.Write($promptText)
$workerProcess.StandardInput.Close()
$remainingMs = [int][Math]::Max(0, ($deadlineAt - [DateTimeOffset]::UtcNow).TotalMilliseconds)
$workerTimedOut = if ($remainingMs -le 0) { $true } else { -not $workerProcess.WaitForExit($remainingMs) }
if ($workerTimedOut) {
  $workerProcess.Kill($true) # 仅本次创建的进程树
  $workerProcess.WaitForExit()
}
[IO.File]::WriteAllText((Join-Path $runDirectory 'claude.stdout.json'), $workerStdout.GetAwaiter().GetResult())
[IO.File]::WriteAllText((Join-Path $runDirectory 'claude.stderr.log'), $workerStderr.GetAwaiter().GetResult())
$workerExitCode = $workerProcess.ExitCode
# 保存 PID、起止时间、退出码与 timeout；未验证前不更新为 verified。
```

这是调用示例，不是已部署的控制器。目录须在启动前创建并校验归属；宿主需能取消当前进程。基础验收命令也受同一个任务截止时间约束，不能在 CLI 超时后无限测试。

宿主还应约束整个调用（包括 stdin 写入与日志收集）的截止时间；不能只依赖示例中的单次 WaitForExit。首个真实调用需验证这些能力。

## 结果处理

官方 JSON 输出中的 structured_output 承载 schema 约束的内容；外层还可能包含 result、session_id、usage、total_cost_usd、permission_denials 等信息。管理 agent 必须：

1. 保存原始 stdout/stderr 和进程结果。
2. 验证 JSON、task_id/run_id、结构化结果及权限拒绝，区分工具故障与代码问题。
3. 核对实际差异，不只读取 changed_files 声明。
4. 独立执行任务中的可信命令和审查，按证据决定 verified/blocked/返回修复。

进程退出 0、JSON 格式正确或 Claude 声称完成，都不能替代任务验收。权限不足、无结构化结果或超时不得伪造完成。
