# OPT-017G：读闭包查询规划与批量标读成本

Status: queued
父卡：OPT-017。来源：OPT-017B 范围外发现（017B 复审 P3-4 判定「足以另立子卡」）。

## 已确认的事实（OPT-017B 测量与源码核对）

- `db/articles.rs` 读闭包路径（约 1772-1802）对每个条目执行
  `url_norm = ?2 AND is_read = 0 ORDER BY id`；现有索引是
  `idx_articles_unread ON articles(is_read) WHERE is_read = 0`（`migrations.rs:78`），
  不含 `url_norm` 列。
- 017B 同构建测量（50k×8KiB）：100 条标读 2511ms、1000 条 24846ms
  （≈25ms/条；20k×8KiB 为 793/7695ms），与「每次扫 ~33k 未读行」的假说一致。
- 017B 报告中「EXPLAIN 实测」原文未落盘（复审 P3-4 标注无法验证）——
  本卡第一步必须自行复现并留档，不沿用未验证结论。

## 交付

1. **复现与定位**：在隔离临时库（复用 017A/017B 语料生成方法）对读闭环路径跑
   `EXPLAIN QUERY PLAN` 并落盘；拆分 per-item 成本构成（定位失败则写「无法定位」
   与已排除项，不得编造）。
2. **按证据优化**：以索引（如 `(url_norm, id) WHERE is_read = 0` 等值候选）或
   等价查询改写消除全未读扫描；选择依据含索引体积与写入成本对照。**不得改变**
   同文读传播、独立收藏、去重开关等语义。
3. **同构建前后对照**：50k×8KiB 与 20k×8KiB 的 100/1000 条标读中位数；
   结果与 017A/017B JSON 分开落盘、标明构建。
4. **文档**：`docs/performance.md` 追加结论与未测边界；归属 Note 原位更新。

## 写集

`src-tauri/src/db/articles.rs`、`db/migrations.rs`（仅追加迁移）、相关测试、
`docs/performance.md`、归属 Note。不改前端、IPC、同步协议语义。

## 强制反例（先红后绿，证据 `tmp/optimization-20261008/OPT-017G/evidence/`）

1. 复现慢查询：同语料下先留旧实现的耗时/计划证据。
2. 语义等值：标读 1 条传播到同文其他条目、独立收藏不受影响、批量标读返回计数与
   状态迁移与旧实现逐字一致（既有回归 + 新增夹具）。
3. 升级/旧库：追加迁移在含数据旧库上幂等，索引存在性与查询计划断言；
   删除索引/损坏时行为仍正确（回落）。

## 门禁

`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test`；前端未改动时跑 `npm run lint`/`npm run build` 作最小确认。
变异/还原附「还原校验输出」日志。
