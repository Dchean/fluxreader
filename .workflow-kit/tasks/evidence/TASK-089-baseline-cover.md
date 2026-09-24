# TASK-089 基线证据：封面补全饥饿与死链（修前状态）

记录时间：2026-09-23（主控复核）。本文件是修前的事实快照，供 test_review 的 baseline 引用；
**它不包含任何修复后的结论**。

## 1. 代码事实（逐行核对，当前工作区 = 候选）

- `src-tauri/src/scheduler.rs:290` `COVER_BACKFILL_BATCH = 20`；`:292` 并发 2；`:294` 每轮间隔 60s。
- `src-tauri/src/db/articles.rs:674-687` `articles_without_cover`：`WHERE (image_url IS NULL OR image_url='') AND url IS NOT NULL AND url != '' AND source='direct' ORDER BY published_at DESC LIMIT ?1`。
  ⇒ 每轮只可能看到**最新 20 条**候选；`source='direct'` 之外（miniflux 4714 篇中 757 篇无封面）永不进入该队列。
- `src-tauri/src/scheduler.rs:313-327`：先 `articles_without_cover(20)`，再在内存里过滤掉 `tried` 集合中的 URL；
  `targets.is_empty()` ⇒ `sleep(60s)` 后**继续循环，仍取同一批最新 20 条**。
- `src-tauri/src/scheduler.rs:374-379` `backfill_cover_once` 开头：**发请求之前**就把 URL 插入 `tried`；
  任何失败（网络/无 og:image/超时，见 `:381-400`）只返回 `Ok(false)`，条目仍留在 `tried` 中。
  ⇒ 前 20 条候选一旦全部失败，本进程内第 21 条及以后**永不被尝试**（窗口不推进）。
- `src-tauri/src/scheduler.rs:403-410` 落库：`UPDATE articles SET image_url = COALESCE(image_url, ?1) WHERE id = ?2`
  ⇒ 只填空、不覆盖；`articles_without_cover` 也只选 `IS NULL/''` ⇒ **已写入但失效（404）的封面没有任何纠正通道**。
- 前端：`src/components/Timeline.tsx:565-573` 是全仓唯一 `proxyImageUrl` 调用点（画廊卡片）；
  文章卡 `:374`、播客卡 `:685`、`PlayerBar.tsx:216/332`、`Overlays.tsx:338` 直连 + `referrerPolicy="no-referrer"`，且均无 `onError`。

## 2. 真实库测量（本次只读探针，未写任何行）

探针：`tmp/probe-cover-candidates.py`（`file:...?mode=ro` 打开 `%APPDATA%\com.fluxreader.app\fluxreader.db`）。

```text
articles 总数: 4992
按 source: [('miniflux', 4714), ('direct', 278)]
无封面（全 source）: 809   其中 direct（当前补全队列口径）: 52
候选按域名: [('kirikira.moe', 20), ('hpx.tw', 20), ('iconmoon.com', 6), ('www.douban.com', 4), ('immmmm.com', 2)]
最新 20 条候选的域名: 全部为 kirikira.moe
第 21-40 条候选的域名: douban×4, immmmm×2, hpx.tw×14
已有封面按图片域名 Top10: [('cdn3.ldstatic.com', 1431), ('image.woshipm.com', 1232), ('cdn.ldstatic.com', 441),
  ('cdnfile.sspai.com', 322), ('inews.gtimg.com', 201), ('s.anyway.red', 196), ('tu.aixq.cc', 127),
  ('static3cdn.appcdn.appinn.com', 125), ('imgslim.geekpark.net', 30), ('avatars.githubusercontent.com', 30)]
```

判读：**最新 20 条候选全部落在单一域名 kirikira.moe**，而审计 round-4 对该域实测 3 次请求全部 403
（`.workflow-kit/docs/AUDIT-20260923-round4-covers-and-feature-matrix.md` §1-B）。按 §1 的代码事实，
这 20 条失败后 `tried` 已满且窗口不推进 ⇒ 本进程内不再尝试任何候选。

## 3. 审计已测但本次未复测的事实（引用，不冒充本次测量）

- kirikira.moe 实测 403×3；同批候选第 27-48 位的 hpx.tw 实测 HTTP 200 且含合法 `og:image`（round-4 §1-B）。
- 逐行转写仿真：18 轮 = 20 次请求 / 0 封面 / 32 条从未被尝试（round-4 §1-B）。
- 死链率：`cdn3.ldstatic.com` 30 个真实 URL 中 1 个 404（≈3.3%，对应真实库 1092 篇里约 36 篇永久破图）（round-4 §1-D）。
- 防盗链：`cdnfile.sspai.com` 无 Referer 403×12/12、`img*.doubanio.com` 418×20/20，带 Referer 200（round-4 §1-C）；
  本库现网有 322 篇封面来自 `cdnfile.sspai.com`（上表），即受影响的真实规模。

## 4. 本卡修前复现计划（确定性、离线）

新增 `src-tauri/tests/cover_backfill_e2e.rs`：本地假 HTTP 服务提供
(a) 前 K 条候选对应的文章页一律 403/无图；(b) 第 K+1 条之后的候选返回含 `og:image` 的页面。
在**同一进程内**驱动补全循环（暴露可测试的循环体，或按同样的批次/负缓存语义驱动），断言第 K+1 条候选最终拿到封面。
修前：第 21 条及以后永不被请求（断言失败）；修后：通过。
死链纠正：先给一条文章写入已知失效 URL，调用纠正通道后再触发补全，断言新封面被写入且旧值被替换；
URL 不匹配时断言不动（幂等且不可越权）。
