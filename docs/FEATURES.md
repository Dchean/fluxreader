# 功能与保留清单

源码基准见 [BASELINE](BASELINE.md)。下表是首轮静态盘点，不是全部功能的运行验收。用户决定见 PRODUCT 的 DEC-008：核心必须保留，OPT-001～003、OPT-005～010 也全部必须保留。OPT-004 也按 DEC-009 保留，并调整为仅同步订阅源与客户端设置数据。

## 核心

| ID | 能力 | 当前实现线索 | 验证状态 |
| --- | --- | --- | --- |
| REQ-LAYOUT-001 | 文章、社交、画廊、播客、通知五布局 | src/types.ts 的 ContentLayoutType；components/Timeline.tsx；store/selectors.ts | 类型值为 article/social/image/podcast/notification；桌面行为 NOT_RUN |
| REQ-SYNC-001 | Google Reader / Fever 同步，面向 FreshRSS / Miniflux | src-tauri/src/greader.rs、fever.rs、sync.rs；SettingsModal.tsx | 两协议实现存在；兼容组合与版本仍需确认和实测 |
| REQ-AI-001 | AI 摘要 | commands::ai_summarize、ai::stream_chat、src/store.ts | 流式与缓存相关代码/测试存在；本阶段 NOT_RUN |
| REQ-AI-002 | AI 翻译 | commands::ai_translate、sanitize.rs、Reader.tsx | 流式、缓存、渲染相关代码/测试存在；本阶段 NOT_RUN |

保留核心能力不代表每项现有异常处理都被认定为正确。阅读状态、协议能力差异和取消行为需要进一步形成验收用例。

## 现有扩展能力的保留决定

OPT 编号沿用原清单，以保持需求、任务和证据的关联；编号前缀不表示当前仍可选。OPT-001～010 均已进入必须保留范围，不需要再次询问是否保留；其中 OPT-004 使用 DEC-009 限定的数据范围。

| ID | 现有能力 | 源码线索 | 决定 |
| --- | --- | --- | --- |
| OPT-001 | 本地 RSS/Atom 直连抓取、后台刷新、失败退避 | ingestion.rs、scheduler.rs、commands::refresh_all_feeds | 必须保留（DEC-008，用户已确认） |
| OPT-002 | 全文提取和自动全文 | extraction.rs、commands::extract_fulltext、store.ts | 必须保留（DEC-008，用户已确认） |
| OPT-003 | OPML 导入/导出 | opml.rs、commands::opml_import/opml_export | 必须保留（DEC-008，用户已确认） |
| OPT-004 | 通过 Gist/WebDAV 同步订阅源与客户端设置；保留必要的 GitHub 设备授权入口 | config_sync.rs、github_auth.rs、SettingsModal.tsx | 必须保留（DEC-009）；允许地址、模型等非敏感配置，不同步 API Key、密码等敏感凭据 |
| OPT-005 | 音视频播放、播放条、Windows SMTC | PlayerBar.tsx、media.rs | 必须保留（DEC-008，用户已确认） |
| OPT-006 | 托盘、关闭行为、单实例、开机自启、窗口状态、新文章通知 | src-tauri/src/lib.rs、scheduler.rs、App.tsx | 必须保留（DEC-008，用户已确认） |
| OPT-007 | 全文搜索、筛选、快捷键、右键菜单 | db::search_articles、store.ts、ContextMenu.tsx | 必须保留（DEC-008，用户已确认） |
| OPT-008 | 图片代理、灯箱、内嵌富媒体 | imageProxy.ts、Reader.tsx、commands::fetch_image | 必须保留（DEC-008，用户已确认） |
| OPT-009 | 主题、外观、分栏调整等个性化 | SettingsModal.tsx、styles/、App.tsx | 必须保留（DEC-008，用户已确认） |
| OPT-010 | 缓存清理、去重相关选项 | commands::cache_cleanup、db.rs、SettingsModal.tsx | 必须保留（DEC-008，用户已确认） |

配置同步与文章已读/收藏同步不是同一能力。旧 README 声称存在独立 article_state_sync.rs；本次源码清单及模块注册未见该模块。config_sync.rs 有状态文件名常量，不足以证明完整状态同步链路存在，见 ISSUE-005。

后续任务、目标架构、测试保护和发布验收应覆盖核心能力与所有 OPT 项，其中 OPT-004 按调整后的数据范围验收。可以调整内部实现，但不能借重构缩减这些能力。

OPT-004 的目标范围是订阅源与客户端设置，不同步文章内容、媒体文件、已读/收藏状态或 AI 生成结果。此限制仅适用于 Gist/WebDAV 配置同步，Google Reader / Fever 的核心文章与阅读状态同步继续保留。

当前 SyncPayload 含 folders、feeds、app_settings、ai_config；这是现状，不直接等于新范围。用户已确认服务地址、模型等非敏感连接配置可以同步，API Key、密码等敏感凭据不得同步。后续建立明确字段清单，不能直接复制全部 settings、ai_config 或运行时状态；导入只应用允许的字段，不能通过整体替换清空本地未同步的凭据。本次只更新文档，不修改实际同步实现。

## 兼容性记录方式

服务端 × 协议 × 版本 × 操作能力分别记录。不能从“协议实现存在”推导所有组合均已验证；Fever 当前 quick_add 返回 unsupported，协议能力差异需要体现在界面与验收中。

配置凭据处理、HTML 清洗和账号数据边界属于实现中的重要约束，应在任何相关变更中评估，后续调整 OPT-004 时也需核对这些共享边界。
