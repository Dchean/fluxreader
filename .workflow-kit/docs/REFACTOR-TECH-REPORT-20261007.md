# 核心一致性重构——技术评估文档

- 供第三方审计/评估快速定位：每项改动给出**设计思路、备选取舍、代码锚点（函数/符号级）、测试锚点**。
- 行号以 dev@4bb771c 为准（个别行号可能随后续提交漂移，以符号检索为准）。
- 配套文件：路线决定 `DEC-refactor-roadmap-20261005.md`、审计输入 `AUDIT-20261005-core-consistency.md`、四阶段测量 `PHASE4-MEASUREMENT-20261007.md`、全流程工作报告见会话交付。
- 最终状态：dev@4bb771c / main@99d692b（tag v0.17.0）；前端回归 527→655；Rust 新增测试 17 条全部 CI 通过。

---

## 0. 技术思路总纲：审计四块 → 落地设计

审计判断问题根因是「业务规则分散在多个状态、组件和协议分支」。四个重构块按以下思路落地（Zustand 单库内收敛，不引入第二套缓存）：

| 审计块 | 落地设计 | 任务 |
| --- | --- | --- |
| ①文章实体与视图分离 | **快照合并 + 水合终态机**：列表查询只存视图行，正文等重字段按 id 从旧条目继承（mergeSnapshotEntries 单点）；正文加载建立显式状态机（未请求→在途→成功/空正文/不存在/失败），每态可从 store 判定（entryNeedsHydration） | 106 |
| ②查询范围与批量操作统一模型 | **QueryScope 单点派生**：三把口径键与查询参数全部从单一入口产生；**真分页**替代近似全集；批量操作结果**以后端对账为准**（消费返回条数+重取计数） | 109/110/107 |
| ③统一事务与同步语义 | Rust 侧「状态写入+入队」单事务；**政策单点**（协议差异从实现分支提升为常量）；乐观写入配**版本化回滚守卫**（单一版本源 entryMutationVersion） | 108/112/107 |
| ④同一内容与各源条目分离 | 政策显式化+兼容矩阵文档（本阶段交付）；完整建模分离留 owner 裁决（副本传播策略） | 112 |

横切原则（跨任务反复出现，评估时可用作一致性检查）：

1. **乐观写入必配回滚守卫**，守卫用单调版本而非值比较（值比较无法区分「用户已接管」与「未被触碰」）；
2. **后端真值与 UI 回放严格区分**（mergeSnapshotEntries 的 fromBackend 参数）；
3. **信号与载荷分离**：store 只传 nonce 计数器，载荷由组件层记账（store 不反向依赖 components 层）；
4. **缓存必有预算**（LRU 8 键 / 单键 1000 实体 / 锚存档 8）；
5. **行为零变化的证明方式** = 修前修后语句逐列对照 + 既有测试全过 + 新增政策锁定测试。

---

## 1. TASK-106 文章快照与正文水合生命周期（REQ-001）

**根因链**（审计探针复现）：reloadFromBackend 快照替换 entries（新行无正文）+ 清空 hydratedIds → 虚拟列表按 id 保持组件身份 → useLazyHydrate effect 依赖仅 [id] 不重触发 → 卡片停留「加载正文…」且无请求在途。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/internals.ts` → `mergeSnapshotEntries()` | 按 id 继承 content/rawContent/translatedContent/aiSummary/fulltextExtracted/hydrated/url（url 为详情字段，不继承会破坏「查看原文」）；新行自带正文以新行为准；无痕迹条目原样返回保持引用稳定；hydratedIds/hydrationErrors 按存活 id 裁剪 |
| 六个快照替换点 | bootstrap.ts `reloadFromBackend`/`reloadFilteredEntries`/`anchorToArticle` + nav.ts `selectLayout`/`selectView`/`selectFeed`（缓存恢复同属快照替换，一并收口） |
| `src/store/selectors.ts` → `entryNeedsHydration(s, id)` | 纯函数：条目在 ∧ 无正文 ∧ 无终态 ∧ 无失败态——水合触发判定单点 |
| `src/components/Timeline.tsx` → `useLazyHydrate` | 按需水合布尔订阅，effect 依赖 `[id, needsHydration]`，条件重新成立即重新入队（选此而非「reload 完成后统一重拉」：避免整页背正文洪峰，覆盖缓存恢复路径） |
| `src/store/slices/reader.ts` → `hydrateArticleContent` | 模块级 `hydrationInFlight` Set 在途去重；空 rows/缺行 → hydrationErrors「文章不存在或已被删除」终态；失败仅对仍存在且未水合 id 落错；落地时按现态逐 id 复核（等价 per-id 代际，无整批丢弃副作用） |

**测试锚点**：tools/frontend-regression.mjs `t103-*` 16 条；harness 假后端 `getArticlesPlan/pendingGetArticles`（defer/reject 注入）。既有断言 (a)「新快照清空 hydratedIds」改写——原断言锁的正是缺陷手法，改写理由注释在 `:534-546` 一带。

## 2. TASK-107 全部已读计数一致性与版本化回滚（REQ-003）

**根因**：markCurrentViewAllRead 乐观只按已加载条目扣计数，后端处理整范围（审计 600/1 探针：修后残留 599）；api.markAllRead 已返回实际条数被忽略。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/slices/nav.ts` → `markCurrentViewAllRead` | 乐观先行（保存 prevReadById/openedReadIds 快照）→ 成功消费 `markAllRead` 返回 affected 并 `api.feedCounts()` 重取整体替换（affected===0 免对账）→ 失败回滚+toast 带重试；成功 toast 移到落库确认后 |
| `src/store/internals.ts` → `entryMutationVersion` Map + `getEntryVersion/bumpEntryVersion` | 单调版本源；bump 点三处：`flipEntryFlag`/`markEntriesRead`/`mergeSnapshotEntries`（快照替换使在途乐观声明失效） |
| nav.ts 回滚守卫 | 值守卫（快速短路）+ 版本守卫（仅当条目版本==乐观写入时记录的版本才恢复）——修复「用户双 toggle 停在同值被误回滚」 |
| nav.ts → `reconcileCounts()` | 对账重取失败：诊断 toast（16 字）+ 3s 一次静默重试，仍失败依赖 reload 自愈 |

**备选取舍记录**：单条 toggle 路径（optimisticEntryFlagToggle）的同值覆盖窗口**未改**（审查裁定与其既有契约同族、有 reload 自愈；版本机制已就位供后续接入）；R2 发现并修正 harness mock 行级 mutation 的数字/布尔口径不忠实（对齐 Rust Serialize wire 格式）。

**测试锚点**：`t104-*` 17 条，判别核心：`t104-markall-count-authoritative`（600/1 场景）、`t104-rollback-guard-versioned`、`t104-snapshot-voids-rollback-claim`（外部 DB 写入+快照替换+迟到回滚，唯一守卫是 merge bump）。

## 3. TASK-108 Rust 状态写入事务化（REQ-002/003）

**根因**：record_read_state 先 set_read 后 enqueue_sync 无外层事务——第二步失败留下「本地已改、无待推记录」，离线永不补推。

**代码改动**（与既有 `mark_all_read_with_enqueue` 事务口径逐字对齐）：

| 锚点 | 内容 |
| --- | --- |
| `src-tauri/src/db/articles.rs` | 私有内核 `set_read_enqueue_in_tx`/`set_starred_enqueue_in_tx`（语句顺序与修前逐列一致）；单条入口 `set_read_with_enqueue`/`set_starred_with_enqueue`（unchecked_transaction+显式 commit，出错 drop 回滚）；`set_read_bulk_with_enqueue` 整批单事务全有全无 |
| `src-tauri/src/db.rs` | pub use 三入口（调用路径不变） |
| `src-tauri/src/commands/articles.rs` | `record_read_state/record_star_state/apply_read_bulk` 只调度；契约注释载明「整批单事务、无部分成功」 |

**测试锚点**（commands/articles.rs bulk_read_tests）：sync_queue 上 `BEFORE INSERT ... RAISE(ABORT)` 触发器注入（失败落在真实 enqueue_sync INSERT）；bulk 用 `WHEN NEW.article_id=<literal>` 只令末篇失败（修前逐条提交必部分成功→必红）。调用点盘点结论：生产路径仅三命令经新入口，opml/folders 的 enqueue_sync 调用不受影响。

## 4. TASK-109 QueryScope 口径收口 + merge bump 来源精确化

**根因**：三把键维度不对称（scopeQueryArgs 范围×排序×layout / scopePageKey 范围×layout / viewCacheKey layout×view×scope）且三处口径不一致靠注释默会；merge bump 对「后端真值」与「UI 回放」不区分。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/internals.ts` → `QueryScope` | 单点派生：`args`/`markScope`（scopeFilterArgs，范围维度）/`pageKey`/`viewKey`/`paginationStale`（范围×布局×游标×排序×视图五元组全锁）/`filteredSnapshotStale`（具名化「筛选视图不锁排序」的旧默会） |
| bootstrap.ts → `reloadFromBackend` | 发起时快照 `scopeAtStart/layoutAtStart/sortAtStart`（修正 await 后读完成时状态）；键与参数全部从快照派生；reloadGeneration 保留 |
| nav.ts → `markCurrentViewAllRead` | 经 `QueryScope.markScope` 只取订阅维度（layout 由 api.markAllRead 独立参数承载） |
| internals.ts → `mergeSnapshotEntries(…, fromBackend)` | bootstrap 三处后端快照传 **true**（bump 生效，行级 is_read 是真值）；nav 三处缓存恢复传 **false**（UI 回放非真值，不 bump 以保留本应正确的在途回滚——R2 审查裁定技术成立：经 reload 落进缓存的行在 bootstrap merge 时已 bump） |

**兼容性锁定**：scopePageKey/viewCacheKey 字符串形态逐字不变（`t109-key-format-compat` 断言含变异验证）。

## 5. TASK-110 筛选视图真分页（REQ-003）

**根因**：reloadFilteredEntries 是全库唯一 limit:100000 调用点，articlesExhausted 恒 true——旧文章在筛选视图不可达。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/internals.ts` | `ARTICLES_PAGE_SIZE=500` 收口单点；`viewFilterArgs(view)`（only_unread/only_starred/only_today，'all'→空对象 wire 逐字一致） |
| bootstrap.ts → `reloadFilteredEntries` | 首屏 PAGE_SIZE；`articlesExhausted` 真实判定 `rows.length < PAGE_SIZE` |
| bootstrap.ts → `loadMoreArticles` | 筛选视图续拉接线（:212 消费 `QueryScope.viewFilter`，:318 首屏同源）；追加按 id 去重（:259-264）；**游标按拉取行数推进**（:265-282，offset 漂移量=去重量，不跳行） |
| nav.ts → `toggleTimelineSort` | 两类视图同构重拉（旧「筛选视图本地重排全集」随分页化废除）；已加载水合正文由 merge(fromBackend=true) 继承 |
| nav.ts 三缓存恢复 | exhausted 真实判定 `cached.length < PAGE_SIZE` |

**策略裁定（有判别断言）**：同步插入致 offset 漂移 → **追加去重保序**，新条目不回填已加载窗口（保位优先于即时可见，随下次 reload 进入）；offset 分页对删除型漂移可能跳个位数行——已在 bootstrap.ts 注释声明为 offset 固有限制（keyset 属后续性能阶段）。

**测试锚点**：`t110-*` 16 条（wire/落地/去重/切排序/过滤参数/范围/布局）。

## 6. TASK-111 缓存实体预算 + 后台刷新保位（REQ-003）

**审计依据**：「单纯限制为 8 个视图并不能限制每个视图的大小」「后台刷新保留当前阅读位置」。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/internals.ts` | `VIEW_ENTRIES_CACHE_ENTRY_BUDGET=1000`；缓存值升级 `ViewEntriesSnapshot{entries, loadedCount, exhausted}`；写入唯一收口 `setViewEntriesSnapshot`（尾部截断+元数据） |
| nav.ts 三恢复路径 | 用记录的 `cached.loadedCount/cached.exhausted` 而非截断长度——exhausted 两态自洽（false 截断恢复游标不重拉；true 拦截伪续拉）；附带修复 TASK-110 偏移漂移（cursor 610/entries 600）场景恢复误判 |
| `src/components/timelineAnchor.ts`（新） | `TopAnchor`/`recordTopAnchor`（250ms 节流）/`clearTopAnchor`/`peekTopAnchor`/`anchorRestoreIndex` 纯函数 |
| `src/store/types.ts` + bootstrap/sync/feeds | `positionRestoreNonce`；三处内容刷新入口（feeds-updated/手动同步/单源刷新）传 `opts.keepReadingPosition`；导航/启动/设置路径不传 |
| Timeline.tsx | 回位 effect：先 `suppressNextScrollEvents()` 再 `scrollToIndex(align:'start')`（程序性滚动抑制复用既有机制） |

**三重隔离**（防误回位）：信号侧只有内容刷新入口传参；nonce 仅在「落地快照即当前视图」时 bump；消费侧 filterKey 逐字比对+画廊早退。

## 7. TASK-112/113 协议对账政策显式化 + 兼容矩阵（REQ-002/003，行为零变化）

| 锚点 | 内容 |
| --- | --- |
| `src-tauri/src/sync/conflict_policy.rs`（新） | `ReadDirection{RemoteReadWins, UnreadSetBidirectional}`/`StarDirection{AuthoritativeBidirectional}`；政策格常量 `GR_READ_DIRECTION`/`FEVER_READ_DIRECTION`/`GR_STAR_DIRECTION`/`FEVER_STAR_DIRECTION`（doc 载选择理由）；行级 `apply_read_by_policy/apply_star_by_policy` |
| `sync/greader_pull.rs` → `reconcile_reader_state`；`sync/fever_pull.rs` → `reconcile_fever_state` | 改为消费政策常量——**行为零变化**以修前语句逐列对照证明（GR RemoteReadWins ≡ 原 if-contains 无 else；Fever ≡ 原 want_read 求值时点；merged_states 的 Ok(n)/Err→0 ≡ 原 if let Ok(n)） |
| `sync/push.rs` read 广播分支 | 同文副本传播政策注释（DEC 第 6 条：保持广播现状，unread/star 不广播） |
| `docs/sync-compat-matrix.md`（新） | 协议×操作差异矩阵、冲突政策表、游标语义、服务端已知差异（2026-10 时点声明）、已知限制 L1-L5（含审计「游标变更时间假设」结论） |
| `db/articles.rs:186` | 过时注释修正（前端 PAGE_SIZE 实际 500） |
| 测试 | 9 条（政策格锚+GR 单向/Fever 双向/星标双向/pending 保护）——GR 单向此前无任何测试锁定（grep tests/ 核实） |
| **TASK-113（延续卡）** | CI clippy 修复 3 触发点：`sync/mod.rs` 删未消费 `use conflict_policy::*`（rustc 1.98 unused glob）、两测试模块 `///` 文档块改普通注释（empty_line_after_doc_comments/doc_lazy_continuation）——clippy-driver 抛置片段实证复现，行为零变化 |

## 8. TASK-114 五布局状态与快捷键统一（REQ-005/008，ui_change）

| 锚点 | 内容 |
| --- | --- |
| Timeline.tsx → `NotifCard` | 补水合三态（订阅 hydrationErrors/hydratedIds）；正文条件链 `fullText → hydrationError(重试行) → snippet → hydrated(暂无正文) → 加载中`——**R1 修**：fullText 提到错误前（对齐 SocialCard content 优先，消除「错误滞留+正文经详情路到达」组合态假报错）；展开按钮门控 **R2 修**为 `{isLong && (!hydrationError || !!fullText)}`（组合态钳制 snippet 可展开） |
| Timeline.tsx → SocialCard/NotifCard onKeyDown | Enter/Space→onSelect（`e.target===e.currentTarget` 守卫防劫持嵌套控件激活）；role 维持 `article`（role=button 会按 ARIA 掩蔽交互后代，卡内有 4+ 嵌套控件——裁定入契约实施记录） |
| `src/lib/jkNavigation.ts`（新） | `jkLayoutAllowed`（排除 image，与虚拟化开关同口径）+ `jkNextIndex`（与修前内联回绕逐条等价）；App.tsx 消费，J/K 从「仅文章布局」扩展到四虚拟化布局 |
| `src/components/settings/ShortcutsTab.tsx` | J/K 行改「文章/社交/播客/通知（画廊不支持）」 |
| `src/styles/base.css` | .notif-card 占位/重试两条镜像规则 |

**测试锚点**：`t114-*` 16 条（x1f 锁链序、x1g 锁门控——两处变异自证）。

## 9. TASK-115 返回位置统一规则（REQ-005，ui_change）

**探查基线**：切布局/视图/范围/排序一律 filterKey 归零丢位置；阅读器关闭只清字段无焦点归还；TASK-111 后台刷新保位是唯一回位场景。

**代码改动**：

| 锚点 | 内容 |
| --- | --- |
| `src/components/timelineAnchor.ts` | 锚存档 `stashTopAnchorForReturn/peekReturnAnchor`（Map<filterKey,TopAnchor> 按键覆盖+LRU 8）+ `rearmTopAnchor`（恢复落点重锚活锚并推进节流基准，防 250ms 窗口滞留旧位）+ `readerFocusReturnIndex` 纯函数；头注=全场景规则表（X3 载体） |
| `src/store/types.ts` | `switchRestoreNonce`/`readerCloseNonce`（nonce 计数器而非 id 字段：连续开关同一篇也要每次触发） |
| nav.ts 三缓存命中路径 | switchRestoreNonce 与 entries **同一次 set 原子写入**；`contextChanged` 守卫（同上下文重复导航不恢复）；cache-miss/切排序=新语境不恢复 |
| reader.ts → `clearReaderSelection` | bump readerCloseNonce（唯一关闭路径；action 只发信号不携带载荷） |
| Timeline.tsx | filterKey effect **先存档后清锚**；switchRestore effect（查档→决策→抑制→scrollToIndex→rearm，声明序保证先归零后恢复）；readerClose effect（ref 记账原选中卡→`focusCardAt` 归还，原卡可见不滚/不可见定位/列表已切回落）；`moveCardFocus` 收口为 `focusCardAt` 共用 |

**不叠加保证**：switchRestoreNonce 与 positionRestoreNonce 触发源/消费 effect/锚来源（存档 vs 活锚）三重分离；t115-x3a 双向断言。

## 10. TASK-116 同步四态展示（REQ-002/003，跨栈，ui_change）

| 锚点 | 内容 |
| --- | --- |
| `src-tauri/src/db/migrations.rs` v16 | sync_queue 增 `attempts INTEGER NOT NULL DEFAULT 0`/`last_error TEXT`（追加式 ALTER，旧库升级行得 0/NULL=「等待同步」与修前语义等价）；t116-r0 走生产 `open()` 验证旧库升级 |
| `src-tauri/src/db/sync_queue.rs` | `mark_push_failed`（attempts+1 + last_error 摘要，200 字符码点截断）；`sync_queue_stats`→`SyncQueueStats{waiting,failed,last_error}`（last_error 按 id 最大≈最晚入队，无独立时间戳列的取舍入注释） |
| `src-tauri/src/sync/push.rs` + `phases.rs` | `exec_push` 返回 `(done, failed=Vec<(queue_id,摘要)>)`（保持无 DB 访问）；失败标记+prune 同一锁临界区；**prune 语义零变化**（成功出队/失败保留重试） |
| `src-tauri/src/commands/sync.rs` + `lib.rs` | `sync_queue_stats` 命令注册（纯读，未连接可调） |
| `src/lib/syncPill.ts`（新） | `syncPillLabel`：优先级 **error > syncing > waiting > connected**（修复修前「失败被 syncing 覆盖」缺陷——修前 Sidebar isBusy 分支先于 error 判定）；`failed>0` 全分支追加「· 部分失败」；`syncStateSummary` 摘要真值表（错误按码点截 60 字符） |
| `src/lib/api.ts` + store + Sidebar.tsx + SyncTab.tsx | `syncQueueStats()`；store `syncWaiting/syncFailed`（刷新点收敛在 reloadFromBackend 的 syncStatus 读取点旁）；SyncTab 摘要卡（说明句并入 desc 收尾，守住既有「恰一块 mini-dialog-hint」断言） |
| `sync/greader_pull.rs` | seed_bound doc 笔误修正（延续任务登记项） |

**四态口径**（X3 如实原则）：本地已保存=事务化落库（TASK-108）；等待同步=队列行数；部分失败=attempts>0；远端已确认=prune 出队——**不可累计溯源，不虚构总数**。

## 11. 四阶段性能测量（无需优化的证据结论）

- 工具：`tools/phase4_measure.mjs`（CDP 电池：内存/滚动帧率/切换延迟/搜索）、`tools/phase4_seed.py`（规模注入+备份/还原纪律）。
- 结果矩阵与判定：见 `PHASE4-MEASUREMENT-20261007.md`——20k/50k 实测内存平坦（6.5-8MB）、滚动 p95=10.1ms 零卡顿、切换 63-78ms、搜索 ~550ms；审计三条触发线全部未触发；**SQLite 单连接保持**（审计：按锁等待/查询耗时证据决定）。

## 12. 验证体系与复跑入口（评估者用）

1. **前端回归**：`npm run test:frontend` → 655/655（既有 26 + t103×16/t104×17/t109×9/t110×16/t111×20/t114×16/t115×21/t116×13 + 既有真值表）。断言定位：tools/frontend-regression.mjs 内按 `t1xx-` 前缀检索。
2. **Rust**：`cargo fmt --check` 本地；`cargo test`/`clippy --all-targets -- -D warnings` 以 CI rust job 为准（tag v0.17.0 运行全绿）——本机无 MSVC 链接器（DEC-local-cargo-gate-20261005）。
3. **变异复核样例**（审查者均已独立复现，可重放）：删 merge bump→t104-snapshot-voids 红；置空 stashTopAnchorForReturn→t115 5 红；删 timelineAnchor filterKey 拦截→t111-5b 红；移除 notif-expand-btn 的 `|| !!fullText`→t114-x1g 红；回退 pill 优先级→t116-x1a/x1d 红。
4. **提交链**：dev `30cd2bc..4bb771c`；main 合并点 798ed97/82b2488/3d011b7/99d692b；tag v0.17.0。
5. **工作流记录**：`.workflow-kit/tasks/items/TASK-106..116.json`（运行史/候选摘要/审查运行）；审查报告 `tmp/refactor-20261005/TASK-*-review-report*.json`；UI 证据 `.workflow-kit/tasks/evidence/TASK-{114,115,116}-ui/`。

## 13. 已知偏差与遗留（评估者应关注的诚实清单）

1. 门禁环境误配事故（103/104/105 取消重建为 106/107/108）——流程教训已入 journal；
2. TASK-109 worker 曾违反「禁 git checkout」（代理自行上报，重做面经审查逐行核对）；
3. UI 契约文档（protected_paths）与 worker 边界的多次 scope 摩擦——最终模式：worker 增补暂存 tmp/，主控落账；
4. TASK-112 的 CI clippy 失败由延续卡 113 修复（DEC-local-cargo-gate 盲区的固有权衡）；
5. 四阶段测量首轮曾因时间戳单位与电池设计产生误导数据——已修正重测，结论只基于修正后数据；测量前未先走 seed 脚本备份路径（内联注入，当时库为空）——纪律缺口已披露；
6. 遗留决策点：同文副本传播策略、顶部豁免特例、reader 错误态状态卫生微任务、真实服务端同步耗时测量、本机 VS Build Tools。
