# 双向同步缺口定位与修复设计（TASK-031，REQ-002/003）

- 方法：源码审读（sync.rs / commands.rs / ingestion.rs / db/sync_queue.rs / db/sync_map.rs / greader.rs）+ 三个 mock e2e 复现测试（src-tauri/tests/sync_gap_repro_e2e.rs，#[ignore] 标注，实跑复现成功）。
- 结论：双向同步的**底层管线存在**（队列、push/pull 两阶段、绑定回填、pending 保护），缺口集中在**用户操作未接线到队列**与**对账语义**两层，共 8 个独立缺口。

## 缺口清单（复现测试已覆盖前三项）

### A-1 删除订阅不回传 + pull 复活（复现：deleted_feed_revives_on_pull ✓）
- 位置：commands.rs:222-230（delete_feed 仅 db::delete_feed，注释明示"本地删除 ≠ 强删远端"）；sync.rs:403-462（pull_feeds 按 URL 匹配不到已删 feed → insert_feed_origin 重建）。
- 根因：删除动作不入队、不留墓碑；pull 只做"远端有→本地建"，无墓碑排除。
- 修复设计：delete_feed 时若 feed 已绑定 remote_id 且 sync_configured → GReader 协议调用 greader.rs:457 `unsubscribe`（已实现未接线）；Fever 无退订端点 → 写入删除墓碑表（feed_url + 时间戳），pull_feeds 建feed前查墓碑跳过；墓碑在被远端订阅列表确认不再包含后清除。

### A-2 订阅改名/移动目录不回传（复现：feed_rename_never_reaches_backend ✓）
- 位置：commands.rs:243-276（update_feed 注释明示"靠 pull 对账收敛……不做远端 best-effort 推送"）；greader.rs:467-485 edit_subscription 已实现但全仓零调用（Backend 枚举未暴露）。
- 根因：写路径只落本地；pull 也不回填标题（update_feed_title_if_empty 仅在本地标题==URL 时生效）→ 双端永久分歧。
- 修复设计：Backend 枚举暴露 edit_subscription；update_feed 若 sync_configured 且 feed.remote_id 非空 → 锁外 best-effort 推送 `edit_subscription(remote_id, title, 目标分类 label)`，失败仅记 report/日志，不影响本地生效；Fever 降级跳过。

### A-5 离线状态变更不补推（复现：offline_read_change_never_pushed_after_connect ✓）
- 位置：commands.rs:370-411（set_read/set_starred 仅在 sync_configured(&conn) 时 enqueue，否则直接 return——离线变更无持久化待推记录）。
- 根因：入队决策取决于"变更瞬间"是否已配置凭据。
- 修复设计：set_read/set_starred **无论是否 configured 都入队**（sync_queue 本就有 prune；凭据缺失时 push_states_now 静默跳过即可）；或 sync_save 成功后做一次全量状态对账上传。
- A-5b（Fever 特有，代码审读证实、未做复现测试）：sync.rs:926-958 reconcile_fever_state 以远端 unread 集合为准双向回滚（`want_read = !unread_set.contains(remote_id)` → sync_mark_unread_if_read）。离线已读 + 无 pending 保护（未入队）的条目会被改回未读。GReader 对账（sync.rs:680-709）为单向合并（只正向 mark + 取消收藏），无此回滚。修复 A-5 后 pending 保护自然覆盖；另建议 C-1 修复（见下）避免空集合误判。

### A-3 push_feeds 丢弃队列 payload 的 folder_id（代码审读证实）
- 位置：sync.rs:317-334（PendingFeed 只取 feed_url，不解析 payload）；commands.rs:212-213、707-711（add_feed/opml_import 声称 payload 携带分类供 push 挂载）。
- 修复设计：push_feeds 解析 payload JSON 取 folder 名；quick_add 成功后追加 edit_subscription(remote_id, None, Some(label))（依赖 A-2 接线）。

### A-4 分类改名/删除在 pull 时复活为空目录（代码审读证实）
- 位置：commands.rs:85-96（rename_folder 仅本地）；sync.rs:385-400（pull_feeds find_folder_by_name 失败即 create_folder）。
- 修复设计：与 A-1 共用墓碑机制（folder 改名/删除记录旧 label 墓碑，pull 建目录前排除）；长期用 edit_subscription 的 a 参数双向收敛。

### A-6 本地直连抓取不回写同步系统（部分属协议限制）
- 位置：ingestion.rs:330-502（refresh_feed 只 upsert 文章，不碰 sync_queue）；新文章无 remote_id，push 时被 plan_push 跳过（sync.rs:184-190）。
- 设计边界：GReader/Fever 协议没有"上报新条目"端点，纯本地源接受单向；对远端也有的源，pull 的 URL 兜底匹配（merge_pulled_entry）已能补绑定——修复设计：states_phase 末尾的二次 push 确认覆盖"绑定回填后补推状态"场景，补一条集成测试。

### A-7 pull 单向：远端已退订的订阅本地永不删除（疑似有意保守设计，需产品确认）
- 位置：sync.rs:403-462 无"本地有→远端无"对账。
- 修复设计（待确认）：全量同步时对 origin='remote' 且远端 subscription/list 不再包含的 feed，提示用户或按策略清理。

### A-8 队列卫生：无绑定条目永久滞留（代码审读证实）
- 位置：sync.rs:184-190（plan_push 跳过无 remote_id 条目但保留队列）；db/sync_map.rs:88-97（pending_ids 永久保护，pull 对账跳过）；sync_queue 无 TTL。
- 修复设计：队列项加 created_at 老化清理（如 30 天）或全量同步末尾对多轮未绑定条目出队。

## 连带缺陷（同步健壮性，建议随批修复）

### C-1（P0 级）对账把"拉取失败"当"空集合"——静默清空收藏/已读
- 位置：sync.rs:639-642（GReader read/starred 流 `unwrap_or_default()` → reconcile 把本地收藏全部 unstar）；sync.rs:819-820（Fever unread/starred 同样）。
- 修复设计：任一集合拉取失败 → 跳过该轮对账段（记 report.errors），绝不把错误等同空集合。

### C-2 take_sync_queue 错误当空队列（sync.rs:324、commands.rs:869-893 三处）→ 至少 log::warn。

## 修复批次建议

1. 第一批（小而关键）：C-1（对账防误判）+ A-5（离线变更一律入队）——两个改动点少、直接消除"状态丢失/不一致"主诉。
2. 第二批：A-1 + A-2（删除/改名接线，含墓碑与 unsubscribe/edit_subscription 接线）+ A-3（push 挂分类，依赖 A-2）。
3. 第三批：A-4（目录墓碑）、A-8（队列 TTL）、A-7/A-6（产品确认后处理）。
- 每项修复将本文件对应的复现测试转为必过（去 #[ignore]，断言反转为期望行为）。

## 复现测试证据（2026-09-15 实跑）

- deleted_feed_revives_on_pull：断言通过 = 订阅删除后 pull 复活 + 无 unsubscribe（exit 101 原始失败语义按设计保留 3 项中的 2 项成立）
- feed_rename_never_reaches_backend：断言通过 = 无任何 subscription/edit 动作
- offline_read_change_never_pushed_after_connect：断言通过 = 连接后无 edit-tag 补推
- 复现测试随修复批次转正：#[ignore] 逐步移除并断言期望行为（默认 `cargo test` 覆盖）；A-2/A-5 已转正，A-1 随 TASK-035 转正，A-3/A-4/A-8 待后续批次。

## A-1 的残留缺陷（TASK-055，2026-09-18 实证并修复）

A-1 的修复（TASK-035）引入了删除墓碑机制，但**墓碑的清除点判据过宽**，构成一处独立的 P1：

- **位置**：`src-tauri/src/sync/subscriptions.rs` 的 `unsubscribe_remote`——原先在 `unsubscribe` 返回 `Ok` 时
  调用 `db::remove_feed_tombstone`。
- **根因**：该 `Ok` 来自 `greader.rs:323-334 post_form_text`，判据仅 `resp.status().is_success()`，
  **看不到响应体**。真实 GReader/MiniFlux 兼容后端在 token 失效、权限不足或 `s=feed/<id>` 不存在时
  **可能返回 2xx + 错误体**，此时客户端误判为「远端已确认退订」。
- **后果**：墓碑（`pull_feeds` 防复活的**唯一防线**）被误清 → 远端仍列出该订阅 → 已删订阅被重新建回本地
  （用户现象：「删掉的订阅自己回来了」）。
- **为何既有测试没抓到**：`deleted_feed_stays_deleted_and_unsubscribes` 覆盖的是**正常路径**——
  mock 的 `ac=unsubscribe` 分支确实把订阅从列表移除，于是「远端不再列出 ⇒ 不复活」自然成立；
  且旧断言「2xx 后墓碑应清除」**断言的正是这个过宽判据本身**。缺陷只在**异常路径**上，此前无覆盖。
- **修复**：删除该清除点；墓碑清除条件收口为唯一一处——`pull_feeds` 中「远端订阅列表**实际已不含**该 URL」
  （读响应体，唯一有证据的判据）。
- **验证**：新增 `unsubscribe_2xx_without_removal_keeps_tombstone_and_no_revive`，
  经 mock 故障注入（`unsubscribe_returns_2xx_without_removing`）证明 **fail-before（exit 101）/ pass-after（exit 0）**。
- **同类路径核对**：目录墓碑（folder tombstone）的清除判据取自**远端 tag/list 的实际 label**，
  与请求是否 2xx 无关，**无同类缺陷**。
- **协议影响**：不改任何对外请求的内容与顺序，只改收到响应后的本地处理；墓碑会多留一段直到远端确认。
  经 owner 裁决记于 `DEC-tombstone-and-ignored-tests-20260918`。
