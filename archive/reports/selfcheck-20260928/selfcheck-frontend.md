# 前端逻辑审查报告（发布前全项目自检 · 前端子代理）

- 日期：2026-09-29
- 审查人：前端逻辑审查子代理（只读；未修改任何文件）
- 范围：`src/store/**`、`src/lib/**`、`src/components/**`、`src/App.tsx` 的逻辑接线；IPC 契约对表（对照 `src-tauri/src/commands/*.rs` 等）；`tools/frontend-regression.mjs` 覆盖缺口
- 方法：api.ts 全量 invoke 清单 → 逐命令对照 Rust 签名/serde 输出；9 个 store slice + internals/selectors 状态机通读；组件接线通读；grep 空壳特征；对照 git 历史中的 `docs/FEATURES.md`（现仓库已移除，README 指明在 5afdfec 等提交中）
- 说明：frontend 回归 419/419 全绿是既有基线；本报告只报门禁测不到的判断类问题。

---

## 一、发现清单

严重度定义：P0=发布阻塞；P1=应修（明确 bug 或用户可感知的假成功）；P2=宜修；P3=备忘。

**分布：P0 × 0，P1 × 2，P2 × 3，P3 × 10，合计 15 条。**

### P1（应修）

**[P1-1] 无任何分类时添加订阅必然失败——前端把空 catId 变成 `folderId=0` 而不是 null，后端「未分类」兜底契约不可达**
- 文件：`src/store/slices/feeds.ts:105`（`Number(catId.replace('cat-',''))`）、`src/components/Overlays.tsx:138/412`、`src/components/Sidebar.tsx:139`、`src/components/ContextMenu.tsx:242`
- 依据：catId 为空串时 `Number('') === 0`，`api.addFeed(url, title, 0, …)` 发送 `folder_id: 0`。Rust 侧 `add_feed`/`persist_new_feed`（`src-tauri/src/commands/folders.rs:114-190`）只在 `folder_id = None` 时走 `ensure_uncategorized_folder` 兜底（注释明言「`folder_id = None`（UI 未选分类，如全新安装无任何分类时）→ 自动落到未分类……首次使用添加源不再报错」）；`Some(0)` 直接用作 folder_id，而 `feeds.folder_id INTEGER REFERENCES folders(id)`（migrations.rs:25）且连接时 `PRAGMA foreign_keys=ON`（migrations.rs:299）→ 插入外键违约 → toast「添加失败: [db] FOREIGN KEY constraint failed」。
- 触达路径（全新安装、0 个分类）：侧栏「+」`openAddFeedModal(categories[0]?.id ?? '')`、右键菜单「新建订阅源」`openAddFeedModal('')`、命令面板「添加订阅源…」`openAddFeedModal('')`——三种入口的 AddFeedModal 在无分类时 catId 均为 `''`，下拉无可选项，提交即触发。后端专门为「首用添加」设计的兜底被前端参数转换整个短路。
- 建议：`addFeed` 里 `catId` 提不出数字时传 `null`（`folderId: Number.isFinite(n) ? n : null`），或在 AddFeedModalBody 无分类时禁用提交并引导先建分类；两处都做更稳。

**[P1-2] 「滚动出列表标已读」把程序性滚动当作用户滚动：J/K 回绕、搜索锚定跳转会整段误标已读（并同步远端）**
- 文件：`src/components/Timeline.tsx:182-192`（`handleScroll` 无条件 `scrollDrivenRef.current = true`）、`139-161`（scroll-away effect）、`173-179`（`scrollToIndex` 定位）；判定纯函数 `src/components/scrollAwayRead.ts`
- 依据：`rowVirtualizer.scrollToIndex()` 通过容器的 `element.scrollTo` 实现，会触发该容器的原生 `scroll` 事件 → React `onScroll`（= `handleScroll`）执行 → `scrollDrivenRef = true`。注释宣称「scrollToIndex（J/K 定位）与筛选切换的归零都不经过本 handler」不成立——它无法区分程序性滚动与用户滚动。随后 range.startIndex 大跳时，effect 判定 `start > last` 且 scrollDriven=true → `markEntriesReadBulk(items[last..start))`。
- 具体场景（设置「滚动出列表区域时标为已读」开启——GeneralTab 有入口，默认关；默认 `timelineFilter='unread'` 恰好满足另一门槛）：在列表顶部按 `K` 回绕到最后一项（`Timeline.tsx:274` `nextIdx = items.length - 1`）→ startIndex 0→N → 整个未读列表一次性标已读并推送到同步后端；搜索/命令面板 `anchorToArticle` 打开很老的文章（跳几百条）同理。
- 影响：用户未阅读的文章被批量置为已读，且该状态入 sync_queue 推远端（不可自动回退）。回归网只断言了 `scrollAwayRange` 纯函数，「事件是否真的来自用户滚动」这一前提无保护。
- 建议：改用 `wheel`/`touchmove`/指针拖动或 `keydown`（Space 翻页）等真实输入信号置 `scrollDrivenRef`；或对「单次跳变超过阈值（如 >3 屏）」的 startIndex 变化拒绝标读。

### P2（宜修）

**[P2-1] tauri 模式删除分类/订阅源后不清理 `activeFeedFilter`（与 mock 分支行为分叉）→ 时间流停在幽灵范围显示空白**
- 文件：`src/store/slices/feeds.ts:63-71`（deleteCategory）、`154-160`（deleteFeed）；对照 mock 分支 `76`、`168`（有 `activeFeedFilter: … ? 'all'` 复位）
- 依据：正在浏览分类 `cat-N` 时删除它：后端删除成功 → reload → 该分类从 feedIndex 消失 → `selectScopeEntries` 按 `cat-N` 过滤恒为空；Sidebar 无高亮行、标题栏也不显示名称，列表空白且无提示，用户须自行点别处恢复。删除当前选中的订阅源同理（`activeArticleId` 也未清理，Reader 空态但无导航修正）。回归网对这两个 action 零断言。
- 建议：tauri 分支在发起删除前按 mock 分支同口径复位 `activeFeedFilter`/`activeArticleId`。

**[P2-2] `reloadFilteredEntries` 竞态守卫只比较 view，不比较 scope/layout——慢的旧响应可持久覆盖新筛选列表**
- 文件：`src/store/slices/bootstrap.ts:225-261`
- 依据：守卫仅 `get().activeViewFilter !== view`。场景：收藏视图下 源A →（切源B 触发 reloadFilteredEntries，慢）→ 切布局 social（selectLayout 会把 scope 复位为 all 并再触发一次）。若源A 的旧响应**后到**，`activeViewFilter` 仍是 'starred' → 放行 → entries 被换成「源A × article 布局」的收藏列表，且游标写进过期键 `article|A`；若较新的那次请求已先返回，这次过期覆盖会**一直留存**到下一次导航。与 `reloadFromBackend` 的 reloadGeneration 代际守卫、`loadMoreArticles` 的四元组守卫相比，这里缺同一粒度的口径比较（scopeKey/sort 均未参与）。
- 建议：发起时快照 `scopePageKey(scope, layout)`，返回时不一致即丢弃（与 loadMoreArticles 同判据），或纳入 reloadGeneration。

**[P2-3] 社交/通知卡片级翻译失败后的恢复陷阱：半截译文残留 + 卡片无内联错误 + 「翻译」按钮不会触发重试**
- 文件：`src/store/slices/ai.ts:105-125`（流内错误保留半截 + translateErrors）、`src/components/Timeline.tsx:566-581`（SocialCard 翻译按钮）、`809-821`（NotifCard）
- 依据：卡片翻译失败且流内已产出半截译文时：卡片按纯文本渲染半截内容，但 SocialCard/NotifCard 都**不渲染 `translateErrors`**（与 Reader/摘要卡的错误行不同）；失败 toast（带重试）4.2s 后消失，此后卡片上的「翻译」按钮：`next && !item.translatedContent` 为假（半截仍在）→ 走「已显示正文翻译」分支，既不重发也不清除，半截+失败态永久并存，只能靠重新拉取列表（reload 会以 DB 空译文覆盖）解锁。`translateEntry` 本身对失败态放行重试（D1b 修复），但按钮入口到不了它。
- 建议：卡片补一行内联错误 + 重试（复用 NotifCard 摘要的错误行形态）；翻译按钮在 `translateErrors[id]` 存在时改走 `translateEntry(id)`。

### P3（备忘）

**[P3-1] 布局/范围切换的缓存未命中窗口内，哨兵/续拉可把新口径的一页追加到旧口径 entries 后（瞬态错排）**
- `src/store/slices/nav.ts:43-77,137-156`：selectLayout/selectFeed 先写游标镜像再异步 reload，此窗口内 `loadMoreArticles`（或 `refillDecision` 自动续拉）不被「reload 在途」拦截，取回的新口径首页会被 append 到旧布局列表尾（随后被 reload 整体替换，故为瞬态）。

**[P3-2] Sidebar `fetchFailed` 警示点 tooltip「最近抓取失败，点击重试」无对应 handler**
- `src/components/Sidebar.tsx:240-247`：span 无 onClick/role，点击只会命中父级的 selectFeed（导航而非重试）。假 affordance；重试按钮是旁边独立的刷新钮。

**[P3-3] `triggerManualSync` 把 `syncPhase('feeds')` 的**全部**异常吞成「未连接」**
- `src/store/slices/sync.ts:78-80`：`.catch(() => null)` 注释只说 notConnected，但网络故障/超时等同样变 null → 静默跳过订阅层同步，最终 toast 只报直连刷新结果，已连接用户的同步失败不可见（对比：report.errors 路径已按 TASK-058 做到可见）。

**[P3-4] `toggleAllFolders` 逐分类发 N 次 `set_folder_collapsed` IPC，失败逐个 toast**
- `src/store/slices/feeds.ts:311-321`：几十个分类时一次点击几十次往返；单个失败各弹一条「折叠状态未能保存」（toast 上限 4 条可兜住，但语义上应整批一次）。

**[P3-5] SettingsSidebarFooter 版本号失败兜底为硬编码 '0.8.0'**
- `src/components/settings/SettingsSidebarFooter.tsx:11`：AboutTab 已按 P2-7 移除假版本号（「假版本号会让检查更新拿错误基准」），页脚仍会显示假版本。应显示 '…' 或重试。

**[P3-6] ShortcutsTab 快捷键表缺 Space（播放/暂停）**
- `src/components/settings/ShortcutsTab.tsx:4-11`：App.tsx 实现了 Space（播放器激活时），表内未列。

**[P3-7] `viewEntriesCache` 仅在切排序时整体清空，无条目数/内存上限**
- `src/store/internals.ts:45`、`nav.ts:172`：筛选视图缓存的是 limit=100000 的全集快照，按「布局×视图×范围」组合持续累积；超大库长会话内存增长无界（ready 缓存有 300 上限的是 coverImage，两者不对称）。

**[P3-8] SyncTab `doDisconnect` 把「断开成功但 reload 失败」误报为「断开失败」**
- `src/components/settings/SyncTab.tsx:144-158`：`reloadFromBackend` 失败重抛被同一 catch 捕获，toast 文案与实际（已断开）不符。

**[P3-9] Rust `SyncReport.removed_feeds`（sync/mod.rs:40）前端 `SyncReport` 类型未声明、未消费**
- `src/lib/api.ts:111-118`：后端新增的对账删除计数被前端忽略（无害，纯信息缺失，备忘）。

**[P3-10] mock 分支 `addFeed`（catId 无匹配）静默丢弃订阅源**
- `src/store/slices/feeds.ts:129-150`：`categories.map` 找不到目标分类时 feed 无处挂载，仍弹「已添加订阅源」——仅浏览器演示模式可达，备忘。

---

## 二、空壳盘点（功能 → 实现状态 → 证据）

对照 git 历史 `docs/FEATURES.md`（5afdfec）核心 REQ + OPT-001~010 逐条核对前端接线：

| 功能 | 状态 | 证据（前端接线点） |
| --- | --- | --- |
| 五内容布局 article/social/image/podcast/notification | ✅ 完整 | Timeline.tsx 四种卡片 + 画廊；selectLayout 带布局重拉（TASK-094） |
| 阅读：选中/标读/收藏（乐观+回滚+计数联动） | ✅ 完整 | reader.ts + internals.optimisticEntryFlagToggle（回滚带「当前值仍等于乐观值」守卫） |
| AI 摘要（Reader+通知卡、流式、缓存、重试） | ✅ 完整 | ai.ts summarizeEntry；半截产物/失败态重试语义有回归断言（D1a） |
| AI 翻译（Reader 流式 + 卡片级 + 消毒回读） | ✅ 完整（卡片失败态恢复见 P2-3） | ai.ts translateEntry/toggleReaderTranslation；rawTranslatedIds 纯文本渲染契约 |
| 全文提取 / 自动全文 / 降级提示 | ✅ 完整 | reader.ts extractCurrentArticle / ensureArticleContent（degraded 结构化判据） |
| 搜索（FTS5 + 浏览器回退 + 锚定打开） | ✅ 完整 | Overlays.SearchModal + anchorToArticle + anchorScopeNav（先导航后锚定） |
| 全部已读（范围/视图/布局口径） | ✅ 完整 | nav.markCurrentViewAllRead（starredOnly/sinceMs/layout 与后端对齐，回归 S-5/(f)） |
| 播客播放/迷你条/全屏播放器/倍速/seek/续播/SMTC | ✅ 完整 | player.ts + PlayerBar.tsx（last_playback 续播含 P2-2 元数据就绪修复；media_update_full/media_stop 接线） |
| 灯箱 | ✅ 完整 | Overlays.Lightbox + GalleryCard/SocialCard/Reader 正文图（data: URL 直显） |
| 封面代理 + 失效上报 | ✅ 完整 | coverImage.ts/imageProxy.ts/CoverImage（五图片位共用，幂等上报，回归 cov-* 30+ 条） |
| 同步状态展示/手动同步/首连推送确认 | ✅ 完整 | sync.ts + SyncTab（firstConnect/unboundLocalFeeds 确认框） |
| 自动刷新/并发/去重/通知/托盘/自启设置 | ✅ 完整（后端消费） | scheduler.rs 读 autoRefresh/refreshInterval/fetchConcurrency/smartDedup/notifyOnNewArticles；lib.rs 读 closeToTray；AutoStartSwitch 真读写注册表 |
| OPML 导入/导出 | ✅ 完整 | FeedsTab（file.text → opml_import；Blob 下载导出） |
| 配置同步 Gist/WebDAV + GitHub 设备流 | ✅ 完整 | ConfigSyncSection + sync.ts（设备流常驻轮询；webdavConflict 结构化 code 判定） |
| 缓存清理（articles/ai + 确认框） | ✅ 完整 | CacheCleanupSection → cache_cleanup |
| 检查更新 | ✅ 完整 | AboutTab（P2-7 版本可比性守卫） |
| 快捷键 S/M/J/K/Space/Ctrl+K/Ctrl+,/Esc + 浮层让路 | ✅ 完整 | App.tsx + shortcutYield（表缺 Space 行，见 P3-6） |
| 右键菜单（article/feed/reader/全局） | ✅ 完整 | ContextMenu.tsx（全部菜单项有 onSelect 实实现） |
| **空壳项** | ⚠️ 1 处 | Sidebar fetchFailed「点击重试」tooltip 无 handler（P3-2）；ShortcutsTab 表缺 Space（P3-6，文档性） |

未发现「成功 toast 但实际未调后端」类假成功：所有写操作（标读/收藏/全部已读/布局/AI 开关/折叠）均为乐观更新 + 失败 toast（多数带一键重试），成功提示统一在落库成功后出现（P1-5 口径）。mock 模式的假成功提示（演示模式弹「已刷新」等）为有意的演示行为，不计。

---

## 三、IPC 契约对表（api.ts ↔ src-tauri）

`api.ts` 全部 55 个 invoke 命令逐一对照 `src-tauri/src/lib.rs` 的 `generate_handler!` 注册表与各命令 Rust 签名：

| 组 | 命令 | 参数名（Tauri camelCase 转换后） | 结果 |
| --- | --- | --- | --- |
| 分类 | list_folders / create_folder(name,layout) / delete_folder(id) / rename_folder(id,name) / update_folder_layout(id,layout) / set_folder_collapsed(id,collapsed) / set_folder_ai_flags(id,summary,translate) | ✅ | 一致 |
| 订阅源 | list_feeds / add_feed(feedUrl,title,folderId,layout,autoSummary,autoTranslate,syncToBackend) / delete_feed(id) / update_feed(id,title?,folderId?,layout?,autoSummary?,autoTranslate?，缺省键=None 语义正确) / update_feed_layout / set_feed_ai_flags(id,summary,translate) | ✅ | 一致，**唯语义问题见 P1-1（folderId=0 vs None）** |
| 条目 | list_articles({args}) / article_index({args,articleId}) / get_article(id) / get_articles(ids) / search_articles(query,limit) / set_read / set_read_bulk(ids,read) / set_starred / mark_all_read(feedId,folderId,starredOnly,sinceMs,layout) / feed_counts / report_broken_cover(articleId,url) / refresh_feed(feedId) / refresh_all_feeds | ✅ | 一致；`ArticleListArgs` snake_case 字段与 Rust `#[derive(Deserialize)]` 字段逐字对齐 |
| 设置 | get_setting / set_setting(key,value) / extract_fulltext(articleId)→{html,degraded,reason} / fetch_image(url,pageUrl) | ✅ | 一致（ExtractOutcome 字段名逐字匹配） |
| AI | save/get_ai_config / ai_list_models(baseUrl,apiKey) / ai_summarize/ai_translate(articleId,onChannel→Channel) | ✅ | 一致；AiEvent `tag="type", content="data"` serde 输出 {type:'delta'|'done'}，前端三态分支含防御性 'error' 兜底（后端已无该变体，注释如实） |
| 播放 | media_update_full(title,show,durationSec,positionSec,playing) / media_stop | ✅ | 一致 |
| 配置同步 | config_sync_status/save_credentials(credentials)/upload/download/apply(payload)→{imported,updated,skipped} | ✅ | 一致（TASK-074 语义分离已对齐） |
| GitHub | github_login_start(force?；client_id 前端不传=合法 None)/poll/status/disconnect | ✅ | 一致（DeviceLoginStart/GitHubAccount 字段匹配） |
| OPML | opml_import(content)→{imported,skipped} / opml_export | ✅ | 一致 |
| 后端同步 | sync_test/sync_save(protocol,endpoint,username,password)→JSON{message,firstConnect,unboundLocalFeeds}（含旧纯文本兼容解析）/ sync_phase(which,full)/sync_local_feeds/sync_disconnect/cache_cleanup(days,scope)/sync_status→{connected,endpoint,account,last_sync,protocol} | ✅ | 一致；SyncStatusInfo snake_case 与 Rust 结构（无 rename_all）匹配 |
| 关闭 | resolve_close(action,remember) | ✅ | 一致（close-ask / close-ask-ack 事件对接 lib.rs 10s 兜底） |

- 行类型对表：`FolderRow/FeedRow/ArticleListItemRow/ArticleDetailRow/FeedCountsRow` 与 Rust serde 输出（无 rename_all → snake_case 保留）逐字段一致；`FolderRow.position`、`FeedRow.site_url/fetch_error/last_fetched_at`、`SyncReport.removed_feeds` 为前端未消费的多余/缺省字段（P3-9 备忘），无「前端读了不存在字段」。
- 后端返回被忽略项：`feedCounts` 为 null 时前端静默走保守计数（L2 有意设计，有回归断言）；`syncStatus` 失败静默（提示性刷新，合理）。

---

## 四、tools/frontend-regression.mjs 覆盖缺口

现有 419 断言对 store 状态机/竞态/文案/封面代理/哨兵判定覆盖极深（含大量「修前可复现」变异取证）。以下真实路径**无断言保护**：

1. **addFeed 全链路**（0 断言）：P1-1 的 folderId=0 场景、`syncToBackend` 分支、重复 URL 错误路径均无回归保护。
2. **deleteCategory/deleteFeed store action**（0 断言）：P2-1 的 activeFeedFilter 复位分叉不可被现有网捕获。
3. **reloadFilteredEntries 的竞态守卫**：只有「失败可见/unhandled rejection」断言，无「旧 scope 响应不得覆盖新 scope」断言（P2-2 正是缺口）。
4. **PlayerBar 组件效果**：last_playback 落库节流/续播 seek（含 P2-2 修复）/SMTC 节流推送/mediaStop 触发——只有 store action（(j) 系列）被断言，组件 effect 全靠人眼。
5. **App.tsx 全局 keydown 接线**：Ctrl+K/Ctrl+,/Esc 级联/Space 分支无行为断言（仅 anyOverlayOpen/shouldYieldToOverlay 纯函数 + 源码形态断言）。
6. **SearchModal 组件**：防抖、FTS 失败回退内存过滤、代际守卫（alive）——无断言。
7. **scroll-away 的「事件来自用户滚动」前提**：scrollAwayRange 纯函数有强断言，但 scrollDrivenRef 的赋值来源（handleScroll 对程序性滚动也置真）无保护——P1-2 即从该缝隙漏出。
8. **设置持久化 payload**：updateSettings → set_setting('app_settings', JSON) 的键集与校验后内容无端到端断言（D5 只断言校验表行为）。

建议优先补 1/2/3/7 四项（与本次 P1/P2 一一对应）。

---

## 五、结论

- **P0：0 条**（无数据丢失/崩溃/核心功能假的发布阻塞项）。
- **P1：2 条**——①无分类时添加订阅必败（folderId=0 vs null 契约断裂，后端兜底被短路）；②「滚动出列表标已读」把程序性滚动当用户滚动，J/K 回绕/搜索锚定可整段误标已读并同步远端（设置开启时）。
- **P2：3 条**——删除分类/源不清理活动范围；筛选视图 reload 缺口径竞态守卫；卡片级翻译失败态恢复陷阱。
- 其余 10 条 P3 备忘。空壳仅 1 处假 affordance（fetchFailed 点击重试），FEATURES.md（git 历史）声明的核心与 OPT-001~010 前端能力均有真实实现与后端命令对接。
