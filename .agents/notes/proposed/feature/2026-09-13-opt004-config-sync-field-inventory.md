# Agent Note: OPT-004 配置同步字段清单与同步边界

Status: proposed

## Problem

DEC-008/DEC-009 已确认 OPT-004 保留且范围为"仅同步订阅源、客户端设置与非敏感连接配置；排除 API Key、密码等凭据"，但实现缺少一份逐字段的权威清单：哪些字段进同步载荷、哪些必须排除、依据是什么。没有这份清单，实现任务无法定义验收，凭据泄露风险无法逐项核对。

## Proposal

以 TASK-011 只读分析产出的字段清单为准（64 个字段/字段组，证据逐条到文件:行），完整清单存于 [docs/runs/OPT-004-field-inventory-20260913.json](../../../docs/runs/OPT-004-field-inventory-20260913.json)。分类结论：

- **sync（38 项）**：订阅源数据（feeds/folders：URL、标题、分类、布局、AI 开关等 13 项）、客户端设置（app_settings JSON 子字段 21 项 + 独立 settings 键）、非敏感连接配置（sync_protocol / endpoint / username 等 7 项）。
- **exclude（24 项）**：全部凭据类——`greader_password`、`config_sync_credentials`（Gist PAT / WebDAV 密码）、`ai_config`（API Key）、`miniflux_token`，以及文章正文、阅读进度等本机内容数据（不属于配置同步语义）。
- **undecided（2 项）**：`folders.collapsed`、`app_settings.closeToTray`——纯本机 UI 偏好，建议 exclude（不构成有价值的同步对象），留实现任务定稿。

实现必须以"白名单"方式组装同步载荷：只有显式列为 sync 的字段进入载荷，新增字段默认不同步，直到清单更新。这比黑名单（排除已知敏感项）安全：新凭据字段默认不会漏进同步。

## Alternatives considered

### 不做：维持现有 config_sync.rs 的字段处理

最强理由是现有实现已在工作且经过 config_sync_e2e 测试，改动有回归成本。

不采用的理由：现有实现没有与 DEC-008/009 逐字段对应的可核对清单，"哪些字段被同步"分散在代码里；凭据排除靠零散判断而非结构保证。TASK-011 分析显示 4 个凭据字段当前处理正确，但这不能防止未来新增字段时出错。

### 黑名单：同步除已知凭据外的全部字段

最强理由是字段覆盖省心，新客户端设置自动获得同步。

不采用的理由：新敏感字段（例如未来新增的 WebDAV 密码变体）默认进入同步载荷，漏改一行就是凭据外泄。与 OPT-004 的隐私边界意图相反。

### 等真实多设备需求出现后再做

最强理由是当前没有第二个设备在用，同步价值未验证。

不采用的理由：DEC-008/009 已确认功能保留，范围问题已由用户裁决，设计清单是实现的必要前置；本任务只产出设计不写实现，成本已控制在最小。

## Acceptance criteria

- 字段清单完整覆盖 SQLite settings 键、app_settings JSON 子字段、feeds/folders 列、前端持久化模型，证据到文件:行（管理 agent 已抽查 4/4 一致）。
- 凭据类全部 exclude；白名单原则写入实现任务验收。
- undecided 项在实现任务启动前定稿。
- 本 Note 随 OPT-004 实现任务同批转为 implemented。

## Risks

- 清单基于静态代码分析（TASK-011，Claude 汇总 + 管理 agent 抽查），可能遗漏动态构造的 settings 键；实现任务的第一个验收项是"实际组装的同步载荷与清单逐项对照"。
- `app_settings` JSON 是自由结构，新增子字段不会被类型系统强制进入清单流程；靠清单文件与验收对照约束。
