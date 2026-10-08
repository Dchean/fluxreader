# 第五阶段（审计驱动重构）技术评估文档

- 用途：供**第三方审计**快速核对本次工作 —— 对照 `tmp/audit-20261007/REVIEW.md`（下称「审计」）逐项给出**技术思路、备选取舍、代码锚点、测试锚点、原始证据**。
- 锚点修订：**dev@15705cc / main@cdb9bb3**（两者业务代码逐字一致；main 不含 `.workflow-kit/`）。行号可能随提交漂移，**以符号检索为准**。
- 审计基线：dev@d9448ee（对照前次审计基线 07a0db9）。
- 配套文件：门禁调整 `DEC-gate-adjust-20261007.md`、前四阶段技术文档 `REFACTOR-TECH-REPORT-20261007.md`、四阶段测量报告 `PHASE4-MEASUREMENT-20261007.md`、owner 实机清单 `tools/phase4_checklist.md`。
- 一句话交付：审计的 2 项 P1 + 6 项 P2 全部处置（P2 中 6 项实现修复、1 项测量工具重写、1 项协议矩阵重排）；前端回归 **655 → 759**（新增 t117/t118/t119/t122/t123/t124 六族共 136 条断言）；Rust 单元测试 **+8**（`db/articles.rs` 18→21：keyset 三例含可变集合续拉；`db/sync_queue.rs` 3→5：事件 payload 口径、推送阻塞标记；`sync/push.rs` 0→1：认证 vs 网络失败分类；`sync/conflict_policy.rs` 1→3：成功写入行数、DB 写失败传播），全部由 CI rust job 执行；dev→main 合并后 main CI 全绿。

---

## 0. 审计者快速核对入口

```bash
# 代码面（main 与 dev 业务代码一致，任选）
git log --oneline -1 origin/main          # cdb9bb3 merge: dev → main（不含工作流内容）
git diff origin/dev origin/main --stat -- src src-tauri tools docs   # 应为空

# 全部门禁复跑（本地；cargo test/clippy 见下方说明）
npm run lint && npm run build && npm run test:frontend   # 期望 759/759
cd src-tauri && cargo fmt --check

# 关键回归块（只跑前端状态机，node 直跑无需浏览器）
node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs   # 断言标签含 t117-/t118-/t119-/t122-/t123-/t124-

# CI（需认证明细；匿名可看 run 列表与合并状态）
#   git credential fill 取得的 token 可直接 GET .../actions/jobs/<id>/logs（实测 HTTP 200）
```

**本地环境限制（重要，勿误判）**：本机**没有 MSVC 链接器**（`cargo test`/`clippy` 无法链接），故这两个门禁在任务卡里为 `required:false`（依据 `DEC-local-cargo-gate-20261005`），**由 CI rust job（windows-latest）承担**。补偿手段：审查者用 `rustc --emit=metadata` 做片段探针复核编译正确性（TASK-121/124 先例）。

**证据索引**（每条断言/发现的原始材料都在 `.workflow-kit/tasks/`）：

| 任务 | 上游问题 | 提交（dev） | 验证 RUN | 审查 RUN | 验收 DEC |
| --- | --- | --- | --- | --- | --- |
| TASK-117 | P1-1 可变集合 OFFSET 分页 | b7947e9 | RUN-ac201789cc76419e87ee62468a7fedf8 | RUN-f72b939388e44a01a06521a93d56e1e3 | DEC-5d88a33cd5de49d7bf0b2c5c5d8d91dc |
| TASK-121 | P1-1 延续（CI E0596 修复） | f195aec | RUN-97860d5bd86d4f19a95f860be2d27c70 | RUN-312a98e11fc74f54890b2e9a66d463ab | DEC-cf0a3d2742c54cf79e4e31d59a24e2fa |
| TASK-118 | P1-2 跨字段版本 | feae9cd | RUN-7509ff497f1349129793937b1470f602 | RUN-8f5290f4a25349f5921f6b56700f8ff5 | DEC-22272821c4c64877a402352bbe918cc3 |
| TASK-119 | P2-4 请求/计数过期 | 7a3d0f4 | RUN-5880f0b894da46d48586c230cf7709bc | RUN-dfe3d62ba96747d3ac194b649235d62b | DEC-cf0a3d2742c54cf79e4e31d59a24e2fa |
| TASK-122 | P2-3 实体缓存与失效 | 91059a3 + eba4df5（R1 P0） | RUN-e4a1095b1ef244739fe319c2128dbcf4 | RUN-4f92ab4690554440a77baf5412ffdaa5 | DEC-00de125f547c4591b1d8be0e3be19f6d |
| TASK-123 | P2-5 保位窗口 | 4f040fc | RUN-435771d3935a4dc3839bc8d6949baca3 | RUN-36b10dd1e41a47ba9ac397d0fcce946d | DEC-7bcb581dc5b64482bfadecebd685377e |
| TASK-124 | P2-6 同步事件 | 2f38ff2 + 8a4ca5e（R2 CI） | RUN-c5788168f4404eceb7aa90f2297a6d8c | RUN-64854e14b4714187b47f94222e4d8ea6 | DEC-cfb13b2ec3a44f1cb07cc877b1a2b6d3 |
| TASK-127 | P2-8 矩阵三列化 + 冲突层 Result | 0cc84e6 | RUN-b72ae07fa51941fd88770bb0601e7f52 | RUN-05c4c67a52a2420eb2d59972366d3a9c | DEC-0b906820888c47c28194164e3d135715 |
| TASK-128 | P2-7 测量纠正与补测工具 | 2275f02 + b7c3ed8（R1） | RUN-b269c104e7a744c1a0d2c883de4b2bea | RUN-999aa4d5487b45738d2de39266416a33 | DEC-013d34d427db48b1a727615fb8244d3e |
| TASK-125/126 | 同 P2-8，范围缺陷重建 | （取消） | — | — | cancel 记录在 task.dispositions |
| TASK-120 | 同 P1-1，范围缺陷重建 | （取消，由 121 承接） | — | — | 同上 |

交付面：合并提交 `cdb9bb3`（main），CI run 37723115685 全绿；工作流记录分支 dev@15705cc。

---

## 1. P1-1 可变集合分页：OFFSET → keyset 游标（TASK-117，延续 TASK-121）

**问题**（审计 §1）：`WHERE is_read=0` 这类**可变集合**上 OFFSET 不等于「用户已看过的条数」。1200 未读加载前 500 篇、全部标读后再翻页 → 集合只剩 700，仍 OFFSET 500 → 跳过剩余前 500 篇、只取最后 200 并宣告 exhausted。收藏视图取消收藏同理。审计探针：loaded=700 / missingUnread=500 / exhausted=true。

**技术思路**：把「偏移量」换成**稳定排序键的游标**，使分页谓词与 `ORDER BY` 逐字同构（避免并列值造成跨页重复/漏读）：

| 锚点 | 内容 |
| --- | --- |
| `src-tauri/src/db/articles.rs` → `KEYSET_PREDICATE_DESC/ASC`（≈:179/181） | `(a.published_at < ? OR (a.published_at = ? AND a.id < ?))` 与其 ASC 镜像；与 `PUBLISHED_ORDER_DESC/ASC`（≈:165）**同一表达式口径**——谓词不匹配排序就会出漏洞，注释里写明这条不变量 |
| 同上 → `ArticleQuery.last_published/last_id`（≈:146） | 外部字符串只经绑定参数进 SQL；`last_published` 是 RFC3339 TEXT，比较必须是裸字符串比较 |
| `src-tauri/src/db/migrations.rs` → v17 `idx_articles_published_id`（≈:346） | `(published_at, id)` 复合索引，使 keyset 谓词可走索引而非全扫 |
| `src/store/internals.ts` → `ArticlesCursorState` / `advanceArticlesCursor` / `emptyArticlesCursor`（≈:161-169） | 前端游标**单点推进**：续拉传游标现值，首屏传空游标；「已加载条数」与「游标」分离，解决「过滤视图中一次取满多页」的语义 |
| `src/store/slices/bootstrap.ts` → `loadMoreArticles`/`reloadFilteredEntries` | 续页/首屏共用同一游标语义；TASK-119 起携带「首页代际」（见 §4） |
| `src/lib/api.ts` → `last_published/last_id`（≈:166-171） | 两者成对给出才生效，缺一回落 OFFSET 既有语义（兼容窗口，注释明示） |

**备选取舍**：审计另给「固定查询快照的 ID 序列」；未采用——需额外快照表/生命周期管理，而 keyset 只需一个复合索引且与既有 ORDER BY 天然一致（记录在任务卡「参考方案」）。

**测试锚点**：Rust `src-tauri/src/db/articles.rs` 的 `#[cfg(test)]`：`keyset_pagination_matches_offset_and_resolves_ties`、`keyset_continuation_tracks_mutable_unread_collection`（**探针本体**：读一篇翻一页，断言不跳行）、`keyset_tie_group_spans_page_boundary_without_gap`（并列值跨页边界）。前端 `t117-*` 21 条（续拉 wire 从 offset 改为 keyset 锚、代际守卫比对 per-scope 游标等）。

**过程事故（审计者应关注）**：TASK-117 首次落地漏了 `let (mut where_clauses, mut params)` 的 `mut`（E0596），导致 dev 上 117/118/119 三次推送的 rust job 连续失败；日志需登录（403），最终以**一手 CI 日志**定罪（TASK-121，f195aec），同期补齐 `tests/ingestion_e2e.rs` 的 `ArticleQuery` 字面量（E0063）。同时披露 TASK-117 worker 的「cargo check = 0」为不实记录。详见 `TASK-121` 的 dispositions 与审查报告。

---

## 2. P1-2 跨字段操作版本：articleId × field（TASK-118）

**问题**（审计 §2）：版本守卫是**文章级**的。未读文章 → 全部已读在途 → 用户收藏（bump 同一 article 版本）→ 全部已读失败 → 读状态回滚被误判「已被接管」而跳过。探针：应为 isRead=false/unread=1，实际 isRead=true/isStarred=true/unread=0。

**技术思路**：版本键细化为 `articleId|field`，并把三处回滚路径合并到**同一助手**（消除「各写各的守卫」）：

| 锚点 | 内容 |
| --- | --- |
| `src/store/internals.ts` → `entryVersionKey/getEntryVersion/bumpEntryVersion`（≈:443-452） | 版本键 `${id}|${field}`；`entryMutationVersion` Map 单一版本源 |
| 同上 → `bumpEntryVersion` 调用点矩阵（≈:397-398 及标记写点） | 真实写入才 bump：`flipEntryFlag`、`markEntriesRead`、`mergeSnapshotEntries`（快照替换使在途乐观声明失效）；回滚本身不 bump（消费方只做相等比较，注释给出理由） |
| `src/store/internals.ts` → `EntryRollbackClaim` + `rollbackEntryClaims`（≈:584-640） | **统一回滚助手**：声明形态 = id+field+prev+version；两道守卫（值等快速短路 → 版本守卫），计数按行恢复方向逐 feed 回补（`Math.max(0,)` 钳制） |
| `src/store/slices/reader.ts` → `optimisticEntryFlagToggle`（≈:642-670）、`markEntriesReadBulk`（≈:429-475）、打开即标读（≈:105-116、462-468） | 三条**此前只提示不回滚**的路径全部接入统一助手（审计「相邻缺口」一并关闭） |
| `src/store/slices/nav.ts` → `markCurrentViewAllRead` 失败分支（≈:320-380） | 快照与守卫都取 **isRead 字段**版本（修前文章级共享版本会因收藏 bump 而误跳过） |

**测试锚点**：`t118-*` 14 条，含审计探针本体（收藏不 void 读回滚）+ 反向判别（读回滚不得 void 收藏回滚）+ 批量/打开即标读回滚。

---

## 3. P2-3 正文/AI 实体缓存分离与显式失效（TASK-122，含 P0 事故）

**问题**（审计 §3）：`mergeSnapshotEntries` 用 `||` 回退继承 `aiSummary/translatedContent`——清理 AI 缓存（DB 置 NULL）后 reload，UI 又把旧值「复活」；正文无条件保留能修死区，但**没有内容版本/失效机制**，内容会长期陈旧。

**技术思路**：把正文/AI 从「视图行」彻底剥离为**独立实体缓存**，用判别联合状态机 + 显式失效取代继承：

| 锚点 | 内容 |
| --- | --- |
| `src/store/bodyCache.ts`（新，346 行） | 模块级 `bodyById`（LRU `BODY_CACHE_MAX=2000`，≈:107/140）；判别态 `BodyState = loading\|ready\|cleared\|missing\|failed`（≈:82）；单点写入 API：`markBodyLoading/applyBodyRow/markBodyMissing/markBodyFailed/dropBodyEntry/applyExtractedFulltext/applyAiProduct/appendAiDelta`；**唯一失效入口** `reconcileBodyEntities`（≈:307）：行携带 NULL → cleared、内容变更 → bump `contentRevision`、列表 snippet 变化 → 删记录重取、stamp 守卫防迟到响应复活 |
| `src/store/selectors.ts` → `selectArticleBody`（≈:97） | **读取单点**（卡片与 Reader 都经它）；订阅依赖经 `bodyCacheNonce`；无记录时回退视图行 AI 列（标注为**过渡兼容位**，待 Rust DTO 分离后删除） |
| `src/store/internals.ts` → `mergeSnapshotEntries`（≈:350） | 剥离正文/AI/译文继承（只留 url 轻字段）；`isRead/isStarred` 的 bump 语义与 `fromBackend` 门控**逐字保留** |
| `src/store.ts` → `bindBodyCacheNotify`（≈:60） | 缓存写入 → bump `bodyCacheNonce`，建立「模块级实体 → zustand 订阅」的桥 |
| `src/components/settings/CacheCleanupSection.tsx` | 显式失效指针注释（reload 携带清理后的行真值） |

**备选取舍**：审计建议「内容版本 + 失效状态」；实现取判别联合 + `contentRevision`（可断言、可 LRU 淘汰），未引入 TanStack Query（审计明确其非必要）。

**测试锚点**：`t122-*` 31 条（清理失效/重取/语义区分/LRU 预算/死区回归/stamp 守卫）；改写的既有断言（`S-1/S-2/S-4`、`(a)`）逐条附「为何锁的是旧手法」的理由注。

### 3.1 P0 事故：`selectArticleBody` 订阅缺 `useShallow`（R1，eba4df5）

**这不是审计项，是我方引入的回归，由独立复审发现**：`selectArticleBody` 有记录时每次返回**新对象**，而 Reader / SocialCard / NotifCard 三处订阅未包 `useShallow` → zustand v5 把 selector 直接交给 `useSyncExternalStore`（无快照缓存）→ `getSnapshot should be cached` → React #185「Maximum update depth exceeded」，**打开任意文章即崩**。

- 复现证据：我用 headless Chrome + CDP（`tools/t059_cdp.mjs`）对 dev 构建实测——点击卡片后 reader 不渲染并抛 #185；包 `useShallow` 后同场景零错误（A/B 对照）。
- 修复：三处订阅包 `useShallow`（`Reader.tsx:53`、`Timeline.tsx:697/1124`）。
- 防护：新增 `t122-9` **源级锁定**（扫描全 `src`，任何引用 `selectArticleBody` 的 `useAppStore` 调用未包 `useShallow` 即红；实现含注释/字符串掩码，审查者用「还原修前代码」验证过它会红）。
- 教训入册：审查者指出前端回归是 node-only 无 DOM，**不能代替真实渲染验证**——这条写进了 `DEC-gate-adjust-20261007` 的「真实挂载组件测试通道」评估项。

---

## 4. P2-4 请求与计数过期覆盖（TASK-119）

**问题**（审计 §4）：两个窗口——① 同一 scope/view/sort 两次筛选请求，新请求先返回、旧请求后返回并覆盖；② 全部已读成功后取计数（0），随后用户改回未读，旧计数响应整体替换，UI 未读=0 而实际有未读。守卫只比较「查询参数相同」，不识别同一查询的旧版本，也处理不了 A→B→A。

**技术思路**：引入**查询实例代际**（全局 + 每键）与**本地写入序号**，让「谁更新」可判定：

| 锚点 | 内容 |
| --- | --- |
| `src/store/slices/bootstrap.ts` → `queryGeneration` / `bumpQueryGeneration` / `firstPageSerialByKey`（≈:69-96） | 全局单调序号（**全局状态/窗口写入者**用：reloadFromBackend、缓存恢复、保位窗口）；`firstPageSerialByKey` 为**首屏代际**（同键两次发起，先发后至的旧版本被拒；A→B→A 也正确） |
| 同上 → 落地判据 | 续页携带「首页代际」而非自增（续页不是新查询实例）；`paginationStale` 具名守卫（`src/store/internals.ts` ≈:269） |
| `src/store/internals.ts` → `currentLocalFlagWriteSerial`（≈:479-489） | 读/藏**真实写入**（翻转或恢复）时推进的序号；bump 点矩阵与「回滚不 bump 的理由」写在注释里 |
| `src/store/slices/nav.ts` → `reconcileCounts`（≈:346-375） | 对账发起时快照序号，落地时比对：`applied`（替换计数）/`stale`（丢弃并重取）/`empty`；重取失败降级为「下次 reload 自愈」 |

**测试锚点**：`t119-*` 21 条，含两个探针本体（迟到首屏响应被拒；计数快照被本地写入 void 后不被旧响应覆盖）。

---

## 5. P2-5 保位窗口：深页刷新不再丢锚（TASK-123）

**问题**（审计 §5）：后台刷新固定 `offset=0/limit=500`，加载 1000 篇读到第 750 篇时刷新，列表缩回前 500 篇、锚不在 → 程序根本无法兑现「后台刷新原位保持」。另有「缓存返回先定位、导航又无条件重拉第一页把锚删掉」的组合问题；锚只存 ID 不存卡片内像素偏移；250ms 节流无尾沿补记。

**技术思路**：刷新不再「重建第一页」，而是**围绕当前已加载窗口按 keyset 续拉重建**：

| 锚点 | 内容 |
| --- | --- |
| `src/store/slices/bootstrap.ts` → `fetchWindowRows`（≈:153-188） | 窗口目标 `windowTarget` = 发起时已加载条数；循环 keyset 续页直到「长度达标 或 发起时底锚已被覆盖」；**防呆上限** `ceil(target/PAGE_SIZE)+1`（底锚被真删除时收敛而非无界追取）；短页/空页即停；每页核对代际 |
| 同上 → `reloadFromBackend`/`reloadFilteredEntries` 的 keepReadingPosition 路径（≈:286-347、588-633） | 未带信号时行为与修前**逐字一致**（窗口目标 0 则跳过，断言 t123-0 锁定） |
| `src/components/timelineAnchor.ts` → `recordTopAnchor/commitTopAnchor/rearmTopAnchor`（≈:143/155/…） | 锚载荷增加 **intra-item 像素偏移 `offsetPx`**（= scrollTop − 顶条卡片虚拟起点）；`commitTopAnchor` 为**尾沿补记**（绕节流）；`rearmTopAnchor` 同口径 |
| `src/components/Timeline.tsx` → 锚接线（≈:186-220、305-364） | 记录/提交/恢复/偏移应用单点；`applyAnchorOffsetPx` 在 `scrollToIndex(align:'start')` 之后补偏移 |
| `src/store/slices/nav.ts` → 三处缓存命中路径（≈:116-117、180-181、243-244） | 缓存命中的刷新携带 `keepReadingPosition`，修掉「返回锚在第二页但重拉第一页又删锚」的组合缺陷 |

**R0 审查发现的低危缺陷（已在 R1 修）**：续页失败被外层静默 catch 吞掉（重开 TASK-067 N10 已关闭的「后台刷新失败不可见」缺陷类）。R1：`try/catch` → toast「刷新失败：…」+ **rethrow**，失败不落地、不缩窗；`t123-6` 行为级锁定。

**测试锚点**：`t123-*` 34 条（探针本体+组合+intra-item+尾沿+截断边界+失败可见），四处变异自证（移除以保位窗口重取即红）。

---

## 6. P2-6 同步状态变化事件（TASK-124）

**问题**（审计 §6）：统计刷新只集中在 reloadFromBackend，普通标读/收藏入队与推送出队**没有刷新事件**——探针实测本地写入后队列 1 项而 pill 仍「后端已同步」。另：认证/端点构建失败在 `build_client` 阶段提前返回、未进 `exec_push`，`attempts/last_error` 对这类失败永不记录（真值表测试不能证明真实状态会到达文案）。

**技术思路**：把「状态变化」变成**事件**，让 UI 与同一状态源同步；并把构建失败从 `Option` 中解放出来分类上报：

| 锚点 | 内容 |
| --- | --- |
| `src-tauri/src/sync/credentials.rs` → `ClientBuildFailure{NotConfigured, Failed}`（≈:33）、`build_client -> Result`（≈:126） | 区分「未配置」（静默，队列保留待连接后补推）与「有凭据但认证/端点/网络失败」（上报）；调用方 `phases.rs`/`subscriptions.rs` 相应改写 |
| `src-tauri/src/sync/push.rs` → `QUEUE_EVENT_TARGET: OnceLock<AppHandle>` + `init_queue_event_target`（≈:192-197） | 事件目标用**注册表**而非改函数签名：`push_states_now/states_phase` 的二参签名被 tests/ 大量调用锁定，加参会波及不可修改的集成测试面；集成测试不注册 → 发射点 no-op |
| 同上 → `push_block_summary`（≈:207）、`record_push_block`（≈:249） | 认证失败 vs 网络/端点失败分类（`code=="auth"` 或含 `ClientLogin`）；阻塞时对**全部现存队项** attempts+1 + 摘要（`mark_push_blocked`），确有标记才发事件（统计真变了才发，不做噪音） |
| 同上 → `notify_queue_changed`（≈:227） | 锁外自持短锁读统计后 emit；载荷经 `db::queue_changed_payload` 单点（`sync_queue.rs` ≈:195）——未配置 → `None`（静默不误报） |
| `src-tauri/src/commands/articles.rs`（≈:139/185/197/226）与 `phases.rs` 推送段 | 事务提交后 / 推送确认·失败后 / 老化清理后各发一次（同一 `notify_queue_changed`） |
| `src/App.tsx`（≈:135-150）→ `src/store/slices/sync.ts` → `applySyncQueueChanged`（≈:173） | 监听器**唯一落点**：只写队列三字段（waiting/failed/last_error），不触碰连接态（越权防护写进注释并被断言锁定） |
| `src/lib/syncPill.ts` | pill 优先级链（错误 > 同步中 > 等待 > 已连接），随真实事件驱动 |

**测试锚点**：Rust `t124-r1/r2/r3`（payload 口径 / 认证 vs 网络分类 / 阻塞标记累加）在 CI 执行；前端 `t124-*` 15 条（入队即时呈现/出队恢复/失败呈现/认证区分/未配置静默/接线源级锁/SyncTab 跟随）。

**R0 审查发现（已在 R1 修）**：`t124_tests` 跨模块调用 `t116_tests` 内的私有 helper `attempts_of` → **E0425 必炸 CI**（本地不跑 cargo 故门禁未拦）。R1：helper 上移文件级 `#[cfg(test)]`（`sync_queue.rs` ≈:211-224），审查者用 `rustc --emit=metadata` 双向复现机制。

**R2（CI 门禁）**：CI clippy `-D warnings` 报 4 处 `doc_lazy_continuation`（`push.rs:204/205/336/337`，文档注释列表续行未缩进）→ 以 `///` 空行分段修复（8a4ca5e，纯注释）。**一手 CI 日志**经 `git credential fill` 取得的 token 下载（HTTP 200）——这是本阶段建立的取证手段。

---

## 7. P2-7 性能测量纠正与媒体/并发补测（TASK-128）

**问题**（审计 §7，六点）：切换完成判据「存在任意卡片」在旧卡未换掉时即成立（63-78ms 主要是 50ms 轮询开销）；搜索计时起点不实、完成判据可能命中背景文本、超时仍输出普通耗时；50k 数据四布局 0 卡、社交为纯文本无解码压力；JS 堆 ≠ 进程内存、长会话未测；只测顶部 3 秒；独立 SQL 无锁竞争。

**技术思路**：**先保证测量正确，再谈优化**（审计原话落为任务非目标「本卡不做任何性能优化」）：

| 锚点 | 内容 |
| --- | --- |
| `tools/phase4_measure.mjs`（重写） | ① 切换延迟双判据：卡片**身份基线变化**（`[data-ctx=article][data-id]` 集合）→ `firstResultMs`，随后连续 3 帧 rAF 无 DOM 变更（MutationObserver）→ `stableMs`；rAF 粒度为 16ms；超时写 `timedOut` **不输出毫秒**。② 搜索分段 `openMs/resultMs` + **结果归属断言**（命中必须落在 `[role=group][aria-label="文章"]` 分组内、文本含唯一 token、结果数与基线不同）+ **负向探针**（不存在 token 必不命中）+ 键入前 token 已在面板即判 `invalid`（不给 resultMs）。④ 内存：JS 堆与**进程 RSS** 分列（WebView2 进程按命令行含 `com.fluxreader.app` 过滤；机器上有 47 个同名无关进程，不过滤会把别人的内存算进来），长会话 8 轮逐轮堆采样 + 首末（GC 后）增量，**中断则增量作废**而非输出假值。⑤ 滚动 top/mid/bottom 三档，每档等虚拟列表静默 300ms 再采 3s rAF。⑥ 锁等待**代理**：`sync_queue_stats` IPC 往返 20 次 p50/p95/p99（空闲 / 同步在飞两态），标注为代理而非锁计时；`--no-sync-probe` 可跳过会触发真实同步的那一段。⑦ 报告带 `conditions` + `conditionsBySection`，**每条结论行前置本段条件**（电池中途自己触发同步后，后续段的结论不再印「同步在飞=false」）。⑧ 崩溃布局（渲染树被卸载）堆值置 `null` + 失效原因，不进结论行 |
| `tools/phase4_seed.py` | `--media`（`image_url`/`author`/`duration_sec`/`enclosure_url`，picsum/SoundHelix 稳定 URL）、`--layouts`（按 `feeds.layout` 轮转铺满五布局，源不足自动补种子源）；`--info` 增加按布局卡数与媒体覆盖；**备份/还原纪律不变**（逐字节+SHA256），新增「已有干净基线时拒绝覆盖」与**还原前检测 FluxReader.exe 在跑则拒绝**（`--force` 才放行，文档标危险） |
| `tools/phase4_checklist.md`（新，owner 实机用） | 前置（副屏规则、调试端口）、六步流程（备份→注入→启应用→跑电池→**先退出应用**→还原校验）、字段含义、判定线、**代理指标专节**、失败处置表（一律先还原）、以及「跑电池会触发一次真实同步」的必读警告 |

**备选取舍**：仍用既有 CDP 基建（`tools/t059_cdp.mjs`，**受保护不可改**），不引入 Playwright/Puppeteer；真机测量留给 owner（本机无 MSVC、无真实同步后端）。

**测试锚点**：`node --check`、`python -m py_compile`；**烟雾跑通**（Vite dev + 无头 Chrome CDP，mock 模式）验证工具不崩、各测量段产出结构化字段、非 Tauri 段如实 `skipped`；lint/build/frontend 不回退。

**R1（审查 FAIL → 修复）**：独立复审提 6 findings，其中两条**仍会产出/标注误导性数字**——F1 搜索正例可被浮层内「订阅源」条目抢先（防呆用的 overlayText 在浮层未打开时读取，等于死代码；审查者实测 token=「APOD」在旧判据下 203ms 命中订阅源行）；F2 结论行条件取自电池开头、与中途触发的同步自相矛盾。R1 逐条修复并用**对抗性探针**验证（token 为订阅源名子串 → 新判据不命中且判 `invalid`；注入同步在飞 → 后续段结论正确显示 true）。

---

## 8. P2-8 协议能力矩阵三列化与冲突应用层错误可见（TASK-127）

**问题**（审计 §8）：矩阵把 Fever 写成「无全量历史端点」，**把「本客户端未实现」说成「协议做不到」**（审计核查当前 Miniflux `internal/fever/handler.go` 约 227-267 行确认 `items&max_id` 支持向更旧翻页）；三类事实（协议支持/客户端实现/服务端版本已验证）混在一起不能作为验收矩阵；`apply_*_by_policy` 用 `unwrap_or(0)` 把 DB 写失败吞成「没有变化」。

**技术思路**：**事实分层**（文档）+ **错误分层**（代码）：

| 锚点 | 内容 |
| --- | --- |
| `docs/sync-compat-matrix.md`（§1 表，约 :32 起） | 表头改为 `维度 \| 协议/服务端支持 \| 本客户端已实现 \| 指定服务端版本已验证`；Fever 条目获取行如实写「协议/当前 Miniflux 支持 `items&max_id`（向更旧，重复直到空数组）」，新增**「历史回溯（向更旧翻页）」专行**标 `未实现`；文首与 §5 加**版本验证声明**（仅 mock 替身自动锁定，Miniflux/FreshRSS 具体部署版本未逐一验收）；§2 明确四格政策**均为客户端选择的冲突策略**而非协议必然要求（GR read-wins 三处显式标注） |
| `src-tauri/src/sync/fever.rs`（模块头 ≈:17、`items_since/items_recent` 文档） | 能力记录点（协议支持 vs 本客户端未实现），**纯注释**，供后续适配器能力报告消费；不实现 max_id 翻页（任务非目标） |
| `src-tauri/src/sync/conflict_policy.rs` → `apply_read_by_policy`/`apply_star_by_policy`（≈:124/154） | 返回类型 `usize` → `AppResult<usize>`，**移除全部 `unwrap_or(0)`**；失败不再按 0 计 |
| `src-tauri/src/sync/greader_pull.rs` → `reconcile_reader_state`（≈:266）；`fever_pull.rs` → `reconcile_fever_state`（≈:193） | 返回 `AppResult<()>` 并 `?` 传播；调用点把 `Err` 记入 `report.errors`（该 report 经命令返回到前端，由 `src/store/syncErrors.ts` 呈现）——失败**端到端可见**，锁纪律保持（DB 写仍在短临界区内） |

**测试锚点**：`conflict_policy.rs` 新增 `apply_read_by_policy_returns_written_rows_on_success`、`apply_read_by_policy_propagates_db_write_failure`（无表内存库 → UPDATE 失败 → 断言 `Err`；旧 `unwrap_or(0)` 语义下必红），政策格方向锁定测试保持；矩阵事实由审查者**重新抓取上游源码**独立核对。

**范围两次修正（审计者应关注流程）**：TASK-125/126 两版因 `allowed_paths` 未含 `src-tauri/src/fever.rs`（`prepare` 的 allowed_paths 取自 `snapshot_paths`，第一版 spec 的 scope 对象不被读取）被范围门禁拦下并 `cancel`，第三版（TASK-127）把该文件纳入 `snapshot_paths` 后交付。取消记录在 task.dispositions，未改 ID 清零、未重置预算。

---

## 9. 门禁清理与调整（`DEC-gate-adjust-20261007`，先于 P1 修复执行）

依 owner 指示「现有工作流中的门禁如果有不适用或者不够严谨的，先进行清理和调整」：

1. **探针→回归纪律**：外部审计的行为探针必须转化为**真实行为回归**（修什么锁什么）；静态分页/单路径断言不再视为充分验收，**操作序列场景**（阅读中连续翻页、跨字段并发、迟到响应、缓存清理失效）纳入验收 —— 本阶段所有修复卡都以「探针本体转回归」为硬性验收项。
2. **源级字符串断言降级**：仅保留「单点性必要」场景（如 `t122-9` 的唯一入口防回退、`t124-6` 的接线锁定），新卡默认不要求。
3. **既有断言不回退**：655 条不弱化；允许按行为变化更新**被证明锁旧缺陷**的断言（附理由注，如 `(a)`、`buildEntries` 副作用清除、`S-1/S-2/S-4`）。
4. **真实挂载组件测试通道**：列为独立评估任务（涉 devDependency 决策），不阻塞本轮 —— TASK-122 的 P0 之后，其必要性被实证（node-only harness 无法覆盖渲染崩溃）。
5. **回归网按领域拆分**：候选微任务，本轮不阻塞。
6. **测量工具修正**随本轮提交入库。

---

## 10. 诚实清单（评估者应重点核对的未决项）

1. **NotifCard #185 不可稳定复现（待观察）**：TASK-128 首轮烟雾在 worker 自起 Chrome 上捕获过一次 `getSnapshot should be cached` → #185（`tmp/phase4/measure-smoke.json` 的 `appHealth.crashes`，两份，分别落在 `longSession` 与 `memoryByLayout:通知`）；该 JSON **未存组件栈**，worker 报告称报错组件为 `<NotifCard>`（现场观察，无存档）。TASK-122 R1 的 `useShallow` 修复发生于该烟雾**之前**。我的复现尝试全部阴性：完整电池 ×2、快速五布局切换 ×6 轮、GC+布局循环、Reader 打开/AI 按钮/Escape、命令面板输入；全 `src` 的 17 处非平凡订阅逐一审查（均为标量或稳定引用）。**未阻断交付**，记为待观察项；backlog 已列「给测量工具的错误钩子补 componentStack 捕获」以便复发归因。
2. **本地无法运行 `cargo test/clippy`**（无 MSVC 链接器，`DEC-local-cargo-gate-20261005`）：Rust 编译与测试的权威判据是 CI rust job；本阶段 CI 曾两次拦截真实缺陷（E0596、clippy doc_lazy_continuation），说明该分工有效，但**Rust 侧的本地预检能力仍缺**（建议 owner 装 VS Build Tools）。
3. **审计未要求的相邻缺口仍在**：`scheduler.rs` 封面回填的 DB 写仍用 `unwrap_or(0)`（TASK-127 审查观察项，文件在 allowed_paths 外）；乐观写入虽已全部接入统一回滚，但**失败提示与回滚的分工**仍是调用方逐点写（未收敛为单一协调器）。
4. **四阶段第④项（同文内容/各源条目独立建模）未实现**：本阶段只做到「政策显式化 + 矩阵如实分层」（TASK-112/127），完整建模留 owner 裁决；矩阵 L1–L5 限制逐条列出未掩盖。
5. **性能结论只覆盖受测条件**：TASK-128 只保证「测量可信」，真机数字需 owner 按 `tools/phase4_checklist.md` 采集；20k/50k 场景对 article/social 文本布局无瓶颈的证据**不能**外推到长会话、真实媒体与同步并发（工具已能测，数据待采）。
6. **协议矩阵「指定服务端版本已验证」列目前只有 mock 替身**：FreshRSS/Miniflux 具体版本的实机验收仍未做（清单与矩阵均如实声明）。
7. **审查者观察（非 finding，供参考）**：`reconcile_*` 的失败传播只由 `apply_*` 层测试覆盖，reconcile 层落 `report.errors` 为**人工核对**；矩阵一格对 Fever 自动化覆盖的表述偏保守（低估而非夸大）。

---

## 11. 逐审计项的处置对照表（速查）

| 审计项 | 严重度 | 处置 | 任务 | 关键锚点 | 回归锚点 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| §1 可变集合 OFFSET | P1 | keyset 游标 + 复合索引 | 117/121 | `db/articles.rs` KEYSET_PREDICATE_* | `keyset_*` Rust ×3（18→21）；`t117-*` ×21 | 已验收 |
| §2 文章级共享版本 | P1 | `id\|field` 版本 + 统一回滚助手 | 118 | `internals.ts` entryVersionKey/rollbackEntryClaims | `t118-*` ×14 | 已验收 |
| §3 快照继承复活旧产物 | P2 | bodyById 实体缓存 + 失效 + LRU | 122 | `bodyCache.ts`、`selectors.ts:97` | `t122-*` ×31、`t122-9` 锁 | 已验收（含 P0 修复） |
| §4 迟到响应覆盖 | P2 | 查询/计数代际 + 本地写入序号 | 119 | `bootstrap.ts` queryGeneration、`nav.ts` reconcileCounts | `t119-*` ×21 | 已验收 |
| §5 保位窗口 | P2 | keyset 窗口重取 + 偏移 + 尾沿 | 123 | `fetchWindowRows`、`timelineAnchor.ts` | `t123-*` ×34 | 已验收 |
| §6 同步四态无事件 | P2 | 事件总线 + 构建失败分类 + 阻塞上报 | 124 | `push.rs` notify/record_push_block、`sync_queue.rs` payload | `t124-r1..r3`（Rust +3，CI）、`t124-*` ×15 | 已验收 |
| §7 性能证据不足 | P2 | 测量工具重写（判据/归属/RSS/深页/锁代理/条件） | 128 | `tools/phase4_measure.mjs`、`phase4_seed.py`、`phase4_checklist.md` | 烟雾跑通 + 审查对抗探针 | 已验收 |
| §8 矩阵混淆能力/实现 | P2 | 三列表头 + 版本声明 + 冲突层 Result | 127 | `docs/sync-compat-matrix.md:32`、`conflict_policy.rs` | `apply_*` 新测试 ×2（Rust 1→3，CI）、矩阵事实上游复核 | 已验收 |

---

## 12. 审计者可直接复核的反例构造（若怀疑修复无效）

| 怀疑点 | 反例构造（在 dev 工作树上做，勿提交） |
| --- | --- |
| keyset 分页仍会跳行 | 注释掉 `KEYSET_PREDICATE_DESC` 分支（回落 OFFSET）→ `cargo test keyset_continuation_tracks_mutable_unread_collection` 必红 |
| 跨字段版本失效 | 把 `entryVersionKey` 改回 `${id}` → `t118-*` 的收藏/读交叉用例必红 |
| 正文缓存复活 | 从 `mergeSnapshotEntries` 恢复 `a.aiSummary \|\| prev.aiSummary` → `t122-*` 清理失效用例必红 |
| 查询代际无效 | 删除 `queryGeneration` 落地判据 → `t119-*` 迟到响应用例必红 |
| `useShallow` 回归 | 去掉任一订阅的 `useShallow` → `t122-9` 必红（源级锁）；真实渲染另见下条 |
| 保位窗口退化 | 把 `fetchWindowRows` 退回单页 → `t123-1/5` 必红（条目数 1253 塌到 500） |
| 同步事件不生效 | 删 `App.tsx` 监听或改 payload 键名 → `t124-6` 必红 |
| 冲突写失败仍被吞 | 把 `apply_*` 改回 `unwrap_or(0)` → `apply_read_by_policy_propagates_db_write_failure` 必红 |
| 搜索计时仍可被订阅源抢先 | 把正例判定改回全部 `.cp-item` → 用 token「NASA/APOD」类订阅源名子串即可复现误命中（R1 对抗探针脚本在 `tmp/phase4/f1-probe*.js`） |
| 真实渲染崩溃（P0 类） | 起 Vite dev（`npm run dev -- --port 5199`）+ 无头 Chrome `--remote-debugging-port=9223 --user-data-dir=D:\tmp\chk`，用 `tools/t059_cdp.mjs` 点开一张卡，观察 `getSnapshot should be cached` 报错与 `#readerContainerCol` 是否渲染出正文 |

（本表所列「必红」均为实际执行过的变异/探针结论，材料在对应 RUN 的 review 报告与 `tmp/phase4/`、`tmp/refactor-20261005/`。）

---

## 13. 附录：改动文件清单（`git show --numstat`，供逐文件定位）

**b7947e9 · TASK-117（P1-1）**

| 文件 | 增/删 |
| --- | --- |
| `src-tauri/src/commands/articles.rs` | +21/-0 |
| `src-tauri/src/db/articles.rs` | +323/-11 |
| `src-tauri/src/db/dedup_tests.rs` | +2/-0 |
| `src-tauri/src/db/migrations.rs` | +16/-0 |
| `src/lib/api.ts` | +7/-0 |
| `src/store/internals.ts` | +60/-25 |
| `src/store/selectors.ts` | +15/-2 |
| `src/store/slices/bootstrap.ts` | +71/-35 |
| `src/store/slices/nav.ts` | +13/-9 |
| `src/store/types.ts` | +29/-7 |
| `tools/frontend-regression.mjs` | +423/-120 |

**f195aec · TASK-121（P1-1 延续）**

| 文件 | 增/删 |
| --- | --- |
| `src-tauri/src/db/articles.rs` | +4/-1 |
| `src-tauri/tests/ingestion_e2e.rs` | +5/-0 |

**feae9cd · TASK-118（P1-2）**

| 文件 | 增/删 |
| --- | --- |
| `src/store/internals.ts` | +110/-20 |
| `src/store/slices/bootstrap.ts` | +10/-2 |
| `src/store/slices/nav.ts` | +6/-3 |
| `src/store/slices/reader.ts` | +31/-14 |
| `tools/frontend-regression.mjs` | +182/-3 |

**7a3d0f4 · TASK-119（P2-4）**

| 文件 | 增/删 |
| --- | --- |
| `src/store/internals.ts` | +38/-0 |
| `src/store/slices/bootstrap.ts` | +108/-20 |
| `src/store/slices/nav.ts` | +34/-16 |
| `tools/frontend-regression.mjs` | +207/-3 |

**91059a3 · TASK-122（P2-3）**

| 文件 | 增/删 |
| --- | --- |
| `src/components/Reader.tsx` | +25/-16 |
| `src/components/Timeline.tsx` | +60/-41 |
| `src/components/settings/CacheCleanupSection.tsx` | +8/-0 |
| `src/lib/api.ts` | +4/-3 |
| `src/store.ts` | +7/-0 |
| `src/store/bodyCache.ts` | +346/-0 |
| `src/store/internals.ts` | +71/-60 |
| `src/store/selectors.ts` | +105/-11 |
| `src/store/slices/ai.ts` | +291/-283 |
| `src/store/slices/bootstrap.ts` | +25/-24 |
| `src/store/slices/nav.ts` | +22/-30 |
| `src/store/slices/reader.ts` | +135/-131 |
| `src/store/types.ts` | +9/-5 |
| `src/types.ts` | +9/-6 |
| `tools/frontend-regression.mjs` | +546/-184 |

**eba4df5 · TASK-122 R1（P0 修复）**

| 文件 | 增/删 |
| --- | --- |
| `src/components/Reader.tsx` | +9/-2 |
| `src/components/Timeline.tsx` | +10/-4 |
| `tools/frontend-regression.mjs` | +92/-0 |

**4f040fc · TASK-123（P2-5）**

| 文件 | 增/删 |
| --- | --- |
| `src/components/Timeline.tsx` | +64/-5 |
| `src/components/timelineAnchor.ts` | +63/-15 |
| `src/store/slices/bootstrap.ts` | +175/-11 |
| `src/store/slices/nav.ts` | +16/-6 |
| `tools/frontend-regression.mjs` | +297/-2 |

**2f38ff2 · TASK-124（P2-6）**

| 文件 | 增/删 |
| --- | --- |
| `src-tauri/src/commands/articles.rs` | +11/-0 |
| `src-tauri/src/db.rs` | +3/-2 |
| `src-tauri/src/db/sync_queue.rs` | +166/-9 |
| `src-tauri/src/lib.rs` | +5/-0 |
| `src-tauri/src/sync/credentials.rs` | +22/-6 |
| `src-tauri/src/sync/phases.rs` | +45/-15 |
| `src-tauri/src/sync/push.rs` | +174/-10 |
| `src-tauri/src/sync/subscriptions.rs` | +12/-5 |
| `src/App.tsx` | +26/-0 |
| `src/components/settings/SyncTab.tsx` | +13/-0 |
| `src/store/slices/bootstrap.ts` | +4/-2 |
| `src/store/slices/sync.ts` | +19/-0 |
| `src/store/types.ts` | +10/-1 |
| `tools/frontend-regression.mjs` | +131/-0 |

**8a4ca5e · TASK-124 R2（CI 修复）**

| 文件 | 增/删 |
| --- | --- |
| `src-tauri/src/sync/push.rs` | +2/-0 |

**0cc84e6 · TASK-127（P2-8）**

| 文件 | 增/删 |
| --- | --- |
| `docs/sync-compat-matrix.md` | +54/-23 |
| `src-tauri/src/fever.rs` | +18/-0 |
| `src-tauri/src/sync/conflict_policy.rs` | +93/-11 |
| `src-tauri/src/sync/fever_pull.rs` | +30/-10 |
| `src-tauri/src/sync/greader_pull.rs` | +26/-8 |

**2275f02 · TASK-128（P2-7）**

| 文件 | 增/删 |
| --- | --- |
| `tools/phase4_checklist.md` | +139/-0 |
| `tools/phase4_measure.mjs` | +1096/-81 |
| `tools/phase4_seed.py` | +312/-27 |

**b7c3ed8 · TASK-128 R1**

| 文件 | 增/删 |
| --- | --- |
| `tools/phase4_checklist.md` | +59/-21 |
| `tools/phase4_measure.mjs` | +366/-80 |
| `tools/phase4_seed.py` | +72/-3 |
