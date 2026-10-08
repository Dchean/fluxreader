# fluxreader 审计第三轮（真机 DOM 验证 + 独立复核）

- 基线：`git HEAD = d6234e5`（main；工作区仅 `.workflow-kit/notes/*` 与本文档未提交）
- 授权与口径（用户本次指令）：**不跑任何门禁**（修复时再跑）、**改用后端模拟数据实测**、**证伪优先**、
  **不必重复上一轮已完成的审计**、**mock 必须与真实数据一致**
- 上一轮报告：[AUDIT-20260922-shells-and-regressions.md](AUDIT-20260922-shells-and-regressions.md)（2×P0 / 7×P1 / 20×P2 / 18×P3）
- 本轮新增能力：**真浏览器（Chrome + CDP）+ 忠实假后端**。上一轮遗留的 5 条「未证实项」全部由静态推理
  或 store 层探针得出，本轮第一次在**真实 DOM + 真实 React effect + 真实键盘事件**下取证。

## 0. 本轮为什么能拿到上一轮拿不到的证据

| | 上一轮 | 本轮 |
| --- | --- | --- |
| 前端运行环境 | `dist-test/store.js`（无 DOM、无 effect） | 真实 Chrome 加载 `dist/` 生产包（真 DOM、真 effect、真键盘） |
| 后端 | 各审计员手写假后端（行为不一，曾因**mock 比真实实现更正确**制造出生产不存在的现象） | 单一假后端，逐条注明源码依据：schema 取自**用户真实库**（只读副本）、SQL 逐字复刻、参数反序列化按 `rename_all="camelCase"` 的**实际后果**实现、时间格式照抄真实行 |
| 断言对象 | 变量级 / 参数级 | **集合级**（被写入的 id 集合 vs 用户可见的 id 集合）+ 数据库侧真值 |

**忠实性纪律（本轮事故与修正，必须记录）**：本轮首次运行时出现「成功的『全部已读』同时弹出失败 toast」，
排查发现是**我的假后端自身的 bug**（`sync_queue` 列名写错 → 入队失败），而非产品缺陷；同时暴露出
Python 的隐式事务与 rusqlite 的自动提交语义不同（会造成「失败后仍有半批写入」的假象）。
两处均已按真实实现修正（`db/sync_queue.rs` 的列与互斥 DELETE、`isolation_level=None` 对齐自动提交），
修正后该现象消失。**教训与上一轮一致：mock 的任何一处不忠实都会变成一条假发现。**

- 复现方式：`node tmp/audit-r3/harness/s1_markallread.mjs`（其余 `s2_cascade` / `s3_ai` / `s4_ui` / `s5_ui2` 同理）
- 证据与脚本：`tmp/audit-r3/harness/`（`dist` 为 2026-09-22 16:20 的生产构建，晚于 `src` 最后修改 10:37）

---

# 一、用户报告的现象：确认，且比「跨布局」更宽

## 1.1 「全部标为已读」把其他布局的订阅源一并标读（= 上一轮 P1-1）——DOM + DB 双证

真机（文章布局、范围=全部订阅源、视图=全部）实测：

```
可见卡片 = 14 张（其中未读 3 张）
点击「全部已读」后：后端 is_read 0→1 的 id = 74 条
  · 其中不属于「用户可见集合」的 = 71 条
  · 按生效布局分布 = { article:32, social:33, notification:4, podcast:3, image:2 }
toast = 「已全部标为已读」
```

**用户报告完全成立**：在文章布局点一次按钮，社交（33 条）、通知（4 条）、播客（3 条）、画廊（2 条）
四个**其他布局**的订阅源条目全部被标读，并逐条进入 `sync_queue` 推送远端。按钮文案是「将当前列表全部标为已读」，
而「当前列表」只显示文章布局的条目。

## 1.2 列表为空时点按钮，同样写库（= P0-2）——真机复现

真机（通知布局 + 收藏视图，收藏条目全部属于文章布局 ⇒ 该组合下列表为空）：

```
DOM 卡片 = 0，空态文案 =「暂无收藏内容」
点击「全部已读」→ 后端被写入 3 条（全部属于文章布局，用户一条也看不到）
toast = 「已全部标为已读」
```

## 1.3 「今天」视图的边界条目被标读却看不见（= P1-2）——真机复现

fixture 含 2 条「明天」发布的条目（`2026-09-23T02:00:00+00:00`，本地 10:00）：

```
今天视图 DOM 卡片 = 14，其中「未来条目」可见 = 0（列表按本地日期相等，明天 ≠ 今天）
点「全部已读」→ 写入 8 条，其中含上述 2 条未来条目（标读只判下界 >= 本地零点）
```

即：**看不到的条目被标读**。与上一轮两条独立探针结论一致。

## 1.4 【本轮新发现 R3-1】标读后**其他布局的本地列表陈旧**（后端已读、界面仍显示未读）

上一轮把「跨布局陈旧」记为 P2-7（快照回退），两位审计员结论互相矛盾。本轮真机实测的**真实形态**不是回退，而是**陈旧**：

```
文章布局：首卡 id=3004 未读
→ 切社交布局 → 点「全部已读」（后端未读 74 → 0）
→ 立刻切回文章布局：未读卡片 = 3 / 14，id=3004 仍显示未读
```

机理：`markCurrentViewAllRead`（nav.ts:151-173）本地只标「当前布局可见集合」（`markEntriesRead(visibleIds)`），
而远端/数据库按「范围」标读。两者口径不同 ⇒ **同一次操作在界面上留下两种状态**：
别的布局被静默读掉（用户抱怨的那一半），当前布局之外**看不到的**本地条目又保持未读（这一半）。
任一次 reload（切视图命中缓存后的后台刷新、后台 `feeds-updated` 事件）才会纠正。

> 与 P2-7 的关系：P2-7 描述的「切回旧布局时已读状态可见回退」在真机上**没有复现**（切回即正确，
> 缓存恢复与后台刷新都在切回后极短时间内完成）。本轮把它**替换**为可复现的 R3-1（陈旧而非回退），
> 并把它归入 P1-1 的同一根因家族（本地可见集合 ≠ 后端标读集合）。

---

# 二、上一轮 5 条「未证实项」的裁决（真机取证）

## 2.1 补拉 effect **不级联**，真实症状是「永久空列表 + 提示不可执行」（**推翻上一轮的两种猜测**）

上一轮原文：「若该 effect 在空布局下持续触发，症状会从『翻遍整库』恶化为『无条件翻库』；需 CDP 确认，成本高于收益，建议暂缓」。
本轮真机（选中 feed 15「综合报道」，18 篇全部在 60-120 天前；全局最新 500 条中属于它的 = 0 条）：

```
选中后：可见卡片 = 0，空态 =「暂无匹配内容」，哨兵 =「滚动加载更多」
15 秒内 list_articles 调用序列 = [offset 0] （仅 1 次），此后不再变化
容器度量：scrollHeight = clientHeight = 726 ⇒ scrollable = false
```

结论有三点，都要写进修复的设计约束：

1. **不存在级联**：`Timeline.tsx:197-200` 的 effect 依赖 `[filterKey, items.length, articlesExhausted, loadMoreArticles]`，
   补拉一页后 `items.length` 仍为 0、`articlesExhausted` 仍为 false ⇒ 依赖未变 ⇒ **不再触发**。
2. 真实症状是**永久空列表**（用户在这个范围/视图下永远看不到那 18 篇），不是「翻遍整库」。
3. 空态旁渲染的**「滚动加载更多」是不可执行的提示**：容器无可滚动内容 ⇒ `onScroll` 永不触发。
   （我手动置 `scrollTop` 并派发 `scroll` 能触发补拉，证明链路本身可用——这恰好说明缺的是「用户可执行的触发点」。）

**对照路径**：同一范围下切到「未读」视图 → 可见 1 张（载荷 `limit:100000, feed_id:15`）。
即：**筛选视图能显示，全部视图永久空白**——因为筛选视图走 `reloadFilteredEntries` 的「拉全库再本地筛」路径，
而全部视图走分页路径。这是 P0-1 之下最容易让用户困惑的一个不对称。

## 2.2 P2-15（feed 级自动 AI 在非文章布局永不生效）：**确认**，并订正其机理

真机（源 6 = 社交布局，`auto_summary=1 & auto_translate=1`；源 1/2/4 同理）：

| 布局 | 可见卡片 | 点击卡片后 `ai_*` 调用 | `.reader-col` |
| --- | --- | --- | --- |
| 文章（对照组） | 14（源 14 已开开关） | **1 次 `ai_summarize`** | display:flex, 838px |
| 社交 | 12 | **0** | display:none |
| 画廊 | 5 | **0** | display:none |
| 通知 | 1 | **0** | display:none |
| 播客 | 6 | **0** | display:none |

**机理订正（上一轮记为「Reader 列被 CSS 隐藏且 activeArticleId 恒 null」，前半句不成立）**：
`.reader-col` 在非文章布局**仍然挂载**（`display:none` 只是视觉隐藏，React effect 照常执行）。
真实门控有两级：
1. **社交/通知/播客卡片点击根本不调用 `selectArticle`** ⇒ `activeArticleId` 不被设置（真机实测：点击后
   `[data-ctx="article"].active-selected` 数量 = 0，阅读器仍显示「未选择文章」）；
2. 画廊卡片第一次点击走 `openImage`（标读/灯箱），**未读时也不选中**；只有已读时第二次点击才 `selectArticle`。

所以「自动摘要/翻译对 4/5 布局永不生效」成立，但原因是**卡片交互没有把文章交给阅读器**，
不是「Reader 被 CSS 隐藏」。这决定了修法：要么让卡片点击也 `selectArticle`（并让阅读器侧以非视觉方式工作），
要么把「自动 AI」的触发点从 Reader effect 移到卡片/懒水合层。

## 2.3 P2-7（跨布局陈旧快照回退）：**未复现**，已由 R3-1 取代（见 1.4）

## 2.4 P2-16（命令面板跨布局打开文章无可见反馈）：**确认**

真机（当前布局 = 播客；用 Ctrl+K 命令面板选中一篇**文章布局**的文章）：

```
点击前：布局=播客 卡片=6
点击后：布局=播客 卡片=6（列表完全没变）
        阅读器（display:none，用户看不见）内容变成「…小众软件 脑吧评测室 #3004…」
IPC：article_index ×1、list_articles ×1、set_read ×1
```

即：文章确实被加载并选中了，但**渲染容器是隐藏的**、布局不切换 ⇒ 用户看到「什么都没发生」。

## 2.5 命令面板选「订阅源」跨布局（审计S 的 S-1）：**确认**

真机（播客布局下，用面板选择「文章布局」的源「小众软件」）：

```
点击后：布局=播客，侧栏无选中行（该源的分类在播客布局下本就不渲染），卡片 = 0
空态 =「暂无匹配内容」
```

用户视角：选了一个源，界面变成空白、没有任何选中反馈。

---

# 三、本轮首次取得的其它真机结论

## 3.1 P1-3（分类级 AI 开关是死设置）：从「仅读码」升级为**实测**

| 条件 | 点击该源卡片后 `ai_*` 调用 |
| --- | --- |
| 源级 0 / 分类级 0（基线） | 0 |
| **源级 1** / 分类级 0（对照组） | **1（`ai_summarize`）** |
| 源级 0 / **分类级 1** | **0** |
| DB 回显 | `feed_flag=0, cat_flag=1`（开关确实落库了） |

⇒ 分类级开关「有存储、有 UI、有落库、零效果」在真机上成立。修法仍需 owner 决策（`FeedRow.auto_summary` 是
`NOT NULL DEFAULT 0`，无法表达「feed 级未设」）。

## 3.2 P1-4（浮层让路漏 `closeAsk`）：**对照组/实验组双证**

```
[对照组] 选中文章 3004，无浮层，按 S → set_starred{id:3004,starred:false}；库内 1 → 0  ✅键位注入有效
[实验组] 打开「关闭询问」弹窗，按 S      → set_starred{id:3004,starred:true}；库内 0 → 1  ❌穿透模态
```

⇒ 模态弹窗打开时，S/M/J/K 仍作用于弹窗**背后**的文章。`shortcutYield.ts` 的 `OVERLAY_SOURCES`（8 项）
确实不含 `closeAskVisible`，与上一轮一致。

## 3.3 P1-5（卡片收藏失败却报成功）：**实测复现**

```
注入 set_starred 拒绝 → 点击社交卡片「收藏」
toast = [「已收藏」]；库内该条 is_starred = 0；页面 unhandledRejection = 1
```

## 3.4 观测（低优先，非缺陷）：画廊未读文章需要**两次点击**才能进阅读器

第一次点击 = 标读（+灯箱），第二次点击 = 选中并在阅读器打开。已读文章一次点击即可。
代码注释称这是有意设计（「点开大图本身就是阅读完成」），但与其它四种布局的单击语义不一致，建议统一或明示。

## 3.5 侧栏角标不收敛（P2-6）：真机确认，并测到收敛触发点

```
社交布局点「全部已读」→ 后端未读 = 0
侧栏「未读」角标在随后 6 秒内恒为 31（10 次采样不变）
fire('feeds-updated') → 角标变 0
```

即：**只有 `reloadFromBackend` 才会刷新 `feedCounts`**；任何本地标读后的角标都可能与真实值长期矛盾。

---

# 四、独立复核（本轮对他人结论的确认 / 订正 / 降级）

## 4.1 【订正上一轮 P1-6 的方向性错误】修 P0-1 不会挂回归网（若按推荐方向修）

上一轮 P1-6 称：「回归网 8 处断言直接钉 snake_case ⇒ 修复 P0-1 会让这些断言失败」。
本轮逐行核对 `tools/frontend-regression.mjs`：

- 内存假后端 `queryRows`（:357-368）读的是 `a.feed_id` / `a.only_unread` / `a.newest_first`（**snake_case**）；
- 断言（如 :1894）钉的是 `s1Page2Call?.args.args.feed_id === 10` —— 即**前端 payload 的拼写**。

故两个修复方向后果不同：
- **推荐方向（删 `src-tauri/src/commands/articles.rs:16` 的 `rename_all = "camelCase"`）**：Rust 期望的键名
  变为 snake_case，与前端现有 payload 一致；回归网**全绿**（它本来就读 snake_case）；Rust 侧没有任何测试引用该属性。
- 反向（把前端改成发 camelCase）：会红（8 处断言 + 假后端）。

⇒ 上一轮「正确修复反而挂 CI」的说法**只对反向修法成立**，需在报告与任务卡里订正；
`ArticleListArgs` 全仓仅被 `list_articles`/`article_index` 使用（4 处引用），修面仍然极小。

**但 P1-6 的实质仍然成立且更严重**：回归网的假后端**比真实后端更正确**（它尊重 `newest_first`、`feed_id` 等），
所以 301 条断言在「后端其实全部忽略这些筛选」的真相下全绿——这正是上一轮记的「mock 与实现不同构」缺陷。
修 P0-1 之后，建议**同时**把该假后端改成「先按契约丢弃、再用默认值」的形态，否则它永远无法暴露这类缺陷。

## 4.2 确认：OpenAI 预设默认模型两侧不一致（审计F 的 F-2）

`src-tauri/src/ai.rs:29-35` 明确注释「OpenAI 预设默认模型须选仍支持 `max_tokens` 的 `gpt-4o-mini`
（而非 `gpt-4.1-mini`），否则默认配置即报错」，而前端 `shared.ts:50` 的 openai 预设 model = `gpt-4.1-mini`。
影响面需如实界定：**前端是 `ai_config` 的唯一写入方**，所以用户在 UI 里选「OpenAI 官方 API」时，
实际存进库的是 `gpt-4.1-mini`（后端自述会因 `max_tokens` 被拒的那个），与后端注释的「安全默认」相反。
建议：把 `shared.ts` 的预设模型改为与后端一致，或让后端在 `max_tokens` 被拒时自动回退。

## 4.3 降级：`published_at` 混合时间格式（审计F 的 F-1）——潜伏项，非现网缺陷

真实库只读抽样（4992 行）：`published_at` **100% 为 RFC3339 带 offset**（`substr(published_at,11,1)='T'` 命中 4992/4992），
空间形态命中 0；写入侧 `ingestion/parse.rs:97` 与 `sync/entries.rs:226` 都写 RFC3339。
`fetched_at` 确实是 SQLite 空间形态（`fetched_at` 在 `COALESCE(published_at, fetched_at)` 里只作兜底，
而真实库 `published_at IS NULL` 的行数为 0）。⇒ 记 P3 潜伏项（一旦出现 NULL 兜底或有外部写入空间格式，前端 `parseTs`
会把它当本地时间，产生 8 小时偏差）。

## 4.4 确认：删除分类不退订远端（审计C 的 C-1）

`commands/folders.rs:66-69` 的 `delete_folder` 只调 `record_folder_delete`（纯本地）；
对照 `:371-385` 的 `delete_feed` 会在锁外 best-effort 调 `sync::unsubscribe_remote`。
后果：远端仍保留该分类及其订阅，下次 pull 会把它们拉回（或长期不一致）。属「本地删除 ≠ 远端删除」的
语义缺口，与上一轮的「口径替换」发现同族，需 owner 决策。

## 4.5 确认：优化项的前提在真实 schema 上成立（审计O 的 O-9 / O-10 / O-12 / O-14）

在**真实库 schema** 上实跑 `EXPLAIN QUERY PLAN`：

```
列表查询(ORDER BY COALESCE(published_at, fetched_at) DESC LIMIT 500) → SCAN a + USE TEMP B-TREE FOR ORDER BY
feed_counts(GROUP BY feed_id)                                       → SCAN articles USING INDEX idx_articles_feed
only_today(date(a.published_at,'localtime') = date('now','localtime')) → SCAN a
建 date(published_at,'localtime') 表达式索引 → 被拒绝：non-deterministic use of date() in an index
建 COALESCE(published_at, fetched_at) 表达式索引 → 可建；计划变为 SCAN a USING COVERING INDEX ix_cov
articles_fts 的生产消费点 → 0（仅 migrations.rs 的建表/触发器与 articles.rs 的一行注释）
```

⇒ 「列表分页是全表扫描 + 临时 B 树排序」「today 判定无法直接索引」「FTS 表与 3 个触发器零消费」
三项前提都被独立证实。O-9/O-10/O-12 的**加速倍数**来自审计员的合成库基准（本轮未复跑），
但**机制方向正确**；O-14（删除或收敛 `articles_fts` 触发器）前提成立。

## 4.6 未复核（如实登记）

审计路线 S/O/C 的其余条目（S-2…S-14、O-1…O-25 的多数、C-2…C-12）本轮只做了抽样复核，
未逐条独立复现；其中 S-5/S-9/S-13/S-14 与 C 的多数条目仍是 `[仅读码]`。

---

# 五、修复优先级建议（本轮证据支撑）

| 序 | 事项 | 依据 | 备注 |
| --- | --- | --- | --- |
| 1 | **P0-1** 删 `articles.rs:16` 的 `rename_all` | 本轮 4.1 证明该方向不挂回归网；修面 4 处引用 | 同时把回归网假后端改成「按契约丢弃 + 默认值」形态 |
| 2 | **标读口径收口为「可见集合」**（P1-1 / P0-2 / P1-2 / R3-1 同一根因） | 本轮 1.1/1.2/1.3/1.4 真机四证 | 后端加 `ids` 参数或前端改用 `set_read_bulk(visibleIds)`；顺带修今天视图边界 |
| 3 | **空列表/单源老文章的永久空白**（P0-1 的直接后果） | 本轮 2.1：不可滚动 + 提示不可执行 | 建议：空列表哨兵改为可点击按钮，或空时自动续拉直到出现内容/到底 |
| 4 | P1-5 卡片的假成功 toast + 未 catch | 本轮 3.3 | 与 `toggleEntryFlag` 的 `void api.*` 同源 |
| 5 | P1-4 `closeAsk` 入清单 + 断言改逐项存在性 | 本轮 3.2 | 注意：只改等号不够，`overlayProbes` 也要加项 |
| 6 | P2-15 自动 AI 的触达面 | 本轮 2.2（机理已订正） | 先定产品口径：非文章布局要不要自动 AI |
| 7 | P1-3 分类级 AI 开关 | 本轮 3.1 | 需 owner 决策「feed 未设」的表达 |
| 8 | `delete_folder` 的远端语义、`config_sync` 的订阅/分类删除语义 | 本轮 4.4 + 上一轮模式 5 | 同一类「本地删除 ≠ 远端删除」 |
| 9 | 优化：表达式索引（O-9/O-10/O-12）、`articles_fts` 触发器（O-14）、IPC 去冗余（O-1/O-11） | 本轮 4.5 证实前提 | 改索引前先 grep 回归网的源码形态断言 |

---

# 六、本轮的方法学记录（供后续审计复用）

1. **真机 harness 是「effect 层」缺陷的唯一有效证据源**：上一轮 5 条未证实项，本轮 4 条在 1 小时内出结论，
   其中 1 条（补拉级联）被推翻、1 条（P2-7）被替换。
2. **假后端的忠实性要逐条写进代码注释**（本轮的 `mock_backend.py` 文件头逐项标注源码行）：
   一旦某处比真实实现「更正确」，就会产出生产不存在的现象。
3. **同一个 mock 里，命令语义与构建语义要分开**：命令路径必须复刻 rusqlite 的自动提交；
   fixture 构建是基建，可用显式事务包住（否则 3000 次 fsync 拖垮启动）。
4. **无效子测试要如实作废而不是将就**：本轮场景 4 的 A（搜索浮层）与 B（未先选中文章）因选择器/前置状态
   不当而不成立，已在场景 5 用修正后的版本重做；场景 4 的结论不作数。
5. **每次「成功却报错」类现象先怀疑自己的 mock**：本轮第一次运行就遇到一次（`sync_queue` 列名），
   若不复核就会多报一条假缺陷。
6. **审计基建自身要有防「上次运行残留」的设计**：本轮第二次遇到的故障是**上一个被放弃的后端进程仍占着库文件**，
   导致新 fixture 的显式事务在锁等待后被丢弃（实测耗 8 分 19 秒后以 `cannot commit - no transaction is active` 失败），
   表现成「场景莫名跑不起来」。修正：库文件名带进程号、构建用独立连接（默认隔离级合并为一次提交）、
   就绪检查要求端点返回 JSON 数组、收尾按进程树 `taskkill /T`。
   ——这类问题不会进入结论，但会**吃掉大量时间并诱使人怀疑被测对象**，值得写进流程。

---

# 七、证据索引

| 脚本 | 覆盖 | 关键输出 |
| --- | --- | --- |
| `tmp/audit-r3/harness/mock_backend.py` | 忠实假后端（真实 schema + 逐字 SQL 复刻 + 故障注入） | `[fixture] folders=5 feeds=16 articles=3005 unread=74` |
| `tmp/audit-r3/harness/drive.mjs` | Chrome/CDP 驱动 + Tauri IPC shim（含 Channel 流式事件） | — |
| `tmp/audit-r3/harness/s1_markallread.mjs` | 用户现象 / 空列表 / 今天视图 | `s1_stdout.log`, `s1_result.json` |
| `tmp/audit-r3/harness/s2_cascade.mjs` | 单源老文章、补拉是否级联、切「最早」 | `s2_stdout.log`, `s2_result.json` |
| `tmp/audit-r3/harness/s3_ai.mjs` | 源级/分类级 AI 开关、四种非文章布局 | `s3_stdout.log`, `s3_result.json` |
| `tmp/audit-r3/harness/s4_ui.mjs` | 假成功 toast、角标收敛、（含两处作废子测试） | `s4_stdout.log`, `s4_result.json` |
| `tmp/audit-r3/harness/s5_ui2.mjs` | 命令面板、浮层让路（含对照组）、跨布局陈旧、画廊、空列表可滚动性 | `s5_stdout.log`, `s5_result.json` |
| `tmp/audit-r3/route-S/REPORT.md` | 审计S：界面控件空壳扫描（195 个控件） | 14 条 P2/P3 |
| `tmp/audit-r3/route-O/REPORT.md` | 审计O：优化点（SQL/索引/IPC/内存） | O-1…O-25 + Top10 |
| `tmp/audit-r3/route-F/REPORT.md` | 审计F：serde 契约/镜像对照（54/55 匹配） | F-1…F-4 |
| `tmp/audit-r3/route-C/REPORT.md` | 审计C：声称已修核对（67 行总表） | C-1…C-12 |

> 真实性说明：`tmp/` 被 `.gitignore:23` 忽略，不入库。真实库只以 **`?mode=ro` 只读**方式被读过
> 一次（用于导出 schema 与抽样值格式），抽样前后原库 `fluxreader.db` 的 SHA-256 均为
> `1790cdcc044bcebe…`（一致，未写入）。导出结构后**已删除**含凭据的库副本，
> 假后端改为从纯结构文件 `tmp/audit-r3/harness/schema.sql` 建库（`make_schema.py` 可重建）。
