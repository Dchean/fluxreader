# Agent Note: db.rs 按领域拆分为 db/ 子模块

Status: implemented

## Problem

`src-tauri/src/db.rs` 单文件 3516 行，同时承载迁移、连接打开、行结构体、folders、feeds、articles、
URL 规范化、settings、同步队列、同步映射与三个内嵌测试模块；`crate::db::` 及其 94 个公开符号
被 commands.rs、sync.rs、scheduler.rs、ingestion.rs、config_sync.rs、credentials.rs 与全部集成测试引用。

M2 处置方案（[2026-09-13-m2-module-disposition](../../proposed/architecture/2026-09-13-m2-module-disposition.md)）
把「职责聚合」列为 ISSUE-009 的主要证据，并给出「db.rs 内部模块化」的整理项。定位一个函数要在
3500 行里翻找，改动一次要通读上下文，评审面过大——这是维护成本问题，不是缺陷。

## Decision

把实现按领域移入 `db/` 子模块，`db.rs` 只留共享 import、模块声明与再导出：

- 实现模块：`migrations`（MIGRATIONS/open/backfill_url_norm）、`folders`、`feeds`、`articles`、
  `url_norm`、`settings`、`sync_queue`、`sync_map`。
- 测试模块：`commands_extraction_tests`、`sync_extraction_tests`、`dedup_tests`（作为 `db` 的直接
  子模块文件，使各自 `use super::*` 的解析结果与拆分前完全一致）。
- `db.rs` 用 `pub use <mod>::{...}` 逐个列出公开符号，不用通配再导出：公开面显式可审，
  且 `crate::db::name` 的全部既有调用路径保持零改动。
- 子模块不重复声明共享 import：`db.rs` 保留 `Connection/OptionalExtension/Migrations/M/
  HashMap/HashSet/Path/LazyLock/AppResult`，子模块以 `use super::*` 取得。

三条被实现推翻的细节，记录下来供下一个改这里的人参考：

- **宏不走 glob**：`use super::*` 会带来类型，但**不会**带来父模块 `use` 进来的宏，所以每个用到
  `params!` 的模块各自 `use rusqlite::params;`；有 `#[derive(Serialize)]` 的模块各自
  `use serde::Serialize;`。
- **模块名不得遮蔽外部 crate**：初版把 URL 规范化模块命名为 `url`，直接遮蔽了 `url` crate，
  导致 `url::Url` 解析失败；改名为 `url_norm`。
- **`MIGRATIONS` 只在测试构建被用到**：`credentials.rs` 的 `#[cfg(test)] mod tests` 引用
  `db::MIGRATIONS`。因此再导出写成 `#[cfg(test)] pub(crate) use migrations::MIGRATIONS;`——
  否则非测试构建会报 unused import，而 `-D warnings` 会把它变成门禁失败。

## Alternatives considered

- **保持单文件（不做）**：这是最强的一条反对意见。拆分是纯移动，收益只在「下一次改动」时才兑现；
  若不拆分，本次改动量为零、零风险，且 105 项测试的通过状态天然不变。之所以仍然拆：M2 处置方案
  已把该文件列为职责聚合的主要证据，且后续 TASK-024/025/026 要把同一模式用到 sync.rs、
  commands.rs、store.ts——先在 db.rs 用最小代价验证模式可行、再用证据决定是否扩大，比一次性
  铺开更稳。若这次试点没有让定位与评审变省事，应停在这里而不是继续拆其他文件。
- **用 `include!` 把片段拼回单文件**：改动更小，但 `include!` 破坏模块边界与 IDE 跳转，
  等于把问题藏起来，且 rustfmt/clippy 的模块级诊断会错位。否决。
- **引入 ORM（Diesel/SeaORM）替代裸 SQL**：能从根上消除「SQL 散落」的抱怨，但要重写全部 SQL、
  引入依赖与迁移框架，收益（结构）远小于成本与风险，且 SQLite 个人场景下裸 SQL 更直接。否决。
- **只把三个测试模块移出去、实现留在 db.rs**：改动量最小，db.rs 从 3516 降到约 2215 行。
  但这没有解决实现侧的领域混杂，是半成品。否决。

## Consequences

- 定位从「3500 行里翻找」变成「按领域进模块」；测试与实现分离，`db.rs` 只剩 43 行声明与再导出。
- 公开符号集合不变（94 个，模块间无重名）；`crate::db::` 调用方零改动。
- 行为不变：`cargo test` 105 passed / 0 failed / 23 ignored，与拆分前一致；fmt、clippy 均干净。
- 代价：`use super::*` 让共享 import 的来源变成隐式的——子模块顶部不再自解释依赖了什么；
  宏需显式 import 这一条也是隐式规则，容易在新模块里踩到。
- 代价：`MIGRATIONS` 的可见性从「模块内定义」变成「`#[cfg(test)]` 再导出」，读代码时要多跳一层。
- 未处理：其余 M2 整理项（sync.rs 分层、commands.rs 命令组拆分、store.ts 拆 slice）仍留在
  [处置方案](../../proposed/architecture/2026-09-13-m2-module-disposition.md) 中，本 Note 只覆盖 db.rs 部分。
