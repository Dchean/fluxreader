<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-064 · 后端 P2 缺陷修复四项：手动全刷去重、重加清墓碑、今天视图时区、配置同步事务（REQ-102/N3~N6）

**状态**：verified

**目标**：AUDIT-20260919-v2.md 的 N3/N4/N5/N6（REQ-102，均为后端数据正确性缺陷）逐项修复，每项配「修前失败⇒修后通过」的成对验证：

【N3 · 手动「刷新全部」忽略 smartDedup】scheduler.rs:104——refresh_all 走 due_filter=None 路径，dedup 被 unwrap_or(false) 写死；用户开智能去重后点侧栏「刷新」/托盘「刷新全部」会放行跨源同文（文章翻倍且不可逆），与单源 refresh_feed（正确读开关）自相矛盾。修复：把 dedup 从 due_filter 元组中拆出为独立参数——refresh_feeds_inner_with_concurrency 签名改为 (db, http, due_filter: Option<i64>, dedup: bool, concurrency)；refresh_all 读 read_refresh_config 的 dedup 传入（手动语义不变：仍抓全部源、仍忽略到期时间、同步模式过滤不变）；refresh_due_feeds 传 Some(interval_min), dedup。

【N4 · 已删订阅重新添加后墓碑永不清除】commands/folders.rs 的 add_feed 与 commands/opml.rs 的 import_feeds（TASK-062 抽出的纯函数）插入成功后都不调 remove_feed_tombstone，而全库只有 pull_feeds 的「远端不再列出」分支会清墓碑——重新添加的 URL 被墓碑永久压制：pull 跳过绑定（不回填 remote_id/标题）、plan_push 跳过未绑定文章、30 天后 prune_stale_unbound 物理删除其队列项（TASK-055 防复活防线在「用户改变主意」反场景成了陷阱）。修复：两处插入成功后调 db::remove_feed_tombstone（重新添加本身即用户改变主意的最强证据，不存在复活误判面；remove_feed_tombstone 内部按 normalize_url 匹配）。为使 add_feed 路径可测，把 add_feed 抓取验证后的入库段抽为纯函数 persist_new_feed(conn, feed_url, &parsed, etag, last_modified, folder_id, layout, auto_summary, auto_translate, sync_to_backend)（除新增清墓碑行外逐字纯搬运，TASK-062 同款配方）。

【N5 ·「今天」视图/今日计数时区错位】db/articles.rs:195 与 :692——date(a.published_at) 对带 offset 的 RFC3339 值归一到 UTC，与 date('now','localtime')（本地日期）比较，非 UTC 时区用户本地 00:00-08:00 发布的文章不进「今天」视图、today 计数少 1。修复：两处统一为 date(a.published_at, 'localtime') = date('now', 'localtime')。附带说明：同区域 ORDER BY COALESCE(published_at, fetched_at) 的字符串比较混合格式排序错乱问题（体检 N5 附带项）不在本任务（涉及查询语义更广，另行评估）。

【N6 · config_sync_apply 无事务】config_sync.rs apply_payload 全程无事务，中途失败（典型：folder 兜底 create_folder(...).unwrap_or(0) 产生 id=0 → insert_feed 外键违约上抛）会留下半套已应用配置（已建 folders、已插 feeds），重试叠加。修复：apply_payload 以 conn.unchecked_transaction() 包裹（apply_payload 签名 &Connection 不变，Transaction 经 Deref 使用；成功 commit、Err 路径 drop 自动回滚），两处 create_folder(...).unwrap_or(0) 改为 ? 上抛；update_folder_layout/set_folder_ai_flags 的 let _ = 吞错保持（可选标志更新失败不阻断 apply，事务保证一致性）。

**依赖**：TASK-063
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/scheduler.rs, src-tauri/src/commands/folders.rs, src-tauri/src/commands/opml.rs, src-tauri/src/db/articles.rs, src-tauri/src/config_sync.rs, src-tauri/src/db/articles_tests.rs, src-tauri/tests/refresh_dedup_e2e.rs, src-tauri/tests/config_sync_e2e.rs

## 验收标准

- diff 仅含 allowed_paths 内文件；N3 dedup 参数拆分后 refresh_due_feeds/refresh_all 语义与声明一致；N4 两处插入成功后清墓碑且 persist_new_feed 除修复点外纯搬运；N5 两处 date() 统一 localtime；N6 unchecked_transaction 包裹成功才 commit、unwrap_or(0) 全部改 ?
- 每项修复有修前失败⇒修后通过的成对证据（变异：N3 还原 dedup 写死 false；N4 移除 remove_feed_tombstone 调用；N5 还原 date(a.published_at)；N6 移除事务包裹——各自新增测试必须失败，还原后必须通过）
- 四门禁全绿：cargo test 通过数 ≥164+新增 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 286/286 不回退
- 不引入新依赖；Cargo.toml/Cargo.lock 与 package.json 零改动
- 文本文件 LF 行尾；台账改动须在 begin 之前完成
- 用户真实数据库不得写入（沿用项目约束）；测试不连真实外网（N3 用进程内 TcpListener）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-19 基线（TASK-063 终版候选验证 RUN-1a3b0ef，提交 f072603）：cargo test 164 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 286/286。本任务 behavior=change 的范围如实声明（四处均为缺陷修复，非契约演进）：① refresh_all 的 dedup 从写死 false 改为读设置——「手动全刷忽略智能去重」是缺陷行为，开 smartDedup 的用户手动刷新后的去重生效是新行为；② add_feed/import_feeds 插入成功后清墓碑——重新添加的源从此可被 pull 正常绑定与推送；③ only_today/feed_counts 的 today 判定从 UTC 日期改为本地日期——非 UTC 时区用户凌晨文章进入「今天」；④ apply_payload 获得原子性——中途失败全量回滚。既有测试除新增文件与用例外一行不动。
- 基线证据：.workflow-kit/tasks/runs/RUN-1a3b0ef069ce4696b3c3bbd75de5a3b8.json
- 需求决定：DEC-992f64fd15714ca2a614fe66d5a144ec
- 补充：N3：src-tauri/tests/refresh_dedup_e2e.rs（非 ignored，进程内 TcpListener HTTP server 复用 ai_e2e 模式）——两个本地源 serve 同一 article link：smartDedup=on 时 refresh_all 后全局该 article 只 1 篇；smartDedup=off 时 2 篇（开关语义双向锚定）；refresh_all 的 dedup 接线此前零覆盖；本地 server 使调度器全链路（含 HTTP）可确定性验证；验证：cargo_test
- 补充：N4：commands/folders.rs 与 commands/opml.rs 的文件内测试——先 add_feed_tombstone 再 persist_new_feed/import_feeds，断言墓碑清除且源插入成功；墓碑清除此前零覆盖；persist_new_feed 抽取使其可脱离 Tauri State 测试；验证：cargo_test
- 补充：N5：src-tauri/src/db/articles_tests.rs（新，仿 dedup_tests.rs 模式）——插入 published_at=今天本地 01:00（带本地 offset 的 RFC3339）的文章：feed_counts 的 today 计数与 only_today 查询必须命中；另插入昨天文章锚定不误判。测试注明其判定力依赖主机时区非 UTC（本项目环境 +08:00；UTC 主机上变异不改变结果但不误报）；时区错位此前零覆盖；以本地 01:00 构造 UTC 日期≠本地日期的确定性形态；验证：cargo_test
- 补充：N6：src-tauri/tests/config_sync_e2e.rs 追加——payload（folders+feeds 合法、app_settings=非法 JSON）apply 必须 Err 且 folders/feeds 表零残留（全量回滚）；合法 payload 原有测试不回退；apply 的原子性此前零覆盖；非法 JSON app_settings 是 merge_app_settings 的确定性失败注入点；验证：cargo_test
- 保留：其余全部既有测试（Rust 164 条、前端 286 条）逐字不动；除新增测试文件/用例外，既有回归网一行不改；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-19T15:20:10.506747Z
- 原截止时间：2026-09-19T19:20:10.506747Z
- 当前截止时间：2026-09-19T19:20:10.506747Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 48 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-19T15:20:10.660803Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T16:08:22.807791Z：Worker changed_files does not match the observed diff; declared but unchanged: src-tauri/src/db/articles_tests.rs; declare either the task's cumulative changes ["src-tauri/src/commands/folders.rs", "src-tauri/src/commands/opml.rs", "src-tauri/src/config_sync.rs", "src-tauri/src/db/articles.rs", "src-tauri/src/scheduler.rs", "src-tauri/tests/config_sync_e2e.rs", "src-tauri/tests/refresh_dedup_e2e.rs"] or this run's changes ["src-tauri/src/commands/folders.rs", "src-tauri/src/commands/opml.rs", "src-tauri/src/config_sync.rs", "src-tauri/src/db/articles.rs", "src-tauri/src/scheduler.rs", "src-tauri/tests/config_sync_e2e.rs", "src-tauri/tests/refresh_dedup_e2e.rs"]；下一步：按报错列出的漏报/多报文件修正 worker-result，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-19T16:09:22.773800Z：阻塞已处置（protocol）：核对 RUN-a64dc5a037374fddb1c8f37dcc6994a4 的 diff 回执与 worker-result：changed_files 与实际 diff 的差异仅为多声明的 db/articles_tests.rs（N5 测试实际以 db/articles.rs 文件内测试落地）；已修正声明，范围本身无越界；下一步：begin 重新实现
- 2026-09-19T16:09:29.296728Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T16:09:46.366357Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/folders.rs, src-tauri/src/commands/opml.rs, src-tauri/src/config_sync.rs, src-tauri/src/db/articles.rs, src-tauri/src/scheduler.rs, src-tauri/tests/config_sync_e2e.rs, src-tauri/tests/refresh_dedup_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T16:13:13.657819Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T16:29:23.004834Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-064.json)

- [RUN-a64dc5a037374fddb1c8f37dcc6994a4](../runs/RUN-a64dc5a037374fddb1c8f37dcc6994a4.json)
- [RUN-ae622c6f04fd404d90e9602de4a9b212](../runs/RUN-ae622c6f04fd404d90e9602de4a9b212.json)
- [RUN-177672cf4733428eb607b1645ca11e9d](../runs/RUN-177672cf4733428eb607b1645ca11e9d.json)
- [RUN-fc5315903f4043779e0a19b3dd042e04](../runs/RUN-fc5315903f4043779e0a19b3dd042e04.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
