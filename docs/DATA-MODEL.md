# 当前数据模型

源码基准见 BASELINE。事实来源：[db.rs](../src-tauri/src/db.rs) 的 MIGRATIONS 与行类型。本阶段未打开用户数据库。

## 持久化

应用通过 Tauri app_data_dir 定位 fluxreader.db。db::open 设置 WAL、synchronous=NORMAL、foreign_keys=ON、busy_timeout=5000，并应用迁移及历史凭据转换。

当前源码定义 13 个迁移步骤。此处描述代码定义，不表示已验证任意真实数据库升级成功。

| 实体 | 主要内容和关系 |
| --- | --- |
| folders | 分类名称、位置、布局、AI 标志、折叠状态和 remote_id |
| feeds | feed_url 唯一、所属分类、布局继承、AI 标志、抓取缓存/失败退避、remote_id、origin、origin_was_local |
| articles | 所属 feed、guid、URL、正文、媒体字段、AI 结果、已读/收藏、来源、发布时间；唯一约束为 feed_id + guid |
| settings | key/value；应用设置、AI 配置、协议和同步连接等 |
| sync_queue | 本地待推送操作；关联文章或 feed_url，含 action/payload |
| articles_fts | FTS5 外部内容索引及触发器；实际查询路线需与调用函数一起核实 |
| deduped_urls | 去重记账；与 articles.url_norm、remote_dup_ids 等共同参与去重和同步处理 |

删除级联、跨源去重和账号断开有业务后果，应追踪具体调用，不直接由表结构推导产品行为。

## 命名与契约注意事项

v13 将部分 miniflux_id / miniflux_dup_ids 列改名为 remote_id / remote_dup_ids，并将 feeds.origin 的 miniflux 值改为 remote。并非所有 miniflux 字符串都消失：例如 articles.source、前端实体和部分测试中仍有历史命名。不能批量替换所有字符串。

Rust FolderRow / FeedRow / ArticleRow / ArticleListItem 与前端 CategoryGroup / FeedItem / ArticleEntry 之间存在映射，见 src/lib/api.ts。前端 publishedAt 使用毫秒数，数据库行中有时间字符串字段；验收需要覆盖转换及空值。

同步游标记录包含时间和条目 ID 两类；切换协议时的处理需要专项测试，不能默认两种协议语义相同。

## 配置同步的数据边界（目标）

DEC-009 保留 OPT-004，但同步范围仅限订阅源与客户端设置，并允许服务地址、模型等非敏感配置。API Key、密码等敏感凭据以及文章、媒体、已读/收藏状态、AI 生成结果不进入 Gist/WebDAV 配置同步数据。

本地持久化的数据不等于可同步的数据。后续需为 folders / feeds / app_settings / ai_config 等建立明确字段清单；不能直接导出整份 settings 或配置 JSON。导入只更新允许字段，不应用敏感字段，也不因字段缺省清空本地凭据。此处是目标约束，当前 schema、payload 与实现未修改。

## 本轮兼容性决定

用户不要求首轮保留旧数据与配置，见 PRODUCT 的 DEC-004。现有迁移测试仍应作为当前基线的组成部分记录，不能在尚未批准的阶段删除。

未来若决定重建 schema，应在对应任务中明确新安装、升级、数据处理与回滚策略。当前仅记录现状，不修改迁移，不访问或删除实际应用数据。
