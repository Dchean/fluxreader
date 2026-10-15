# OPT-017B：搜索路径与 FTS 写放大处置

Status: queued
父卡：OPT-017（搜索、媒体与缓存性能）。基线：OPT-017A（`docs/performance.md`
「当前隔离 SQLite 基线」一节，2026-10-09）。

## 主控读码确认的事实（2026-10-16）

- `db/articles.rs::search_articles` 用 `title/body/summary/ai_summary/translated`
  的多词 LIKE 交集（词间 AND、转义 `%_\\`），**不消费 `articles_fts`**：全仓无
  `MATCH` 查询；`articles_fts` 只有写路径（迁移触发器 + `db/contents.rs` 全量重建）。
- 触发器 `trg_articles_fts_ai/ad/au` 与 contents 触发器维持这个无消费者索引，
  属纯写放大与磁盘占用；历史上其 UPDATE 触发器过宽曾触发 `malformed`（迁移内
  断言留有说明），是维护风险而不仅是成本。
- OPT-017A 数据（同一语料、仅 DB 调用）：50k×8KiB 搜索唯一词 1161ms、无结果
  1345ms，已超 `docs/performance.md` 的「视图切换/搜索端到端 > 1000ms」判定线；
  内容更新辅助批次每 500 条 B/C 中位差约 0.779ms/条（方向性，非外推收益）。

## 交付

1. **FTS 写放大处置（必做）**：追加迁移删除无消费者的 `articles_fts` 及其触发器
   （不改历史迁移），同步清理 `db/contents.rs` 的重建路径、`db/migrations.rs`
   对该触发器的校验断言与相关测试。旧库升级不得损失文章/正文/搜索命中。
   若结论是保留，必须给出与删除对照的量化理由（同构建同语料）。
2. **搜索性能（按证据决定）**：在同一固定语料、同一构建上重跑 50k×8KiB 搜索
   前后对照。若 LIKE 仍超判定线，实现候选索引方案（FTS5 `trigram` 或其他等值
   方案）并让 `search_articles` 在可用时走它、不可用/短查询回落 LIKE；**结果集
   必须与现行为等值**（id 集合、顺序、去重、字段覆盖、AI 字段命中、中文/英文、
   无结果/唯一词/常用词、转义字符）。不能等值时不得静默切换。
3. **对照测量**：临时把归档 driver 恢复到 `src-tauri/examples/`（同 017A 头注
   参数与隔离库纪律），跑同语料同轮次的搜索、`feed_counts` 与内容写入 A/B；
   结果与 017A 原始 JSON 分开落盘、标明构建（不覆盖旧样本）。
4. **文档**：`docs/performance.md` 追加本轮结论、构建、语料与未测边界；归属
   Note 原位更新（`docs/performance.md` 的所有权与 017A 结论保持可追溯）。

## 写集

- `src-tauri/src/db/migrations.rs`、`db/articles.rs`、`db/contents.rs` 及对应测试
  （`db/dedup_tests.rs`、`tests/*` 中引用 FTS 的用例）
- `docs/performance.md`、归属 Note
- 临时 driver：仅在 `src-tauri/examples/` 短驻，测完移回任务自己的 tmp 目录

不触及前端、同步协议、IPC 契约；不新增第三方依赖；不访问真实用户库/账号。

## 强制反例（先红后绿，证据落 tmp/optimization-20261008/OPT-017B/evidence/）

1. 搜索等值夹具：中文 2 字词、中文 4 字词、英文词、中英混合、无结果词、
   含 `%`/`_`/`\` 的转义词、AI 字段命中词——实现前后 id 序列逐字一致。
2. 迁移：含数据的旧库（v27/v28 建库后导入夹具）升级到新版本，文章总数、正文、
   搜索命中集不变；删除后 `sqlite_master` 无 `articles_fts`/触发器；重复升级幂等。
3. 写入不再维护 FTS：删除后写入路径不再产生 FTS 相关语句（以 `EXPLAIN`/触发器
   列表为准），历史 `malformed` 场景的复现用例转为绿或删除并说明。
4. 若引入候选索引：同一夹具下候选索引与 LIKE 的等价性断言（含 2 字中文回落到
   LIKE 的路径）必须显式覆盖；索引损坏/缺失时回落 LIKE 仍正确。

## 门禁

- `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test`（本地已可完整链接，不得沿用旧豁免）。
- 前端未改动：跑 `npm run lint`、`npm run build` 作最小回归确认；改动仅 Rust 时
  不强制全量前端回归，但要在 RESULT 说明理由。
- 测量与门禁输出存 `tmp/optimization-20261008/OPT-017B/`，不提交临时产物。

## 验收

- 迁移与搜索行为等值证据齐备；同一构建同语料的前后对照落盘；未测范围写成
  验收缺口而不是结论。不得用「已删除 FTS 所以更快」这类无对照声称。
