# Agent Note: OPT-004 配置同步字段清单与白名单实现

Status: implemented

## Problem

DEC-008/DEC-009 确认 OPT-004 仅同步订阅源、客户端设置与非敏感连接配置，排除 API Key、密码等凭据；但实现缺少逐字段权威清单，且旧 `SyncPayload` 整体携带 `app_settings` 与 `ai_config`（后者含 API Key），导入整体覆盖设置——凭据边界没有结构保证。

## Decision

以 TASK-011 的 64 字段清单（[docs/runs/OPT-004-field-inventory-20260913.json](../../../docs/runs/OPT-004-field-inventory-20260913.json)）为准，`config_sync.rs` 按白名单原则实现（TASK-013，BATCH-004）：

- 上传载荷白名单组装：分类（名称/布局/AI 标志/position）、订阅源（URL/标题/归属/布局/AI 标志/site_url/favicon_url）、`app_settings` 子字段（过滤 autoStart/closePromptShown 本地特定字段）、非敏感连接配置（sync_protocol/greader_endpoint/greader_username 经 `ConnectionConfig` 结构）。`ai_config` 整键从载荷移除。
- 导入为字段级合并：`merge_app_settings` 仅用远端字段覆盖本地（保留未同步子字段与本地特定字段），连接配置逐字段 Option 守卫应用；`greader_password`/`ai_config`/`config_sync_credentials`/`miniflux_token` 四个凭据键在 apply 路径无任何写入。
- 已存在订阅源在导入时更新白名单字段（此前"已存在跳过"）。
- 边界测试：`payload_excludes_all_credential_fields`（上传不含任何凭据）、`apply_preserves_local_credentials_and_updates_allowed_fields`（导入后本地凭据保持原值）。

## Alternatives considered

### 不做：维持旧载荷（app_settings + ai_config 整体同步）

最强理由是现有 config_sync_e2e 测试已通过，改动有回归成本。

不采用的理由：ai_config 含 API Key，整体同步把凭据放上远端；导入整体覆盖会清空未同步的本地凭据——正是 DEC-009 要排除的行为。

### 黑名单：同步除已知凭据外的全部字段

最强理由是字段覆盖省心，新增设置自动同步。

不采用的理由：新敏感字段默认进入同步载荷，漏改一处就是凭据外泄；白名单让新增字段默认安全。

### 导入子字段严格白名单（21 项逐一列举）

最强理由是把"远端注入任意 settings 子键"也堵死，边界最严格。

不采用（本轮）的理由：凭据是独立 settings 键，子字段黑名单已满足 DEC-009 边界且有测试锁定；21 项逐一维护成本高。列为加固跟进项。

## Consequences

- 收益：凭据排除从约定变为结构保证（apply 路径没有凭据字段的写入代码，测试锁死）；第二设备导入不再破坏本地凭据与本地特定设置；新增字段默认不同步，防止未来敏感字段漏进同步。
- 代价：旧版远端载荷中的 `ai_config` 在新版本导入时被忽略——跨设备 AI 配置（含模型名）不再经 Gist 同步，需在每台设备单独配置（DEC-009 允许同步模型/地址，但清单把 `ai_config` 整键列为 exclude，属已记录的保守取舍）。
- 代价：导入对已存在订阅源从"跳过"变为"更新白名单字段"，本地对同 URL 源的标题/布局改动会被远端覆盖（同步语义使然）。
- 加固跟进：`merge_app_settings` 对 app_settings 子字段用"排除 2 个本地字段"而非 21 项白名单；凭据边界不受影响，子字段白名单列为后续加固项。
