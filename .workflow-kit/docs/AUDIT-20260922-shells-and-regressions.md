# fluxreader 只读审计报告（2026-09-22）

- 基线：`git HEAD = d6234e5`（main，工作区干净）
- 授权与口径：用户指令——**不复跑既有门禁**（改在后续修复时再跑）、**改用后端模拟数据实测**、**证伪优先**
- 方法：8 路只读并行审计 + 独立对抗性验证；主控对每条关键结论亲自复跑或重编探针，含 3 条对审计员结论的**推翻/降级**与 1 条**自我修正**
- 既有质量底线（他人本轮实测，非本次复跑）：前端回归网 **339/339 全绿，EXIT=0**——本报告所有缺陷均在该全绿下存活
- 证据：所有探针可重跑，存于 `tmp/audit-scratch/`（`tmp/` 已被 `.gitignore:23` 忽略，不入库）

## 结论摘要

用户两个怀疑**都成立**，且比预期更严重：

1. **存在真实的、用户可感知的缺陷**，其中一条是自首个提交（`9828c8f`）就存在的**根本性契约缺陷**，它使多张任务卡（TASK-052/063）声称修复的问题**并未真正修复**。
2. **「声称已修复但实际仍存在」是本项目的系统性现象**，不是个别疏漏。已归纳出 4 种可复现的失败模式。
3. 但**不应一概否定**：审计确认 **30+ 项确实真正修复**，并推翻了他人 3 条夸大结论（含主控自己 1 条）。

| 等级 | 数量 | 性质 |
| --- | --- | --- |
| P0 | 2 | 静默数据损坏 / 根本性契约缺陷 |
| P1 | 7 | 用户可直接感知的错误行为或未落实功能 |
| P2 | 20 | 一致性、状态收敛、失败可见性、测试缺口 |
| P3 | 16 | 死代码、注释与实现不符、文档漂移 |

其中 **P1-1/P1-2/P2-9/P2-15/P2-16** 五条同源于「后端不知道 layout 存在」这一结构性事实。另有 **1 条治理层发现（模式 5 · 口径替换）** 未计入上方缺陷数——它不属代码缺陷，而属**需求与记录层**的问题。

---

# 一、P0

## P0-1 · `ArticleListArgs` serde 契约不匹配：7/9 字段被静默丢弃（根因级）

**位置**：`src-tauri/src/commands/articles.rs:15-27`；前端 `src/store/internals.ts:82-92`

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]          // ← 期望 camelCase
pub struct ArticleListArgs {
    pub feed_id: Option<i64>,               // ← 字段却是 snake_case
    pub folder_id: Option<i64>,
    pub only_unread: Option<bool>,
    pub only_starred: Option<bool>,
    pub only_today: Option<bool>,
    pub newest_first: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub with_content: Option<bool>,
}
```

前端发的正是 snake_case（`internals.ts:88-90`：`feed_id` / `folder_id` / `newest_first`）。Tauri 只把**命令形参名** `args` 转 lowerCamelCase，内层对象原样交给 serde ⇒ 带下划线的 7 个字段全部落空。

**主控独立证实**（`tmp/audit-scratch/parent-verify/probe_article_args.rs`，`rustc` 直编 + 链接既有 serde rlib，未用 cargo、未碰构建产物）：用前端真实 payload 反序列化，实测 **7/7 全部为 `None`**，仅无下划线的 `limit`/`offset` 幸存；对照组（移除 `rename_all`）则全部正确命中。

**连带后果**（由 `articles.rs:51-62` 的 `unwrap_or` 默认值决定）：后端恒定按
`feed_id=None, folder_id=None, only_*=false, newest_first=true, with_content=false`
查询 —— 即**全局、降序、无任何筛选**，只认 `limit`/`offset`。

**为什么至今没被发现**：默认「全部」视图下 `feed_id`/`folder_id` 本就为 `null`、`newest_first` 本就 `true` ⇒ 丢弃后语义等价。仅三类场景暴露：① 选中单个源/分类；② 排序=最早；③ 未读/收藏/今天视图。

**影响面（主控决定性实测，`probe_old_feed_unreachable.mjs`）**：全库 1200 条，源10=较新 600 条、源20=较老 600 条，全局 DESC 前 500 全属源10。选中「源20」后：
- 立即：`entries=500`，**该源可见 0 条**（列表空白，尽管该源有 600 条）——**正是 TASK-052 声称修复的症状**
- 连续翻页：翻页1 可见 400 条 → 翻页2 可见 600 条（`exhausted=true`），最终取全、无重复

即：单源视图靠**把整库翻完**才凑齐该源条目；`article_index`（`articles.rs:41-48`）用同一结构体，故其返回的是**全局序列**位置而非当前范围位置，破坏 `bootstrap.ts:248-252` 注释宣称的对齐契约（影响搜索锚定分页）。

**修复建议**：删掉 `articles.rs:16` 的 `#[serde(rename_all = "camelCase")]`（`ArticleListArgs` 仅被 `list_articles`/`article_index` 使用，全仓 4 处引用，改面极小）。**注意**：见 P1-6 的断言对抗问题。

## P0-2 · 列表为空时点「全部已读」，后端清空**全库**未读

**位置**：`src/components/Timeline.tsx:234`（按钮无空列表守卫）+ `src/store/slices/nav.ts:151-173` + `src-tauri/src/db/articles.rs:690-719`

三个条件同时成立：① 按钮无条件渲染可点；② 前端算出的可见集合**只用于本地**（`nav.ts:152` → `markEntriesRead`），发给后端的是 `scopeQueryArgs` 四参数（不含可见集合）；③ 范围 `all` 时后端 SQL 即全库。

**主控一手复现**（`tmp/audit-scratch/frontend-defects/probe7.mjs`，13/13 断言成立）：podcast 布局下列表为空 → 点按钮 → 实测 `dbUnread 3→0`；而本地 `markEntriesRead(∅)` 在 `internals.ts:166` 的 `if (!changed) return` 提前返回，**本地状态毫无变化** → 用户只看到 toast「已全部标为已读」，切回列表刷新后未读被静默吞掉。

**严重度依据**：标读经 `apply_mark_all_read`（`commands/articles.rs:221-226`）逐条入队并推远端，无撤销入口。

---

# 二、P1

## P1-1 · 「全部已读」无布局维度（用户报告的现象）

`nav.ts:151-173` 已**正确算出**可见集合，却只用于本地；后端 `db/articles.rs:697-715` 的 SQL 只有 `feed_id`/`folder_id`/`is_starred`/`published_at` 四条件。**结构性事实**：后端完全不知道 layout 存在（layout 只是 `folders`/`feeds` 的列，从不参与 article 查询过滤）。

主控两条腿实测（IPC 层 + SQL 层，见 `repro_mark_all_read_layout.py`）：文章布局下点按钮 → `mark_all_read` 实参键集合实测为 `feedId,folderId,starredOnly,sinceMs`，**无布局维度**；复刻同形 SQL 后社交布局源的条目**一并被标读并推远端**。

按钮文案（`Timeline.tsx:234` / `Overlays.tsx:131`）承诺的是「当前列表」。

**半修证据**：TASK-040 的 F8 只补了 view 两分支（`starredOnly`/`sinceMs`），TASK-067 只补了失败可见；**布局维度从未处理**。

## P1-2 · 「今天」视图口径不同构（CF-01 同类第二实例）

列表按**日期相等**（`articles.rs:199`，隐含上界=本地当日 24:00），标读按**只有下界**（`articles.rs:712-715`）。两条独立实测（主控 `probe_today_scope.py`、审计G `probe_today_scope_parity.py`）一致：`published_at` 晚于本地当日 24:00 的条目**列表看不见却被标读**。

## P1-3 · 分类级 AI 开关是死设置（有存储、有 UI、有落库，**零消费点**）

写入链路完全正常（`folders.auto_summary` → `api.ts:572-573` → `FeedsTab.tsx:135,147` → `feeds.ts:272-282` → `set_folder_ai_flags`）。但 `selectors.ts:189-195` 的实现逐字只读 feed：

```ts
autoSummary: binding?.feed.autoSummary ?? false,
autoTranslate: binding?.feed.autoTranslate ?? false,
```

主控把 `src` 全目录所有 `cat.autoSummary/autoTranslate` 与 `binding.cat.` 用法列尽，命中只有 `.layout`/`.id`/写开关时回读自身另一字段/开关自身回显——**没有任何渲染或生成路径读它**。

**语义对照**：`resolveFeedLayout`（`selectors.ts:24-26`）为**布局**建立了「feed 覆盖 → 分类兜底」两级模型；**AI 开关**在同一份数据结构上**没有**这个兜底。UI 与数据结构都摆出了两级形态，仅判定函数漏了第二级。

比 P1-15 那类「白名单不含→静默回落」更难发现：**开关能正常回显、落库也成功**。

> **需 owner 决策**：`FeedRow.auto_summary` 是 `NOT NULL DEFAULT 0`，无三态，无法表达「feed 级未设」。需定产品口径（引入继承语义 or 明确「feed 级优先、默认即未设」）。

## P1-4 · 浮层让路清单漏 `closeAsk`，且断言把漏项**钉死为契约**

`shortcutYield.ts:50-63` 的 `OVERLAY_SOURCES` 恰 8 项，`OverlayState`（`:37-47`）无 `closeAskVisible`。但真实浮层存在（`App.tsx:159-160` → `Overlays.tsx:738-744`），且是模态选择弹窗。

主控实测（`probe_overlay_gap.mjs`）：`anyOverlayOpen(仅关闭询问弹窗打开) = false`、`shouldYieldToOverlay(...,'s',false) = proceed`（不让路）⇒ S/M/J/K 作用到弹窗**背后**的文章。

**关键**：回归网 `frontend-regression.mjs:1209-1210` 断言是**等号**（`length === 8 && name 串 === 旧值`）⇒ **「把漏项补进清单」这个正确修复本身会让门禁失败**。清单从「消除盲区」退化为「把漏项固化成契约」。审计G 另补：只把等号改成 `>=` 不够，`overlayProbes`（`:1213-1222`）只有 8 项且不含 `closeAsk`，当前探针集根本查不出该缺项。

## P1-5 · 卡片标读/收藏失败却报**成功**（CF-04，定性较「静默」更重）

`reader.ts:390-400` 的 `toggleEntryFlag` 两个 `void api.*` 无 `.catch`；而调用点在同一次点击里**无条件**弹成功 toast：

```tsx
// Timeline.tsx:498-499
toggleEntryFlag(item.id, 'isStarred');
showToast(item.isStarred ? '已取消收藏' : '已收藏');
```

因 `flipEntryFlag` 已先翻转本地值，toast **必然报成功**。实测（`probe_cf04_false_success.mjs`）：注入 reject → `toast = ["已取消收藏"]`、`unhandledRejection = 1`。**落库失败却告诉用户「已收藏」。** 这是 SocialCard/NotifCard/GalleryCard/右键菜单**唯一**的标读收藏入口。

## P1-6 · 回归网把错误契约钉死，修 bug 反而挂 CI（ARCH2-2）

`frontend-regression.mjs:357-368` 的内存假后端**读 snake_case**（`a.feed_id`/`a.only_unread`/`a.newest_first`），并有 **8 处断言直接钉 snake_case**（L1894/1895/1916/1924/1983/2024/2055/2079）。mock 与前端自洽、**与 Rust 的自洽性无人校验** ⇒ P0-1 在 301 条断言全绿下存活。修复 P0-1 会让这些断言失败。

## P1-7 · TASK-046 属**文书替代**，引擎缺口仍开放（NF-07）

TASK-046 ledger 逐字称「回归自测 7/7 通过」。主控实跑：

```
python .workflow-kit/scripts/tests/test_allowed_paths.py
  → AttributeError: module 'workflow_runtime' has no attribute 'resolve_allowed_paths'
  → 0/7 通过，EXIT=1
```

`resolve_allowed_paths` 在引擎中 0 命中；修复曾被 rebind 升级覆盖。TASK-080 已如实订正并交付替代品 `tools/task-spec-guard.py`，但**该守卫未被任何入口调用**（引擎/`package.json`/CI 全零命中，仅出现在文档）⇒ 导致 **5 张卡取消的引擎缺口仍然开放**。

---

# 三、P2

| # | 缺陷 | 证据 |
| --- | --- | --- |
| P2-1 | **P3[1] 的修复只改了主路径，6 处同语义平行路径仍静默**（NF-01） | `entries.rs:235/239/265`、`subscriptions.rs:247/250/300` 仍是裸 `let _ =`。已修的是 `merge_remote_status`（`entries.rs:67/100/144` 有 warn）；`upsert_remote_entry` 与 `pull_feeds` 合并段语义完全相同。**处置表只数了「删了几行」，没查「同一语义还有没有别的调用点」** |
| P2-2 | **「清理 AI 缓存」静默失效**（NF-06） | `articles.rs:445` 定义 cutoff；`:451` articles 分支用 `datetime(published_at) < cutoff` ✓，`:468` ai 分支用裸 `published_at < cutoff` ✗。主控三次独立实测（自己 + 审计B + 安全专项）：同一批 now-7d-2h 文章，articles 分支命中、**ai 分支命中 0**。根因：RFC3339 第 11 字符 `'T'`(0x54) 恒大于 cutoff 的空格 `' '`(0x20)。3 条成对测试**全部传 `"articles"`**，ai 分支零覆盖；而 `commands/sync.rs:259-264` 明确允许 `scope="ai"`。用户点该按钮恒清 0 条却看似成功。**建议提 P2** |
| P2-3 | **`articlesExhausted` 跨范围/跨布局残留** | `nav.ts:43-61` 的 `selectLayout` 复位移位 `activeFeedFilter` 但**不复位** `articlesLimit`/`articlesCursor`/`articlesExhausted`。实测（`probe11.mjs`）：短源（40 条 < 500）置 `exhausted=true` → 切布局后该标志带过去，新范围被永久判定「没有更多了」，滚不出内容，须手动刷新 |
| P2-4 | **切换排序后不重拉，`oldest` 语义错误** | `nav.ts:145-149` 的 `toggleTimelineSort` 只改 `timelineSort`。主控实测（忠实后端）：切「最早」后首批可见 id 区间 `[201,1200]`，**全库最老的 id 1..200 不在其中**；真正按时间升序需翻遍全库 |
| P2-5 | **删除当前源/分类后 `activeFeedFilter` 悬空** | `feeds.ts:63-80` 的 `deleteCategory` **只在 mock 分支复位**；tauri 分支不复位。实测（`probe7.mjs` V2、`probe2.mjs` P8）：仍指向已删除 id，侧栏无选中行、列表永久空态且 `exhausted=true`，无自愈路径 |
| P2-6 | **侧栏角标长期不收敛** | `bootstrap.ts:210-246` 的 `reloadFilteredEntries` **从不写 `feedCounts`**；唯一刷新点是 `reloadFromBackend`。实测：库内未读=0 而侧栏「未读」与订阅树角标仍显示 100，列表空而数字 100 |
| P2-7 | **跨布局陈旧快照回退** | `internals.ts:119-125` 的 `syncCurrentViewCache` 只写「当前布局×视图×范围」一档，而 `entries` 是**全局共享数组**。实测（`probe2.mjs` P10、`probe3.mjs` R2）：social 布局标读后 article 档仍记未读，切回时已读状态**可见回退**。（审计G 与审计D 对「后台刷新是否纠正」结论**互相矛盾**，主控未定论，记为「瞬时或较久的可见回退，时长待定」） |
| P2-8 | **N10 三条平行入口未修** | ① `bootstrap.ts:314-316` 的 `anchorToArticle` 裸 `void api.setRead`（对照 `reader.ts:89-91` 已补 catch）；② `reader.ts:395/396` 卡片路径（见 P1-5）；③ `settings.ts:56` 的 `void api.setSetting('app_settings', ...)` 无 catch（NF-04）。实测：路径 A 有 toast、路径 B `toasts=[]` + `unhandledRejection=1` |
| P2-9 | **N11 失败路径标记粘滞 → N8 已修症状在卡片回归**（NF-08） | 见下方「修复互相打架」 |
| P2-10 | **`selectVisibleEntries` 缓存键缺时间维度** | 键（`selectors.ts:79`）不含时间。实测（`probe5.mjs` T3、`probe6.mjs` U2）：跨本地零点后引用与筛选标量都没变 ⇒ 缓存命中返回**昨天**的条目；强制重算则正确返回 0 条 ⇒ 缺陷在缓存键，不在「今天」判定 |
| P2-11 | **`reloadFromBackend` 的 scopeKey 在 `await` 之后读取** | `bootstrap.ts:118`（注释自称写「发起时」范围，与实现矛盾——同文件 `:86-89` 明确在发起时读了 `scopeArgs`）。实测（`probe3.mjs` R1、`probe9.mjs` E12）：查询口径 `feed_id=10`，却把 500 记到 `articlesCursor['all']`，并污染 `viewEntriesCache` 的 `all` 档 |
| P2-12 | **CI 覆盖缺口**（ARCH2-3） | `ci.yml:73-74` 显式跑 6 个套件，`ci.yml:66` 的 `cargo test` 已覆盖其余非 ignore 用例。真实缺口 9 个 ignore：7 个需真实 Miniflux（合理不跑）；**2 个只需 `python -m http.server 8765`**（`ingestion_e2e.rs:13`、`scheduler_e2e.rs:64`）——这是**唯一未被任何自动化覆盖的生产抓取路径**。建议 CI 增 1 步纳入门禁 |
| P2-13 | **「跨 slice 收口 internals 仅 7 处」已失效**（ARCH2-4） | `internals.ts:11-19` 与 `store.ts:26-29` 声称「键集两两不相交」。**声明层成立**（9 个 `Pick` 共 125 键、重复 0）；**写入层不成立**：`set()/setState()` 写他人键实测 **47 处、跨 7 个 slice 文件**（ai 14、nav 11、feeds 9、bootstrap 6、reader 6、settings 1、ui 1） |
| P2-14 | **`ArticleListArgs` 契约缺陷使 `with_content` 恒 false** | 即「社交/通知布局直接渲染，免逐篇水合」的优化从未生效；而 `layoutNeedsBody` 恒返回 false（`bootstrap.ts:42`）恰好掩盖了它 |
| P2-15 | **feed 级自动摘要/翻译对 4/5 的布局永不生效**（F-08） | 全仓唯一的按源**自动**触发点在 `Reader.tsx:98-108`（`selectFeedConfig` → `triggerReaderSummary`/`toggleReaderTranslation`），而 Reader 列在非 article 布局被 CSS 隐藏（`App.tsx:282` 的 `layout-2col` + `base.css:244-246` 的 `display:none`），且 `selectLayout` 显式清空 `activeArticleId`（`nav.ts:48`）⇒ 该 effect 的前置条件 `art` 恒为 falsy。卡片侧**只有 onClick**，无任何 effect（`summarizeEntry`/`translateEntry` 在 `Timeline.tsx` 的全部出现位置 `:521/:742/:754/:784` 均在事件处理器内）。实测（审计B probe9）：切到 notification 布局后 `activeArticleId=null`、卡片挂载+水合后 `ai_summarize`/`ai_translate` 调用数 = 0。**即：用户在非文章布局下，源级自动摘要/翻译设置永不生效**。⚠️ 证据边界：该结论由**静态链路**闭合（非 article 布局下 `.reader-col` 被隐藏 + `activeArticleId` 恒 null），article 布局那一侧因本框架不渲染 DOM 无法实测 |
| P2-16 | **命令面板「打开文章」在非 article 布局下完全无可见反馈**（F-09） | `anchorScopeNav.ts:36-42` 只归一 `feedFilter`/`viewFilter`/`timelineFilter`，**不含 layout**。实测（审计B probe5）：用户在 podcast 布局下搜索并打开一篇 article 布局文章 → 返回 `[{action:'toggleTimelineFilter'}]`（无 layout 步骤）→ `activeArticleId=101` 已设为选中，但列表仍是播客卡片、`.reader-col` 被 CSS 隐藏 ⇒ **界面完全无变化，用户看不到任何反馈**。根因同 P0-1：锚定查询口径与展示口径差一个 layout 维度 |
| P2-17 | **同步硬失败被吞成「成功」**（P1-12） | `sync.ts:79-80` 的 `api.syncPhase('feeds')` 挂了 `.catch(() => null)`，注释自称是为「未连接（notConnected）→ 走纯直连刷新」，但**它把任何 reject 都压成 null**（含 DB 错误、协议错误等硬失败）⇒ `feedsReport` 为 null ⇒ 整个 feeds 阶段被静默跳过、`syncFailures` 为空 ⇒ 最终 toast 是成功文案、`syncStatus='synced'`。实测（审计D `mineH`）：`sync_phase` 全程 reject → 最终 toast「已刷新，新增 0 条」、`syncStatus=synced`，**用户无从得知订阅层同步根本没跑**。注意 TASK-058 的修复只覆盖了「后端返回 Ok 但 `report.errors` 非空」这一半，**未覆盖「IPC 直接 reject」这一半**——又一处「只修一半」 |
| P2-18 | **水合不收敛：`get_articles` 不含某 id 时该 id 每次挂载都重新 IPC** | `reader.ts:216-217` 的 `.then((rows) => { if (!rows \|\| rows.length === 0) return; ...})` **提前返回**，跳过了 `:244-251` 的 `hydratedIds` 记账；且 `:220-242` 的合并只处理 `byId.get(a.id)` 命中的条目。故后端未返回的 id **永不进入 `hydratedIds`** ⇒ 每次挂载都重新走 IPC、恒定显示「加载正文…」。实测（审计D）：3 次挂载 → 3 次 IPC。影响面：条目在批量子集里被遗漏（如 id 已删除、或 `get_articles` 的 limit 语义截断）时表现为**永久假加载**（与 REQ-001 的症状同类） |
| P2-19 | **失败时两条互相矛盾的 toast 并存** | `nav.ts:166-172`：`api.markAllRead(...).catch(() => showToast('全部已读未能保存，重启后可能回退'))`，而**同一函数末尾无条件** `get().showToast('已全部标为已读')`。失败时用户同时看到「已全部标为已读」与「未能保存，可能回退」两条相反信息 |
| P2-20 | **toast 上限 4 条会丢掉最早的「带重试」项** | `ui.ts:101` 用 `.slice(-4)` 保留**最新** 4 条；而带重试按钮的 toast 停留更久（`:105` 的 `stay = action ? 4200 : 2200`）⇒ 批量失败时，用户正要点的那个重试项可能已被后续 toast 挤掉 |

---

# 四、P3

| # | 缺陷 | 说明 |
| --- | --- | --- |
| P3-1 | `feeds.origin_was_local` 无写入方 | **本条主控推翻了第 1 轮的 P0 定性**：列确实无写入方（`articles.rs:405` 的 UPDATE 恒 0 行），但 `subscriptions.rs:243-257` 的 URL 碰撞分支只 `set_feed_remote_id` + 回填标题，**从不把 `origin` 从 `local` 改为 `remote`**；全 `src-tauri/src` 中 `SET origin` 唯一生产写入是 `migrations.rs:224` 的一次性历史迁移。故 `origin_was_local=1` 状态**代码不可达**，「断开连接即删除本地订阅」**不成立**。真实问题降为：死列 + 注释与实现不符 + 该更强不变量无测试锁定 |
| P3-2 | `folders.remote_id` 无生产写入方 | `articles.rs:395` 以 `remote_id IS NOT NULL` 作为「服务端目录」第一信号，但 `create_folder`（`folders.rs:43-46`）不写该列；`sync/subscriptions.rs:177-178` 自述「不维护 remote_id」。恒假条件使该判定退化为单条件（残留服务端空目录） |
| P3-3 | `sync_now` 零生产调用 | `sync/mod.rs:25` 注释把它描述为同步引擎的**全量路径**；`src-tauri/src` 内仅 2 处命中（注释 + 定义），生产全量由前端两次独立 IPC 拼接 |
| P3-4 | `greader::tags::KEPT_UNREAD` / `client_login_url` 死项 | 各仅 1 处命中（定义行）。后者 doc 自称「供 tests/ 复用」但 tests/ 零调用，真实登录路径在 `:320` 内联拼同一 URL（漂移风险） |
| P3-5 | `github_oauth_client_id` 只写不读 | `github_auth.rs:16` 注释宣称「可覆盖为自建 App 的 Client ID」，`:272` 写入，**零读取**。且写入的是「实际使用值」而非「用户配置值」，语义自相矛盾 |
| P3-6 | `close-resolved` 事件无监听方 | `lib.rs:85` emit，全仓零 `listen`。对照同文件 `close-ask`/`sync-running`/`sync-idle`/`player-media` 均有真实消费者——本仓库惯例是「emit 必配 listen」，此条是唯一例外 |
| P3-7 | `numericId` / `scopeNumericId` 两份逐字相同实现 | `selectors.ts:153-156` 与 `internals.ts:74-77` 函数体逐字相同。前者用于布局/AI 开关落库、后者用于分页/锚定范围——一旦分叉，「写库范围」与「查询范围」会不一致（正是注释记载的历史 bug 类型） |
| P3-8 | TS 行类型镜像缺 2 字段 | `db/folders.rs:13` 的 `position`、`sync/mod.rs:40` 的 `removed_feeds` 在 `api.ts` 无对应。当前无消费方故无可观察后果；但 `row_fixture_e2e.rs` 只覆盖 2 个结构体，其余 7 个 `Serialize` 结构无防漂移网 |
| P3-9 | N5 排序子项仍未修（**有意留下**，非冒充） | `articles.rs:163/165` 与 `:227` 仍是字符串比较（注释自称「与 list_articles 完全同口径」）。TASK-064 的 `non_goals` 明确移出。实测混合 offset 下文本序 `[11,10,12]` vs 瞬时序 `[12,10,11]` 不一致 |
| P3-10 | 明确仍**未修**且未处置的三项 | P3-5（`push.rs:54` `unwrap_or_default`）、P3-6（`credentials.rs:16-23` 四读取 `.ok().flatten()?`）、P3-8（`settings.rs:31` `let _ = clear_dedup_tombstones`）——形态仍在，未评估用户可见后果 |
| P3-11 | **「显示: 全部/未读」按钮在 今天/收藏 视图下对列表零影响**（F-06） | 渲染条件 `activeViewFilter !== 'unread'`（`Timeline.tsx:224`）比生效条件 `activeViewFilter === 'all'`（`selectors.ts:100`）**宽**。实测（审计B probe3）：今天视图下点击 → `timelineFilter` 变了、按钮文案变了、列表**完全没变**，只有侧栏「全部」角标数字从 total 变成 unread ⇒ 用户以为按钮坏了 |
| P3-12 | **卡片 `tags` 徽章在生产恒为空**（F-10） | `api.ts:588` 硬编码 `tags: []`；SQLite `articles` 表**无 tags 列**（`migrations.rs` 全量 ALTER 中无）；`ArticleListItemRow` 也不含该字段。消费点 `Timeline.tsx:365-367`、`Reader.tsx:201-206` 在 Tauri 下**恒不渲染**（仅 `mockData.ts` 有非空 tags）。属「mock 才可见的 UI」 |
| P3-13 | **卡片「摘要」按钮在源已开自动摘要时首次点击是反向操作**（F-07） | `summaryOpen = summaryOverride ?? (feedConfig.autoSummary || !!summaryError)`（`Timeline.tsx:727`）。当 `autoSummary=true` 时初始 `summaryOpen=true` ⇒ `if (!summaryOpen) summarizeEntry(...)` 不触发，首次点击只把卡片**收起**。语义上按钮本就是 toggle，不算错，但无「生成中/未生成」区分提示 |
| P3-14 | **`settingsCollapsed` 在设置页只改本地不落库**（**属有意设计，不报为缺陷**） | `feeds.ts:323-328` 只 `set` 本地；对照 `toggleFolderCollapse`（侧栏折叠）**有**落库。两者语义不同（后者是持久偏好，前者是一次性编辑辅助），`api.ts:570` 中恒为 `false` |
| P3-15 | **「查看原始 HTML 源码」文案与行为不符但**有可见效果**（**不计入空壳**） | `Reader.tsx:251-254` 文案承诺显示源码；实际 `:305-317` 仍是 `dangerouslySetInnerHTML`，只是加 `.raw-render-mode` 类换排版（`base.css:1945-1957`）。属文案夸大而非点击无效。**列为 owner 判断项** |
| P3-16 | **`AboutTab` 检查更新的 CSP 疑点**（**未证实，列为排查线索**） | `AboutTab.tsx:48` 用 `fetch('https://api.github.com/repos/Dchean/fluxreader/releases/latest')`，而 `tauri.conf.json:28` 的 `connect-src` 仅 `'self' ipc: http://ipc.localhost`，**未列 `api.github.com`**。审计B 未在 WebView2 实跑，故不断言被拦；即便被拦也有 `.catch` → 明确失败提示（`:55-58`），**不属空壳** |
| P3-17 | **空 `className` 残留仍在**（P3-12 的**半修**订正） | `FeedsTab.tsx:170-171` 与 `:245-246`——属性后跟一行**纯空白**（`className="toggle-action-btn btn-danger-text"` 之后第 171 行只有空白字符）。属**纯形式项**，只影响源码可读性、不影响行为，**不值得单独立项**，建议下次顺手清理。<br>**方法学价值**：审计G 初判此项「已清理」，因其 grep 用了精确字面量 `className=""` / `className={""}`（全 0 命中）。这正是它自己在报告里反复指出的「源码形态/文本匹配可被绕过」——**把「空 className」想象成了唯一一种写法**。经子代理指出后独立复核订正。 |
| P3-18 | **P3-12「死代码三处」的处置是「接线」而非「删除」**（**属正确处置**） | 原报告称 `types.ts:81-84` 的旧 `ToastMessage` 是死类型；实际它现在是**活类型**：`store/types.ts:70`（定义）+ `:139`（`toasts: ToastMessage[]`）+ `store.ts:57`（对外导出）。`formatClock` 亦从 `format.ts` 迁至 `PlayerBar.tsx:30` 并被 6 处使用。**LAYOUT_LABELS 确认全库 0 命中、已清除**。故三处的真实处置为：一处删除、两处接线——均为正确处理，不是遗漏 |

---

# 五、设置项链路核对（24 个设置键 + 分类/源级 AI 开关）

审计 B 对本项目**全部设置项**做了「控件 → settings 字段 → 消费点」的逐条链路核对（主控复核了其中的断链项）：

**结论：24 个 `SettingsState` 键全部有消费点，无一断链。** 断链发生在**分类级 AI 开关**上——它不在 `SettingsState` 里而在 `CategoryGroup` 上，所以历史 P1-15 那种"白名单漏项"检查**覆盖不到它**（见 P1-3）。

| 设置项 | 字段 | 消费点 | 结论 |
| --- | --- | --- | --- |
| 自动刷新 / 刷新间隔 / 并发抓取数 | `autoRefresh` / `refreshInterval` / `fetchConcurrency` | `scheduler.rs:41,44,52`（跨 IPC 由 Rust 消费，含夹取范围） | ✅ 通 |
| 打开文章时标读 / 滚动到底标读 / 滚出列表标读 | `markReadOnOpen` / `markReadOnScrollBottom` / `markReadOnScrollOut` | `reader.ts:85`、`bootstrap.ts:314`、`Reader.tsx:113,126`、`Timeline.tsx:138` | ✅ 通 |
| 开机自启动 | `autoStart` | `AutoStartSwitch.tsx:22,40`（真实读写注册表） | ✅ 通 |
| 关闭时最小化到托盘 | `closeToTray` | `lib.rs:41` → `:230-234` | ✅ 通 |
| 新文章系统通知 | `notifyOnNewArticles` | `scheduler.rs:264,266` | ✅ 通 |
| 启动时打开 / 启动时隐藏已读 | `startupView` / `hideReadOnStartup` | `settings.ts:86-90,92`；白名单与 UI 同源（`types.ts:19-26`） | ✅ 通（历史死选项 `'article'` 已移除） |
| 主题模式 / 配色方案 | `themeMode` / `palette` | `App.tsx:45,46` → `:173-184`；`tokens.css:29-228` | ✅ 通 |
| 字体 / 字号 / 行高 / 最大宽度 / 预计阅读时间 | 5 项 | `Reader.tsx:193,295-297,307-309,207` | ✅ 通 |
| 默认打开方式 | `defaultOpenMode` | `reader.ts:154`（`fulltext` → `extractFulltext`） | ✅ 通 |
| 智能去重 | `smartDedup` | `scheduler.rs:49`、`commands/mod.rs:20`、`commands/settings.rs:28` | ✅ 通 |
| 同步模式 / 后台自动同步 | `syncMode` / `autoSync` | `scheduler.rs:68,114` / `:208-209` | ✅ 通 |
| 列表列宽（非设置页，拖拽） | `listWidth` | `App.tsx:47,316`（`--list-width`）；拖动期不落库见 N9 | ✅ 通 |
| **分类级 摘要** | `CategoryGroup.autoSummary` | **无**（`selectors.ts:189-195` 只读 feed；Rust `folders.auto_summary` 全仓无 SELECT 消费） | ❌ **断在消费点** → P1-3 |
| **分类级 翻译** | `CategoryGroup.autoTranslate` | **无**（同上） | ❌ **断在消费点** → P1-3 |
| feed 级 摘要 / 翻译 | `FeedItem.autoSummary` / `autoTranslate` | `Reader.tsx:103,104`（唯一自动触发点）；`Timeline.tsx:727,728`（仅决定初始展开/显隐） | ⚠️ **部分**：非 article 布局永不生效 → P2-15 |

**另一项独立核对**：`ArticleListArgs` 契约缺陷（P0-1）使 `with_content` 被丢弃，而 `layoutNeedsBody` 恒返回 `false`（`bootstrap.ts:42`）——两者恰好互为掩盖：**「社交/通知布局直接渲染，免逐篇水合」这个优化从未生效，也没有被发现**。

---

# 六、系统性模式（本报告最重要的产出）

「声称已修复但实际仍存在」不是偶发，而是 4 种可复现的失败模式：

**模式 1 · 修复只覆盖单分支** — 同一函数内两条分支对同一变量用不同口径。
- 实例：`cleanup_cache` 修了 articles 分支、漏了 ai 分支（P2-2）
- **判定表只数「删了几行」，没查「同一语义还有没有别的调用点」**（P2-1）——这是最值得引入流程的教训

**模式 2 · 修复只覆盖单入口** — 同一语义有多个入口，只改了一个。
- 实例：`anchorToArticle` vs `selectArticle`（P2-8）；卡片路径 vs Reader 路径（P1-5）；P3[1] 的 6 处平行路径（P2-1）

**模式 3 · 两条修复互相打架** — 共用状态标记，缺少失败复位。
- 实例（P2-9，本报告新发现）：N11 为消除流式未消毒 HTML 引入 `rawTranslatedIds` 标记使流式期间走纯文本插值；N8 要求卡片译文按 HTML 渲染。二者由**同一标记**裁决。失败路径不清理该标记（`ai.ts:222-224`/`:231-236` 清了 `isShowingTranslatedProse` 与 `translating`，**独漏 `rawTranslatedIds`**）⇒ 卡片懒水合把 DB 里**已消毒的 HTML** 写入 `translatedContent`，而标记仍为 true ⇒ 走纯文本插值 ⇒ **N8 修好的症状在卡片上回归**（用户看到字面 `<p>已消毒译文</p>`）。
- 主控**自我订正**：我先前记录称入口是 Reader。经审计G 提出、主控亲自核实：`Reader.tsx:289` 条件是 `isShowingTranslatedProse && rawStream`（双条件），失败时该标志被置回 false；且 `:303-317` 的 else 分支渲染的是 `baseHtml`（原文），**根本不渲染译文** ⇒ **Reader 路径不可达**。真实可达入口是卡片路径（`Timeline.tsx:487-491` 仅以 `rawTranslated` 单条件裁决）。

**模式 4 · 断言形态问题**（解释为何 339/339 全绿仍漏）
- 4a **参数级而非集合级**：回归网只断言 `starredOnly`/`feedId`/`folderId`/`sinceMs` 四个**参数**，从不断言「被标读集合 == 用户可见集合」⇒ P0-2、P1-1、P1-2 三个缺口全都能存活
- 4b **等号钉死漏项**：`OVERLAY_SOURCES.length === 8` ⇒ 正确修复反而挂 CI（P1-4）
- 4c **把两种情形混为一谈**：`(l2)` 断言（`:1560-1562`）在「已有半截未消毒内容」时正确，但缺陷出在「失败且无 delta」的分支（P2-9）
- 4d **mock 与实现不同构**：假后端读 snake_case 而 Rust 期望 camelCase ⇒ P0-1 隐身（P1-6）
- 4e **源码形态/文本断言**（审计G 指出，另见既有记录）：只是 grep token 在场，换个写法即绕过

**模式 5 · 口径替换（需求/记录层，本轮治理层面最重要的一条）**

前 4 种模式发生在代码层；这一条发生在**需求与记录层**：修复覆盖了「被改写的目标」，而不是「原始痛点」。

- **原声明**（两份文档一致，逐字）：
  - `FINDINGS-REQ-007.md:50`：「P2-12 配置同步无删除语义：**远端删除的订阅/分类本地永不删除**」
  - `AUDIT-20260919-v2.md:229`：「**仍存在** | config_sync.rs:141-261 只 upsert」
- **owner 裁决** `DEC-req104-p2-12-config-delete-20260920` 的 `statement` 逐字：「授权为配置同步实施删除语义：**远端白名单字段**在远端消失时本地同步删除」——**用词比原声明窄**
- **TASK-075** 实现的正是裁决文本（`merge_app_settings` 删除远端缺失的**白名单键** + 计数口径修正）——**并未跑偏**
- **当前代码实测**：`Select-String -Path src-tauri\src\config_sync.rs -Pattern 'DELETE FROM|delete_folder|delete_feed'` → **0 命中**。分类（`:151-155`）与订阅（`:190-193`）**均只有 upsert、无删除分支**

**结论**：原发现点名的对象（订阅/分类）**至今零实现**。TASK-075 忠实实现了裁决，但**最上游那份描述真正用户痛点的文档从未被任何下游订正或标注「有意收窄」**。后果：
- 用户仍与服务端不一致（服务端整理订阅后本地无限期保留已删项且无提示）
- 核对者若只读 TASK-075 的 objective 与 config_sync 的测试，会得出「P2-12 已修」的结论
- **下一轮审计会重犯**：要么再判未修，要么按 TASK-075 的证据误判为已修

> **需 owner 裁决**：① 补做订阅/分类删除语义（判据可与 P3-11 同款：远端列表缺失 + `origin='remote'` + 无 pending）；或 ② 确认收窄有意，并在上述两处文档**显式标注范围收窄**。

**给后续修复任务的固定步骤建议**：
1. 枚举该语义的**全部**入口/分支（含卡片、右键菜单、搜索、失败路径、无 delta 分支）
2. 断言改为**集合级**（可见 id 集 vs 实际写入 id 集），而非逐参数
3. 新增浮层/枚举时用「逐项存在性」而非「长度等号」
4. mock 必须**逐字复刻**被验证方的反序列化与默认值逻辑
5. 复核「已修」清单时按**同一语义的所有调用点**核对，而非按 diff 行数

---

# 六、确认真正修复的项（避免一概否定）

- **同步正确性**：C-1 状态集合拉取失败不再当空集合（`greader_pull.rs:122-140`、`fever_pull.rs:46-62`）；pull 分块失败不推进游标（`greader_pull.rs:142-155`）；N3 手动全刷读 `smartDedup`（`scheduler.rs:81`）；N4 重加清墓碑（`folders.rs:205`、`opml.rs:84`）
- **数据安全**：`purge_remote_data` 确实保留用户自建目录（`articles.rs:385-401` + 主控复跑 `purge_probe.py` 实测「用户自建目录是否幸存：是」）；`config_sync` 兜底目录改 `?` 上抛 + 单事务；`sync_local_feeds` 读队列失败改中止而非重复入队
- **前端**：N2 范围切换重拉；N7 锚定复位翻译/全文标志；N8 卡片译文 HTML 渲染；N9 列宽拖动期不落库、松手才持久化（`App.tsx:285-304`）；P0-2 生产错误不回退 mock（`bootstrap.ts:338-345`）；P1-10/P1-11 错误文案走 `extractError`；**P1-15「启动时打开=文章」死选项确认已移除**（`STARTUP_VIEW_OPTIONS` 与白名单同源）；`rawTranslatedIds` 在 Reader 主路径已正确复位
- **接线一致性**：`#[tauri::command]` 55 / `generate_handler!` 55 / 前端 invoke 55，**四个差集全空**；55 个命令的**形参名**与前端键名**逐一致**（含 `setFolderAiFlags→{id,summary,translate}`、`markAllRead→{feedId,folderId,starredOnly,sinceMs}`、`setReadBulk→{ids,read}`）；Rust 模块边界干净（`db/`/`sync/`/`ingestion/` 对 `crate::commands` 引用为 0）
- **仓库卫生**：`dist/`、`dist-test/`、`tmp/`、`.mimosa/`、`src-tauri/target/` 均被 `.gitignore` 正确忽略；`.mimosa/` 入库数 0；根目录 16 个 tracked 文件全部正常

**主控推翻/降级的他人结论 3 条**：
1. P0「断开连接即删除本地订阅+全部文章」→ 降为 **P3**（列无写入方属实，但状态代码不可达）
2. 「应用自行把整库翻完」→ **降级为未证实**（原文与自身数字矛盾：0 次请求；脚本从未驱动 React effect）
3. 第一轮 `cmd-contract.json` 的 `missing_fn: ["add_feed"]` 与 `ai_*` 的 `onChannel` → **假阳性**（前者是脚本正则跨不过 `#[allow(...)]`；后者 `on_channel: Channel<AiEvent>` 是真实 payload 参数）

**主控自我修正 1 条**：我先前报的「切换排序后重复 300 条卡片 + 最早 500 篇永久不可达」**现象不成立、已撤回**——我的假后端「尊重 `newest_first`」而真实后端因 P0-1 恒为 DESC，我的 mock 比真实后端「更正确」，制造了生产不存在的重复。订正后的真实缺陷见 P2-4。

**主控前提被订正 1 条**（由审计G 提出、主控实测确认）：我在给审计员的指令中把 `continuation_of` 当作「收口链接」（即认为取消卡会被后继卡引用）。**实测不成立**：全 60 张卡中 38 张含该字段，内容**全部指向线性前驱**（如 `TASK-036 → TASK-035`、`TASK-045 → TASK-044`）；**11 张取消卡（046/065/068/071/072/073/074/076/083/084/087）的被引用数全为 0**。且 `TASK-085`／`TASK-088` 的该字段**完全缺失**。

⇒ 收口链**只能靠终点卡 objective 的文本语义识别**，无法机械追溯。这是**流程建模缺陷**（不是交付缺陷）：`continuation_of` 当前语义是「前驱」，缺少真正的 `successor`／`supersedes` 字段来表达「本卡取代了哪张卡」。

---

# 七、未证实项（如实登记，不计入结论）

- **补拉 effect 的级联翻页**：依赖 React 渲染层，store 层探针无法判定（主控实测 store 层 0 次调用）。若 `Timeline.tsx:197-200` 的 effect 在空布局下持续触发，P0-1 的症状会从「翻遍整库」恶化为「无条件翻库」；需 CDP e2e 或 React 测试渲染确认，成本高于收益，建议暂缓
- **P2-7 后台刷新是否纠正跨布局陈旧快照**：审计D 判「会纠正」、审计G 判「仍在回退」，**两处矛盾**，主控未定论
- **Rust 侧「成对测试在修前失败」**：本轮未跑 `cargo test`（遵用户指示），审计G 只核对了断言内容与作用位置，未实测其在修前代码下失败
- **`media.rs` SMTC 路径的真实行为**：需 Windows 运行时
- **架构审计员标记的 5 个 `*_for_test` 测试后门**：判为有意设计而非空壳（名称自述用途、`#[doc(hidden)]`、确有测试调用）
