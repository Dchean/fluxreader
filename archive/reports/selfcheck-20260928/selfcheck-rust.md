# Rust 后端发布前自检报告（读码审查）

- 审查范围：`src-tauri/src/**`（commands / db / sync / ingestion / scheduler / credentials / error / lib.rs 等，约 15,100 行）+ `src-tauri/tests/` 覆盖缺口 + `src/lib/api.ts`、`src/types.ts` 契约对表。
- 方法：lib.rs invoke_handler 全量 56 命令清单 → 逐个找实现与前端调用点 → grep 特征（`unimplemented!|todo!|unreachable!|let _ =|TODO|FIXME|stub`）逐条判定 → sync/ingestion/scheduler/db 主链路通读 → 与 README 功能声明对表（docs/FEATURES.md 已不在工作树，仅存 git 历史，见 §6）。
- 门禁状态：cargo 228 通过 / 0 失败 / 9 忽略（本次为纯读码审查，未重跑测试）。

## 结论摘要

**P0 = 0，P1 = 1，P2 = 1，P3 = 11。** 无功能空壳、无假实现；sync push/pull 链路完整、游标守卫严密；锁纪律（db 锁从不跨 .await）逐点核实无违例；迁移追加式且回填事务化。唯一 P1 是与前端自检（tmp/selfcheck-frontend.md）同一根因的后端侧缺口：`add_feed` 对 `folder_id` 无存在性校验，`Some(0)` 直通外键违约，`None → 未分类` 兜底从当前 UI 路径不可达。

---

## 1. 发现清单

### P1（应修：用户可感知的核心流程断裂）

**[P1-1] add_feed 不校验 folder_id 存在性，`Some(0)` 直通 FK 违约；`None → 未分类` 兜底从 UI 不可达**
- 位置：`src-tauri/src/commands/folders.rs:112-123`（命令签名）、`:187-190`（`persist_new_feed` 内 `Some(fid) => fid, None => ensure_uncategorized_folder(conn)?`）；对照 `record_feed_edit`（`commands/folders.rs:455-459`）对 `folder_id` 有 `folder_exists` 校验，`add_feed` 没有。
- 依据：前端唯一调用点 `src/store/slices/feeds.ts:103-108` 把分类 id 机械换算 `Number(catId.replace('cat-',''))`——ContextMenu.tsx:242 / Overlays.tsx:138 以 `openAddFeedModal('')` 打开弹窗、Sidebar.tsx:139 在无分类时传 `''`，`Number('') = 0` → 后端收到 `folder_id: Some(0)`。`folders.id` 是 INTEGER PRIMARY KEY 从 1 起，0 永不存在；`PRAGMA foreign_keys=ON`（`db/migrations.rs:299`）下 `insert_feed`（`db/feeds.rs:88-106`）必然 FK 违约，用户看到裸的「FOREIGN KEY constraint failed」，首次安装无分类时添加订阅（核心首用路径）确定失败。后端的 `ensure_uncategorized_folder` 兜底（`db/sync_map.rs:267-279`，语义本身正确）因前端从不传 null 而不可达。
- 建议：后端补一道与 `update_feed` 同源的防线——`Some(fid)` 先 `db::folder_exists` 校验（不存在时报「目标分类不存在」），可选把 `Some(0)`/`Some(n<=0)` 归一为 `None` 走未分类兜底；前端侧修复（映射 null）由前端自检轨道跟进，两侧任一修掉即可解锁该流程，建议两侧都修（后端防线同时挡住 UI 传陈旧分类 id 的场景）。

### P2（宜修：明确 bug，触发条件依赖服务端行为）

**[P2-1] greader 轻量对账的权威集合分页中断被静默吞掉，可能用「截断的 starred 集合」清掉用户收藏**
- 位置：`src-tauri/src/sync/greader_pull.rs:202-222`（`fetch_stream_ids`），消费点 `:164-182`（reconcile）+ `:249-255`（`starred_set` 不含 → `sync_mark_unstarred_if_starred`）。
- 依据：`fetch_stream_ids` 的续页处理是 `r.continuation.and_then(|c| c.parse::<u64>().ok())` → `Some(c) if got > 0 => 继续，_ => break`。两类中断都不产生错误：① continuation 非数字（服务端异常/中间层改写）；② 返回了 continuation 但本页 0 条。主列举循环 `pull_entries_greader` 对完全相同的两种形态**显式计数 id_failures 并放弃本轮游标**（`:102-123`，TASK-069 F1 的修复），`fetch_stream_ids` 没有同等守卫。截断的 `starred_set` 进入 `reconcile_reader_state` 后，凡在 `mf_id_to_article` 中、但排在截断点之后的已收藏条目会被**本地取消收藏**（数据影响方向为丢失），且游标/错误双无痕迹。同一函数对 read 集合只做 read-wins（`:244-248`），无反向风险。
- 建议：把 `fetch_stream_ids` 对齐主循环的 F1 语义——解析失败/continuation-but-empty 时返回 Err，让外层 `(Err(e), _) | (_, Err(e))` 分支跳过本轮对账（既有 C-1 守卫直接复用，改动约 5 行）。

### P3（备忘 / 宜顺手修）

**[P3-1] persist_new_feed / import_feeds 多语句无事务，中途失败留「半套」数据**
- `src-tauri/src/commands/folders.rs:191-219`（insert_feed → 清墓碑 → 写抓取状态 → 逐条 upsert 文章）、`src-tauri/src/commands/opml.rs:70-89`。任一条 upsert 失败时 feed 已存在而文章残缺（重试会被「该订阅地址已存在」判重拦住，用户需先删再加）。同库 `mark_all_read_with_enqueue`（`db/articles.rs:855-881`）已示范 `unchecked_transaction` 用法。建议包事务。

**[P3-2] folder_name 把查询错误压成 None，与「目录不存在」不可区分**
- `src-tauri/src/db/folders.rs:97-101`：`.query_row(...).ok()`。push_feeds 用它解析队列 payload 的目标分类名（`sync/subscriptions.rs:88-93`）——DB 抖动会静默降级为「订阅挂远端默认分类」。建议区分 `Ok(None)` 与 `Err` 上抛。

**[P3-3] cleanup_cache 的 ai 分支 published_at 未过 `datetime()` 归一，与 articles 分支口径不一致**
- `src-tauri/src/db/articles.rs:504-511`：`AND published_at < {cutoff}`（cutoff 是 `datetime('now','-N days')` 产出的 UTC 'YYYY-MM-DD HH:MM:SS'）；articles 分支（`:488-497`）已按 P3[5] 修复为 `datetime(published_at) < {cutoff}`。RFC3339 带 offset 的行在 cutoff 当天会因 `'T' > ' '` 的字符串比较漏清。后果仅限 AI 缓存清理边界差几小时。建议同口径补 `datetime()`。

**[P3-4] list_articles 的 limit 未夹取，负数在 SQLite 表示「不限制」**
- `src-tauri/src/db/articles.rs:182-184`（`LIMIT ? OFFSET ?` 直绑）。同类问题在 `search_articles` 已修（`:367`，P3[8]）。参数只来自前端（`bootstrap.ts` 传 ARTICLES_PAGE_SIZE），非用户输入，风险低；建议同样 `limit.max(0)` 或上限收敛。

**[P3-5] sync/entries.rs 三处 `let _ =` 吞错无日志（同文件其余处已按 P3[1] 改 warn）**
- `src-tauri/src/sync/entries.rs:150`（跨源副本 `sync_mark_read_if_unread`）、`:235`（upsert 后 `set_article_remote_id`）、`:239`（`sync_set_article_status`）。均可自愈（下一轮对账经 maps 重合），但失败现场无线索，与同文件 `merge_remote_status` 的 warn 纪律不一致。

**[P3-6] config_sync apply_payload 三处 `let _ =` 吞错，与模块自身 N6 事务纪律不一致**
- `src-tauri/src/config_sync.rs:157-158、168`：`update_folder_layout` / `set_folder_ai_flags` 失败被吞、事务照常提交其余字段。这些是简单 UPDATE，实际失败概率低；建议改 `?` 让整包回滚（事务框架已在）。

**[P3-7] 升级库的 legacy 时间格式混排：同日排序错位 + 前端 parseTs 时区偏移**
- v12/v14 回填（`db/migrations.rs:206-212、:260-262`）给 NULL/'' 行写入 SQLite 格式 `YYYY-MM-DD HH:MM:SS`（UTC），新写入是 RFC3339（`ingestion/parse.rs:91-97`、`sync/entries.rs:38-46`）。`ORDER BY published_at` 是字符串比较：同日内 `'T' > ' '` 使 RFC3339 行恒排在 legacy 行之后（无视真实时刻）；且 `src/lib/api.ts:555-559` 的 `Date.parse('2025-01-01 00:00:01')` 按本地时区解析 → legacy 行显示偏移 +08:00。影响面：仅升级库中「源不带发布时间被回填」的少数行。建议：回填时写 RFC3339 或前端 parseTs 兼容空格格式按 UTC 解析。

**[P3-8] 文档/注释与实现漂移（两处）**
- `src/lib/api.ts:253` 仍写「FTS5 全文搜索」，实际搜索入口是 LIKE 子串（`db/articles.rs:307-318` 明确 FTS5 已非搜索入口、仅历史遗留）；README.md:18「允许同步服务地址、模型等非敏感配置」——config_sync 白名单不含 ai_config（含 key 整体排除，`config_sync.rs:6`），模型名实际不同步。建议改注释/README 措辞。

**[P3-9] 流式翻译预览未消毒直达前端（后端注释已明示此权衡），依赖 CSP 兜底**
- `src-tauri/src/commands/ai.rs:226-233`：落库前 sanitize，流中 delta 不消毒。`tauri.conf.json` CSP `script-src 'self'`（default-src 回退）使 inline handler 不可执行、`<script>` 经 innerHTML 插入也不执行，实际风险被压住；作为纵深建议在流式侧同样过 sanitize（成本极低）。非发布阻塞。

**[P3-10] get_setting 对敏感键解密后可经通用 IPC 明文出 webview（备忘）**
- `src-tauri/src/db/settings.rs:4-20` + 命令 `commands/settings.rs:13-17`。当前前端不读 `greader_password`（密码框恒空、留空复用语义在 `commands/sync.rs:56-81`），无实际暴露路径；DPAPI 威胁模型本就是「DB 文件被拷走」而非 webview 边界。记录为边界说明，不改。

**[P3-11] tests/ 覆盖缺口清单（只列缺口，不评质量）**
- 无任何集成/单测触达：`fetch_image`（Referer 候选链仅 commands/settings.rs 内纯函数单测）、`media_update_full` / `media_stop`（Windows 专属，可接受）、`resolve_close`（含 close-ask 10s 兜底路径）、`sync_status`、`sync_disconnect`、`set_read_bulk` 命令壳（核心逻辑 `apply_read_bulk` 有变异验证单测）、`article_index` 命令壳（SQL 有 EXPLAIN 单测）、`opml_import/opml_export` 命令壳（`import_feeds`/parse/build 均有单测）。
- 已覆盖良好（抽查确认）：sync push/pull 全链（sync_e2e / sync_phases_e2e / pull_cursor / pull_window / sync_gap_repro / dual_client / dedup_sync）、迁移中断修复（migration_test + req108 五连）、账号生命周期/换账号清理（account_lifecycle_e2e 含 cache_cleanup）、端点自动适配、GitHub 设备流、config_sync、封面补全/失效上报（cover_backfill_e2e）。

---

## 2. 空壳盘点表（lib.rs invoke_handler 全量 56 命令）

逐一核对结果：**全部为真实实现，无桩/无假成功/无 `unimplemented!`/`todo!`（grep 仅命中 sanitize.rs 的静态选择器 `unreachable!`，不可达且无害）；全部 56 个命令在 src/ 中存在 invoke 调用点（含经 api.ts 门面的多行链式调用），无「声明了但没接线」。** 按域列实现要点与前端调用点：

| 域 | 命令 | 实现状态 | 前端调用点 |
|---|---|---|---|
| 分类 | list_folders / create_folder / rename_folder / delete_folder / update_folder_layout / set_folder_collapsed / set_folder_ai_flags | 真实（rename/delete 带墓碑 A-4） | api.ts → Sidebar/FeedsTab/feeds.ts |
| 订阅源 | list_feeds / add_feed / delete_feed / update_feed / update_feed_layout / set_feed_ai_flags | 真实（add_feed 先抓取验证再入库；delete/update 带远端 best-effort 推送） | api.ts → feeds.ts / FeedsTab |
| 条目 | list_articles / article_index / get_article / get_articles / report_broken_cover / search_articles / set_read / set_read_bulk / set_starred / mark_all_read / feed_counts | 真实（mark_all_read 集合化事务 M-9；set_read_bulk 与逐条等价性有变异锁定） | api.ts → bootstrap/reader/Timeline/Overlays/coverImage |
| 刷新 | refresh_feed / refresh_all_feeds | 真实（三段式 staged 管线） | api.ts → feeds.ts / 托盘 |
| 设置 | get_setting / set_setting | 真实（敏感键 DPAPI 加解密收口在 db 层） | api.ts → settings/* |
| 同步 | sync_test / sync_save / sync_phase / sync_disconnect / sync_local_feeds / sync_status | 真实（换账号清理、首连判定、端点解析缓存落库） | api.ts → SyncTab |
| 缓存 | cache_cleanup | 真实（双 scope，校验 1–3650） | api.ts → 设置 |
| 关闭 | resolve_close | 真实（close-resolved 事件回执） | api.ts → App |
| AI | save_ai_config / get_ai_config / ai_list_models / ai_summarize / ai_translate | 真实（SSE 流式 + Channel；TASK-070 已删空壳 Error 变体） | api.ts → AiTab/reader |
| 全文 | extract_fulltext | 真实（结构化 degraded 结果） | api.ts → reader.ts |
| 图片 | fetch_image | 真实（Referer 候选链 + 25MiB 上限） | api.ts → imageProxy/coverImage |
| OPML | opml_import / opml_export | 真实（规范化 URL 判重 + 墓碑清理） | api.ts → 设置 |
| SMTC | media_update_full / media_stop | 真实（mpsc → SMTC 专用线程） | api.ts → PlayerBar |
| 配置同步 | config_sync_save_credentials / upload / download / apply / status | 真实（Gist/WebDAV；白名单 + 字段级合并删除） | api.ts → ConfigSyncSection |
| GitHub 登录 | github_login_start / poll / status / disconnect | 真实（RFC 8628 设备流；内存态 device_code） | api.ts → ConfigSyncSection |

曾被发现后已确认修复的空壳类问题（本次复核为已闭环）：TASK-070 删除 `AiEvent::Error` 变体；TASK-070 删除 feeds_fetch_failed 死代码查询；TASK-056 系列修复 pull 静默吞错（建目录/建源/建条目失败均进 report.errors）。

---

## 3. IPC 契约对表结果（56 命令 × 参数名/类型/Option 语义 × 返回行类型）

**总体结论：契约一致，无「前端传了后端收不到」或「前端依赖但后端缺落」的字段。** 逐项核对要点：

1. **参数名 camelCase→snake_case**：Tauri 2 默认映射已逐个核对——`add_feed{feedUrl,title,folderId,layout,autoSummary,autoTranslate,syncToBackend}`、`mark_all_read{feedId,folderId,starredOnly,sinceMs,layout}`、`article_index{args,articleId}`、`report_broken_cover{articleId,url}`、`extract_fulltext{articleId}`、`fetch_image{url,pageUrl}`、`media_update_full{durationSec,positionSec}`、`set_folder_ai_flags{summary,translate}`（前端显式换名正确）等，全部对上。
2. **args 结构体**：`ArticleListArgs` 无 `rename_all`，按 snake_case 反序列化，与 api.ts 的 `ArticleListArgs` 接口逐字段一致，并有反序列化单测锁定（`commands/articles.rs:451-482`）。
3. **Option 语义**：`update_feed` 的 undefined 字段=不改（COALESCE 语义，`db/feeds.rs:200-229`）；`mark_all_read` 的 `sinceMs: null` → None；`github_login_start` 的 `{force}` undefined → None；`sync_phase{full}` 传 false → Some(false)。均正确。唯一语义缺口即 P1-1（`folderId: 0` vs None）。
4. **返回行类型**：`FolderRow`/`FeedRow`/`ArticleListItem`/`ArticleRow`/`FeedCounts`/`RefreshSummary`/`SyncReport`/`SyncStatusInfo`/`OpmlImportReport`/`ApplyOutcome` 均为 snake_case 直出，与 api.ts 镜像接口一致（`FolderRow.position` 为前端未消费的冗余字段，无害）；`ExtractOutcome`/`SyncSaveResult`（手工 JSON 串）/`DeviceLoginStart` 为 camelCase，与前端类型一致；`AiEvent{type,data}` 与 Channel 解析一致。
5. **`list_articles.with_content=false` 时 url/content_html 等四列为 NULL/0**：前端类型已标 optional（`ArticleListItemRow.url?` 等），消费侧 `articleRowToEntry` 做 `?? undefined` 兜底——一致。

---

## 4. 专项核对（按任务指定的重点）

1. **add_feed folder_id 语义**：`None → ensure_uncategorized_folder`（不存在则建，创建失败上抛，无硬编码 1）语义正确（`commands/folders.rs:185-190`）；缺口是 `Some(fid)` 直通无校验（P1-1）。与前端「folderId=0」P1 同根因，后端补防线即可双向闭环。
2. **滚动标读后端路径**（scrollDrivenRef 前端问题对应的后端）：`set_read_bulk` → `apply_read_bulk` → 逐 id `record_read_state`（本地写 + enqueue，A-5 无条件入队），与逐条路径语义等价且有变异锁定单测；整批一次持锁、逐条自动提交为**文档化的有意取舍**（`commands/articles.rs:158-183`）。未发现后端侧类似边界问题；WAL + synchronous=NORMAL 下批量语句成本可接受。push 侧防抖合批（STATE_PUSH_FLYING/PENDING + Drop 守卫）无重入/永久静默风险。
3. **sync push/pull 链路完整性**：队列四类动作全部有消费者——read/unread/star/unstar → `plan_push`（states 阶段前后各一次，绑定回填后补推）；add_feed → `push_feeds`（quick_add + 分类补挂 + 碰撞绑定）；remove_feed 僵尸 → `purge_remove_feed_zombies`；无绑定状态项 30 天老化（`prune_stale_unbound`）。pull 侧：分类墓碑/订阅墓碑双向收敛、远端退订对账（只删 origin='remote' 且已绑定、pending 双路径保护、不写墓碑）、C-1（失败≠空集合）与 F1（分页中断计入守卫）守卫齐全。**未发现「只入队不消费」「只本地不回传」类半截语义。**
4. **增量游标/去重边界**：greader 起点候选游标（TASK-097）+ 双失败计数守卫推证成立；fever `last_sync_entry_id` 无条件写安全（只反映已合并条目，失败页不入集合，下一轮 since_id 恰好补位）；时间戳游标仅在窗口拿全时推进（防切协议跳窗）。去重：feed 内 guid → 同 feed url_norm → 智能去重跨 feed url_norm + 墓碑三层，开关关闭清墓碑语义闭环。
5. **锁纪律 / 并发**：全部 `db.lock().await` 均为短临界区（HTTP 一律锁外；`build_client`/`push_states_now`/phases 逐点核实）；PUSH_LOCK 只在「先 PUSH_LOCK 后 db 锁」方向获取，无反向路径，无死锁；plan_push 与 push_feeds 处理的队列行集合不相交（article 级 vs add_feed 级），prune 竞态已由 PUSH_LOCK 串行化评述并成立。
6. **迁移对存量库**：追加式，无 schema 破坏；v11 的去重删除为文档化的定向清理；v13 RENAME COLUMN 无损；M-14 回填「回填+落标同事务、失败可重入」并有用例覆盖。**无破坏性风险。**
7. **DPAPI/凭据边界**：加密失败回退明文（warn）；解密失败返回密文原文（warn）——后果是该轮同步认证失败并留日志，可接受降级；非 Windows 跳过明文迁移保持幂等；设备流 token 内存态不落库；config_sync 上传白名单排除了全部四个敏感键（实测字段清单核对无遗漏）。

---

## 5. grep 特征逐条判定（可达性/危害）

- `unreachable!`×1（sanitize.rs:243，静态合法选择器）→ 无害。
- `TODO/FIXME/XXX/stub/unimplemented!/todo!` → 生产代码 0 命中（命中项均为测试/注释中引用历史缺陷编号）。
- `let _ =` 生产路径共 30+ 处：事件 emit / 窗口操作 / LocalFree / Channel send 等纯通知类 22 处 → 合理；有状态影响的 8 处已逐条列入 P3-5/P3-6（entries.rs×3、config_sync.rs×3）及正文复核（greader_pull.rs:192 游标写失败=下一轮重拉、settings.rs:31 清墓碑失败=墓碑继续拦截，均可自愈，不单列）。

## 6. docs/FEATURES.md 对表说明

`docs/FEATURES.md` 已不在工作树（README.md:18 注明「在 git 历史中」），无法逐条对表；以 README.md「功能范围」为准：直连抓取、全文提取、OPML、音视频播放（enclosure/duration/SMTC）、桌面集成（托盘/自启/单实例/通知）、搜索与快捷操作、图片与富媒体（代理/防盗链 Referer 链）、个性化、缓存/去重、GReader/Fever 同步、AI 摘要翻译、Gist/WebDAV 配置同步（凭据排除）——**后端能力逐条均有对应实现（见 §2 表），无「声明了但后端不存在」的能力**；仅 P3-8 的两处措辞漂移。
