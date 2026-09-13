# 当前接口索引

源码基准见 BASELINE。本文是接口边界和定位索引，尚不是完整参数、错误码与兼容性契约。

## 前端到 Rust

入口：[src/lib/api.ts](../src/lib/api.ts)。命令注册：[src-tauri/src/lib.rs](../src-tauri/src/lib.rs)。大部分实现：[commands.rs](../src-tauri/src/commands.rs)。

| 分组 | 代表命令 |
| --- | --- |
| 分类/订阅 | list_folders、create_folder、list_feeds、add_feed、update_feed、delete_feed |
| 文章/阅读状态 | list_articles、get_article、get_articles、article_index、search_articles、set_read、set_starred、mark_all_read、feed_counts |
| 直连刷新 | refresh_feed、refresh_all_feeds |
| 同步 | sync_test、sync_save、sync_phase、sync_now、sync_status、sync_disconnect、sync_local_feeds |
| AI | save_ai_config、get_ai_config、ai_list_models、ai_summarize、ai_translate |
| 正文与导入导出 | extract_fulltext、fetch_image、opml_import、opml_export |
| 配置同步 | config_sync_save_credentials、config_sync_upload、config_sync_download、config_sync_apply、config_sync_status |
| 桌面 | resolve_close、media_update_full、media_stop |

具体参数、序列化和错误码以注册函数及前端封装共同核实。Rust AppError 具有 code/message 的错误表达，前端 extractError 提取可读信息。

AI Channel 的当前事件定义为 delta(data: string)、done、error(data: string)，见 commands::AiEvent。请求完成、取消、部分结果和缓存写入之间的完整契约仍需测试设计。

## 外部服务

功能范围见 FEATURES：核心与全部 OPT 项均保留。OPT-004 的 Gist/WebDAV 通道仅同步订阅源与客户端设置，不传输文章内容、媒体、已读/收藏状态或 AI 生成结果；Google Reader / Fever 的文章和状态同步继续保留。下表和 IPC 索引记录现有实现，目标字段清单须允许服务地址、模型等非敏感配置，并排除 API Key、密码等敏感凭据。

| 边界 | 实现 | 已核实内容与限制 |
| --- | --- | --- |
| Google Reader | greader.rs、sync.rs | 登录与订阅/条目/状态操作客户端存在；不等于所有服务端实现均兼容 |
| Fever | fever.rs、sync.rs | 协议客户端及分派存在；Backend::quick_add 对 Fever 返回 unsupported |
| OpenAI 兼容 AI | ai.rs、commands.rs | models 与流式 chat 路径；需要验证错误、分片、缓存和取消 |
| RSS/Atom / 全文 | ingestion.rs、extraction.rs | 外部抓取及正文提取 |
| GitHub Gist / WebDAV | config_sync.rs、github_auth.rs | 客户端配置传输；不同于核心协议同步 |

不在文档、任务包、测试产物中记录真实凭据或完整敏感配置。真实服务验证需要明确的测试服务与账号，不能复用默认生产配置。

## OPT-004 的目标数据契约（尚未实现）

- 同步范围：订阅源与客户端设置；服务地址、模型等非敏感配置可纳入字段清单。
- 不同步：API Key、密码及其他敏感凭据；文章内容、媒体文件、已读/收藏状态和 AI 生成结果。
- 当前 SyncPayload 直接包含 app_settings / ai_config 等配置内容，不能将现有对象原样序列化视为符合目标。具体任务需要显式定义允许的输入和输出字段。
- 导入仅应用允许字段；收到敏感字段时不得应用，未同步字段不能通过整体覆盖而清空本地凭据。对应正反例需进入测试计划。
- Gist/WebDAV 操作所需的本地凭据与 GitHub 设备授权属于访问通道，不是要同步的数据；核心 Google Reader / Fever 同步的文章/状态能力继续保留。

以上是用户范围决定对应的后续实现约束。本次没有修改 payload、接口或凭据处理代码。

## 待确认兼容性矩阵

| 服务端 | 协议 | 代码路线 | 本阶段实测 | 首轮验收要求 |
| --- | --- | --- | --- | --- |
| FreshRSS | Google Reader | greader.rs / sync.rs | NOT_RUN | 待 Q-COMPAT-001 |
| FreshRSS | Fever | fever.rs / sync.rs | NOT_RUN | 待 Q-COMPAT-001 |
| Miniflux | Google Reader | greader.rs / sync.rs | NOT_RUN | 待 Q-COMPAT-001 |
| Miniflux | Fever | fever.rs / sync.rs | NOT_RUN | 待 Q-COMPAT-001 |

tests 中存在以真实 Miniflux 为目标的 live 测试；本次没有执行，也没有核实其默认端点可用性。服务端版本、认证配置、支持/不支持的操作都应随矩阵记录。

## 完整契约的补齐顺序

先为下一项获准变更补齐其涉及的输入、输出、错误、幂等性和能力差异；用需求编号与测试用例关联。不要为了文档完整一次性手写所有接口的重复说明。
