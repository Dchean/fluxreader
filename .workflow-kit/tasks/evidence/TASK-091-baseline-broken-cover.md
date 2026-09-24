# TASK-091 基线证据：失效封面无纠正通道（修前状态）

记录时间：2026-09-23（主控复核）。本文件是修前事实快照，供 test_review 的 baseline 引用。

## 1. 代码事实（逐行核对）

- `src-tauri/src/db/articles.rs` 的 `articles_without_cover`：只选 `image_url IS NULL OR image_url = ''`，且 `source = 'direct'`。
- `src-tauri/src/scheduler.rs` 的 `attempt_cover` 写入：`COALESCE(NULLIF(image_url, ''), ?1)` ⇒ **只填空**，不覆盖已写入的值（TASK-090 已把空串纳入可填范围）。
  ⇒ 一旦某条文章的封面 URL 失效（源站 404／防盗链失败／文件被删），库里的值既不会被系统重估，也没有任何入口能把它清掉，**永久破图**。
- 全仓无「图片加载失败」的上报通道：`grep -rn "onError" src/` 只命中 Sidebar 的 favicon（`src/components/Sidebar.tsx`），四类内容图片位（文章卡 `Timeline.tsx:374`、播客卡 `:685`、迷你播放条 `PlayerBar.tsx:216`、全屏播放器 `:332`、灯箱 `Overlays.tsx:338`）均无 `onError`。
- 后端也没有对应的 tauri 命令（`src-tauri/src/commands/articles.rs` 无 report/clear cover 相关命令）。

## 2. 真实库测量（本次只读探针，未写任何行）

探针：`tmp/probe-cover-candidates.py`（`%APPDATA%\com.fluxreader.app\fluxreader.db`，`mode=ro`）。

```text
articles 总数: 4992（miniflux 4714 / direct 278）
已有封面按图片域名 Top10: cdn3.ldstatic.com 1431、image.woshipm.com 1232、cdn.ldstatic.com 441、
  cdnfile.sspai.com 322、inews.gtimg.com 201、s.anyway.red 196、tu.aixq.cc 127、
  static3cdn.appinn.com 125、imgslim.geekpark.net 30、avatars.githubusercontent.com 30
```

判读：`cdnfile.sspai.com` 有 322 篇封面，而审计 round-4 对该域名实测「无 Referer 403×12/12、带 Referer 200×12/12」
（`.workflow-kit/docs/AUDIT-20260923-round4-covers-and-feature-matrix.md` §1-C）；这类封面在直连图片位（无代理）下会失败，
而失败的信号**目前没有任何出口**（无 onError、无上报命令、库里也不会被纠正）。

## 3. 审计已测但本次未复测的事实（引用，不冒充本次测量）

- 死链率：`cdn3.ldstatic.com` 30 个真实 URL 中 1 个 404（≈3.3%；对应真实库 1092 篇该域名封面里约 36 篇永久破图）（round-4 §1-D）。
- 真机 DOM 证据：文章布局下 sspai/doubanio 封面 `naturalWidth=0`（加载失败）、ldstatic 为 1（成功）；画廊同一张封面经代理以 `data:` URL 成功（round-4 §1-C）。

## 4. 本卡修前复现计划（确定性、离线）

1. 后端：`src-tauri/tests/cover_backfill_e2e.rs` 增加用例——
   - 一条 direct 文章，`image_url` 写成一个「已知失效」的 URL；
   - 断言当前没有任何 API 能把它清掉（本卡新增 `db::clear_article_cover_if_matches` + 命令后，用例改为：URL 不匹配 ⇒ 不变更；URL 匹配 ⇒ 清空；清空后补全轮次能写入新封面）；
   - miniflux 源行：报告后**不得**被清空（否则从「破图」退化为「无图且永不补全」）。
2. 前端（后续卡）：回归网断言四类图片位的 `src` 经 `proxyImageUrl` 且带 `onError` 回退；真机/浏览器验证「失败时有占位、成功时用 data: URL」。
