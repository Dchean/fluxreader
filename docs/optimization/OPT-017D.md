# OPT-017D：封面补全的覆盖面、预算与可观测性

Status: queued
父卡：OPT-017。

## 主控读码确认的事实（2026-10-16）

- `db/articles.rs::articles_without_cover` 只选 `source = 'direct'`；`scheduler.rs`
  的注释说明理由是「Miniflux 源入库时已用正文第一图兜底」。
- 实际各后端入库都做过正文第一图兜底：`ingestion/parse.rs`（direct）、
  `sync/entries.rs:384,541`（服务端协议）均调用 `sanitize::first_image`；且
  `sync_map.rs` 回填 `image_url = COALESCE(...)`。
- 因此「服务端文章永不补封面」的旧表述过强：真实缺口是**正文里没有图片的服务端
  文章**——它们不会有任何封面，且永远不会尝试抓文章页的 og:image。
- `clear_article_cover_if_matches` 对服务端源保守不清空，是有意边界，保留。

## 交付

1. **覆盖面**：先以夹具证明各后端（direct / greader / fever / miniflux）在
   「正文有图」与「正文无图」两类下的现状；据证据把补全候选扩展到缺口集合
   （服务端无图文章），或给出不扩展的量化理由（如请求预算/隐私边界）。
2. **预算与生命周期**：候选抓取有批次上限、并发上限、单请求超时与失败缓存
   （失败不永久重试、不无限重试）；应用退出不残留任务（可取消/可等待终止）。
3. **可观测**：DB 读写失败必须日志/可诊断结果，不得压成空集合或 0 后把任务
   标记为已处理。
4. **文档**：归属 Note 与 `docs/performance.md`（如涉及预算）记录新语义与边界。

## 写集

`src-tauri/src/scheduler.rs`、`src-tauri/src/db/articles.rs`、必要时
`db/sync_map.rs` 与 `state.rs`；对应测试 `src-tauri/tests/*`、`db` 单测；
归属 Note。不触及前端；不改 `clear_article_cover_if_matches` 的保守语义。

## 强制反例（先红后绿）

1. 服务端无图文章进入候选、有图文章不重复抓取（夹具断言候选集合）。
2. 抓取失败（超时/404/无 og:image）在同一会话内不无限重试，且有失败缓存上限；
   失败被记录而非静默当作已处理。
3. 应用退出时任务可终止（测试用可控 shutdown 断言不再发起新请求）。
4. 服务端已有封面不被覆盖；`clear_article_cover_if_matches` 的现有保守行为不回归。

## 门禁

`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test`；前端未改动时跑 `npm run lint`/`npm run build` 作最小确认。
真实外网抓取不在无边界测试内，使用本地 mock 服务器。
