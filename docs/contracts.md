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
| 图片/媒体预算 | `commands/settings.rs` 的 `MAX_IMAGE_BYTES` + `src/lib/imageProxy.ts` 的 `IMAGE_LIMITS` | 图片代理、封面、画廊 | [性能基线](performance.md) 的预算段 | 前端 image-resources 域 + 后端代理测试 |
| 本地同步协议与对象布局 | `local_sync/*.rs` + `webdav_transport.rs` | 本地库同步、便携导出 | [备份与恢复](backup.md)、[同步兼容矩阵](sync-compat-matrix.md) 的 WebDAV 段 | `local_sync_e2e`、`segment2`、`database_tests` |
| 便携备份格式 | `local_sync/backup.rs` | 导出/预览/恢复、跨设备 | [备份与恢复](backup.md) | 便携 v4 专项 e2e |
| 门禁命令 ↔ CI job | `package.json` scripts + `.github/workflows/ci.yml` | 本地与 CI、发布流程 | [开发与门禁](development.md) 的对应表（每项必须有承担者） | `docs:check` 不覆盖此表，靠评审 |
| 文档索引与路由 | `docs/README.md` | 所有维护者 | 新增/改名文档时同批更新 | `npm run docs:check` |

## 用法

1. 改之前：在"唯一出处"列找到文件；若这次改动产生新契约，先加一行。
2. 改之后：按"改它要同步"列逐项处理，不要只改代码。
3. 交付时：提交信息写上契约名，便于回溯（例如 `fix(sync): …（契约：同步协议边界）`）。
