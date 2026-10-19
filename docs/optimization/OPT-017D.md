# OPT-017D：封面补全的覆盖面、预算与可观测性

Status: verified-ci
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

## 主控裁量与验收记录（2026-10-16）

- 编码会话 `code-opt017d-20261016`；独立只读复审 `audit-opt017d-20261016` 结论
  **PASS WITH FINDINGS**（P0×0 / P1×0 / P2×2 / P3×2），复审报告
  `tmp/optimization-20261008/OPT-017D/REVIEW.md`。
- 覆盖面：`articles_without_cover` 去掉 `source='direct'`，候选统一为「无封面 + 有 URL」，
  把服务端正文无图文章纳入补全。旧排除理由「服务端入库已兜底」经**真实入库管线**夹具
  证伪（direct / greader-Miniflux / greader-FreshRSS / fever 四形态，「正文有图 →
  封面来自正文首图、不进候选」由整轮 0 页面请求断言）。
- 预算与生命周期：20/轮、并发 2、单请求超时 30s（具名常量）、扫描 4 页（80 行）、
  60s 间隔；负缓存上限 1024 插入序逐出；`ShutdownToken`（watch）+ `RunEvent::Exit`
  ——退出后不发新请求、在途请求取消、round 等齐子任务返回。scheduler 全文件无
  `unwrap()/expect()/panic!`。
- 可观测：候选读失败 `warn` + 下轮重试（不压空集合）；写库失败 `error` +
  不记「已处理」（下轮重试）；抓取失败 `debug` 含 URL 与原因。
- 复审动作项已收口（返工轮）：P2-2 **候选窗口冻结**——引入持久扫描游标
  （负缓存命中/重复 URL/批次满都推进，读到空页回绕），使候选在有限轮数内被全覆盖，
  不再依赖重启；红绿由新变异 M7（去掉游标推进）证明。P2-1 修正 `refresh_existing_article`
  的 COALESCE 口径（「NULL 不覆盖；远端给出非空新图会覆盖」，严格只填空的是
  `backfill_article_content`）——四处正文 + 同源两处统一。P3-1 join 错误改为显式
  `match` + `error!`（不静默，该 URL 不记负缓存、下轮重试）。P3-2 补隐私边界披露
  （扩面后客户端直接请求服务端收录文章的第三方源站页面，IP 暴露面扩大）。
- 不回归：`clear_article_cover_if_matches` 保守语义未动（仅注释）；服务端已有封面不被
  覆盖有断言；REQ-106① 五例保留并更新 API。
- 门禁 5 项 exit 全 0：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test`（lib 369/369、`cover_backfill_e2e` 15/15、46 target 全绿）、`npm run lint`、
  `npm run build`。变异 M1-M7 含 `diff -q` 逐字还原校验。
- 写集外最小增量（主控追认）：`lib.rs` 退出接线（~12 行，卡内未列但「退出不残留任务」
  必须在此触发）、`tests/mock_greader.rs` 新增默认关闭的 `fever_serve_entries`、
  `docs/roadmap.md` 2 处过时条目就地更新——逐条核对无行为副作用。
- 遗留（不以此卡关闭）：真实库候选存量未取数（吞吐上限 ≈1200 URL/h ≈28.8k/天，
  唯一 URL）；质量启发式（只取 og:image）；服务端本地补封面不可回清；强杀退出
  （非 `RunEvent::Exit`）不保证；真实外网抓取未测（用本地 mock）。
- 2026-10-16：提交 `9e49e3d` 推送 dev；提交级 CI run 226
  （[38872257111](https://github.com/Dchean/fluxreader/actions/runs/38872257111)）
  completed success（7m56s）。OPT-017D 状态：verified-ci。