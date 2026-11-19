# 契约单点表

改代码前先在这里查一行：同一个事实只允许有一个出处，改动要按"同步"列逐项核对。表里没列到的新契约，先补一行再改代码（规则见 [维护与文档纪律](maintenance.md)）。

| 契约 | 唯一出处 | 主要消费者 | 改它要同步 | 锁定方式 |
| --- | --- | --- | --- | --- |
| 数据库格式与结构 | `src-tauri/src/db/schema.sql`（权威）；版本与拒绝策略在 `db/migrations.rs` | 全部 `db/*.rs`、`local_sync/database.rs`、备份/恢复 | 形状探针 `current_shape`、[架构](architecture.md)、[备份](backup.md)；旧库不自动迁移 | `migrations.rs` 单测（格式 id、schema 版本、无旧升级步骤） |
| 行契约（Rust 序列化 ↔ TS 镜像） | `src-tauri/tests/fixtures/row_fixture.json` + `src-tauri/tests/row_fixture_e2e.rs` | `src/lib/api.ts`、`tools/frontend-tests/ui-contracts.mjs` | fixture、两端结构与映射、转换器 | `row_fixture_e2e`（Rust 序列化比对）+ ui-contracts 键集合/逐键值/比较器自检 |
| IPC 命令表 | `src-tauri/src/lib.rs` 的 `invoke_handler` | `src/lib/api.ts`、前端各 slice | 两侧命令名与参数、零调用账、[开发与门禁](development.md) | 无自动检查（靠评审与审计口径） |
| 同步冲突方向 | `src-tauri/src/sync/conflict_policy.rs` | `sync/greader_pull.rs`、`sync/entries.rs`、`sync/push.rs`、`sync/fever_pull.rs` | [同步兼容矩阵](sync-compat-matrix.md) 的政策段与锁定测试 | `conflict_policy` 单测 + 各 pull 的方向断言 |
| 同步协议边界（认证/分页/响应上限/退避） | `greader.rs`、`fever.rs`、`sync_http.rs` | `sync/*`、`ingestion/http.rs`（有界收体） | [同步兼容矩阵](sync-compat-matrix.md)、真实服务端 e2e 的断言文案 | `greader_compat_e2e`、`fever_compat_e2e`、`sync_http` 单测 |
| 设置名与值域 | `src-tauri/src/db/settings.rs`（含本机/同步/便携可见范围） | 设置页各 Tab、`src/store/settingsValidation.ts`、`patch_app_settings` | 前端校验区间、[架构](architecture.md) 的设置节 | `settings.rs` 单测 + 前端 settings 域断言 |
| 写事务与锁序 | `db/writer.rs` + `db::write_transaction` | 所有写路径、`state::database_work` | [架构](architecture.md) 的并发与写预留节 | `writer_tests` |
| 渲染安全边界 | `src-tauri/src/sanitize.rs`（写入口）+ `db/articles.rs` 的读边界 `sanitize_renderable_fields` | 阅读器、AI 译文落库、HTML 回包 | [架构](architecture.md) 的安全边界 | sanitize 单测 + 正文渲染断言 |
| 条目身份与去重键 | `db/identity.rs`、`db/url_norm.rs`、`db/articles.rs` 的 `SAME_RSS_GROUP` | 列表/搜索/读闭包/计数/同步匹配 | [同步兼容矩阵](sync-compat-matrix.md) 的身份表 | `dedup_tests`、`url_norm` 单测、查询计划回归 |
| 正文与 AI 产物身份 | `db/contents.rs` + `ai.rs` 的签名推导 | AI 缓存命中/验签、正文失效 | 前端 `ai-cache-signature` 域、落库前提校验 | `commands/ai.rs` 单测 + 前端签名域断言 |
| 图片/媒体预算 | 服务抓取上限在 `commands/settings.rs` 的 `MAX_IMAGE_FETCH_BYTES`，IPC 响应校验在 `src/lib/api.ts` 的 `MAX_IMAGE_IPC_BYTES`，在途预留和资源预算在 `src/lib/imageProxy.ts` 的 `IMAGE_LIMITS`（每请求预留 `reservationBytes`） | 图片代理、封面、画廊 | [性能基线](performance.md) 的预算段 | 前端 image-resources 回归 + 后端代理测试 |
| 本地同步协议与对象布局 | `local_sync/*.rs` + `webdav_transport.rs` | 本地库同步、便携导出 | [备份与恢复](backup.md)、[同步兼容矩阵](sync-compat-matrix.md) 的 WebDAV 段 | `local_sync_e2e`、`segment2`、`database_tests` |
| 便携备份格式 | `local_sync/backup.rs` | 导出/预览/恢复、跨设备 | [备份与恢复](backup.md) | 便携 v4 专项 e2e |
| 远端物理删除关闭与对象保留 | `local_sync/runtime.rs` 的 `maintain_local_gc`（回收编排，有意不发远端 DELETE）；删除边界复核在 `local_sync/storage.rs` 的 `can_remove_path`（当前无生产调用者） | local_sync 工作队列与 journal、便携导出 | [备份与恢复](backup.md)、[同步兼容矩阵](sync-compat-matrix.md) 的 WebDAV 段、[当前缺口](roadmap.md) 的「远端物理删除关闭」条目与「维护边界」 | 现有 `local_sync_e2e` 与「有意暂停」的 Note/roadmap 条目；没有专门断言「不删除远端对象」的测试 |
| 同步正文对象大小上限 | `local_sync/model.rs` 的 `MAX_BODY_BYTES`（32 MiB，`html` + `text` 字节合计，primary 与 rss 分别计）；传输层上限在 `webdav_transport.rs` 的 `XML_LIMIT`、`STATE_LIMIT`、`BATCH_LIMIT` | local_sync 正文通道、便携导出与恢复 | [同步兼容矩阵](sync-compat-matrix.md) 的 WebDAV 段、[备份与恢复](backup.md)、[性能基线](performance.md) | 32 MiB 本身无专门断言；`http_budget_e2e` 的 `webdav_body_cap_rejects_length_and_eof_oversize_and_accepts_normal_json` 覆盖有界收体行为（用例自带上限，未绑定上述常量） |
| 订阅写回队列（outbox） | `db/outbox.rs`（合并同账号同 URL 换新 id、未发送链保留链起点、`abandon_stale_outbox` 的账号屏障、`STATUS_LOCAL_ONLY` 终态）；`account_key` 推导在 `sync/credentials.rs` 的 `account_key`，经 `sync/mod.rs` 重导出 | `sync/subscriptions.rs`、`commands/{sync,folders,opml}.rs`、`db/{feeds,folders}.rs` | [架构](architecture.md) 的同步边界、订阅写回 Note | `db/outbox.rs` 的 `t_opt009_tests` 模块（如 `generation_rebind_only_for_same_account_key`、`unsubscribe_inherit_is_account_key_scoped`） |
| 正文保留与释放（本机） | `db/body_retention.rs`：`retention_days` 读 `readBodyRetentionDays`（0 = 永久、上限 3650）、`quiescent` 门、`release_qualified` 判定 | `scheduler.rs` 的 `retire_read_bodies_slice` 每分钟 slice；`commands/sync.rs` 的 `cache_cleanup` 手动清理（按发布年龄，天数 1–3650） | 本表「设置名与值域」行、[架构](architecture.md)、[备份与恢复](backup.md) | `body_retention_tests.rs` 的 `retention_*` 用例族（默认 0 与边界、slice 公平性、候选与 hold 的有界索引访问） |
| AI 配置、密钥、产物与本机身份的同步与便携排除面 | `db/settings.rs` 的 `SYNC_SETTING_KEYS` / `PORTABLE_ONLY_SETTING_KEYS` / `DEVICE_SETTING_KEYS`（本机/同步/便携可见范围，经 `local_sync/model.rs` 重导出为 `SETTINGS`）；凭据在 `sync/credentials.rs`；便携导出只取白名单、恢复时清掉凭据键在 `local_sync/backup.rs` 的 `SETTINGS` 与 `clear_for_restore` | settings 命令（`patch_app_settings`）、local_sync 导出、AI 命令 | [备份与恢复](backup.md)、[架构](architecture.md) 的设置策略 | `db/settings.rs` 的 `tests` 单测（`set_setting_propagates_encrypt_failure_and_leaves_db_untouched`）；`local_sync/database_tests.rs` 的 `portable_v4_deduplicates_content_and_restores_preferences_with_fresh_ids`（恢复后无 `greader_password`/`ai_config`/`local_sync_config`）与 `settings_patch_preserves_unobserved_remote_keys_and_rejects_partial_invalid_updates`（本机键不入同步字段） |
| 门禁命令 ↔ CI job | `package.json` scripts + `.github/workflows/ci.yml` | 本地与 CI、发布流程 | [开发与门禁](development.md) 的对应表（每项必须有承担者） | `docs:check` 不覆盖此表，靠评审 |
| 文档索引与路由 | `docs/README.md` | 所有维护者 | 新增/改名文档时同批更新 | `npm run docs:check` |

## 用法

1. 改之前：在"唯一出处"列找到文件；若这次改动产生新契约，先加一行。
2. 改之后：按"改它要同步"列逐项处理，不要只改代码。
3. 交付时：提交信息写上契约名，便于回溯（例如 `fix(sync): …（契约：同步协议边界）`）。
