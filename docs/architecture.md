# 当前架构

本页描述当前代码的事实边界，不记录历史演进。决定理由见 `.agents/notes/`，未关闭项见 [roadmap](roadmap.md)。

## 进程与依赖方向

FluxReader 是 Tauri 2 Windows 桌面应用：React/TypeScript 前端经 IPC 调用 Rust 命令层，Rust 侧独占 SQLite、HTTP 与系统媒体控制。

```text
React store/components (src/)
  → invoke → src-tauri/src/commands/（薄命令层）
    → db/（schema、迁移、领域查询与事务）
    → sync/（RSS 服务双向同步：Content/State/Push 三通道）
    → local_sync/（纯本地 WebDAV 持续同步 + 便携导出/恢复）
    → ingestion/（直连抓取、解析、分段落库）
    → scheduler/（定时刷新、封面补全、保留维护）
    → ai/、ai_segments/、sanitize/、media/、image_requests/
```

- `lib.rs` 只做模块声明、托盘/窗口生命周期与命令注册；业务在领域模块。
- 命令层短小：取状态锁 → 调用 `db::` 类型化函数或运行协调器 → 返回可序列化行。
- HTTP 与文件处理不持数据库锁；数据库写入走统一的写预留入口（见下）。
- `webdav_transport.rs` 是不依赖应用数据库与同步策略的独立传输层；`local_sync` 在其上实现协议与状态机。
- 前端不直接访问 SQLite 或网络凭据；同步、备份、AI 等服务配置只经后端命令。

## 数据库与 schema 归属

- 单文件 SQLite（bundled），单写连接由 `state::AppState.db` 持有；连接生命周期与 schema 升级集中在 `src-tauri/src/db.rs`。
- `db/schema.sql` 是当前格式的权威结构；`db/migrations.rs` 只保留已发布格式的必要升级。当前格式用独立 application_id/schema version 识别，旧库与旧格式文件只读拒绝，不自动迁移。
- 领域模块：`db/articles.rs`（列表、搜索、读藏、计数、缓存清理）、`db/feeds.rs`/`folders.rs`（订阅与分类）、`db/contents.rs`（正文与媒体引用）、`db/identity.rs`/`url_norm.rs`（身份与规范化）、`db/settings.rs`（设置策略）、`db/sync_queue.rs`/`sync_map.rs`/`outbox.rs`（同步队项、映射与订阅写回）、`db/backup.rs`/`body_retention.rs`（快照与正文保留）、`db/writer.rs`（写预留）。
- 本地同步状态表（`local_sync_*`）由 `local_sync/database.rs` 定义并注册 SQL 函数；逻辑赢家、候选、journal、工作队列、正文 hold/receipt、对象缓存与接收状态都在这组表中。
- `db/settings.rs` 一次定义设置名、值域与同步/便携/本机可见范围；`app_settings` 的整包写入只经 `patch_app_settings`，坏包与未知键保留原值。

## 同步边界

RSS 服务同步（`sync/`）与纯本地 WebDAV 同步（`local_sync/`）互斥：存在活动 RSS 账号时不能启用 WebDAV，避免两套状态权威互相覆盖。两者都以“本机先提交、远端后确认”为原则。

### RSS 服务同步

- 三个通道：Content（订阅/条目/正文）、State（读藏状态与权威集合）、Push（本地意图发送与 ACK 读回）。同一 runtime 内每类最多一个运行操作，并只允许 Content → State → Push 方向嵌套。
- 账号代际（generation）固定在整轮起点；每个真实 HTTP 前与 ACK 落库前复核。断开或换号后迟到结果不能写回新账号。
- 本地读藏写入在事务内递增字段版本并登记本地意图；远端落地只接受字段版本未变、无 pending、来源生命周期仍有效的值。冲突方向由 `sync/conflict_policy.rs` 单点定义。
- 可推送身份要求精确 `remote_id` + 真实 Content 响应的 `remote_origin_feed_id` + 当前有效来源；缺证明的项留在 `bindable_retry`，不伪装成网络失败。
- 队列状态（`local_only`、`bindable_retry`、`bound_pending`、`retry_wait`、`failed`、`expired_unconfirmed`）是同一生命周期；老化只转为过期，不删队项或本地读藏。

### 纯本地 WebDAV 同步

- 逻辑层（`logical.rs`）用字段寄存器与固定 Lamport/actor/seq 戳合并；`local_sync_winners` 是已落地字段与已接受候选的唯一 join。工作队列与候选由捕获触发器与显式 patch 共同维护。
- 捕获的扇出表（A–F）是唯一事实来源：普通文章字段只入自身条目与同 GUID 同源 URL 的显示别名；源身份/生命周期、分类成员、设置各有明确目标，普通元数据编辑不展开整源。
- picker 使用固定桶序 + 主键 seek + `(job,key,revision)` CAS 批删；每个 slice 的目标数与时间有界。
- 正文与状态分通道：正文有分页目录、内容唯一分块、`local_sync_body_hold` 与 receipt 证明；状态 ACK 与正文 ACK 各自确认，正文不上传的不影响状态收敛。
- 便携导出以 WAL 读快照为截止点（不再等待队列清空）；恢复在预览校验后用单事务替换本机数据，并重新生成 actor/主键高水位、清空旧凭据与运行队列。
- 整库状态应用要求候选折叠期间逻辑修订稳定：生产接收走 `begin_receive → stage_page → preflight → accept`，持续前台写入会得到可重试的 `localSyncPending`，停写后收敛；生产便携恢复另有独占锁与 `capture=0`。
- 远端物理删除当前关闭；未知离线设备的保护义务未证明前，不按年龄回收同步正文。

## 前端结构与数据流

- `src/store.ts` 组装 Zustand store；`src/store/slices/` 按领域拆分（bootstrap、feeds、nav、reader、player、settings、sync、ui、localLibrary、ai）。
- `ReadingSession`（`src/store/readingSession.ts`）拥有一次查询的完整有序 ID、已加载进度与定位输入；成员查询由后端一次捕获，属性按 ID 回读，后台同步不插入新成员或重排当前会话。
- 查询与分页契约集中在 `.agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md`；返回位置锚、像素偏移与 LRU 在 `src/lib/readingPosition.ts`，不复制第二份。
- 乐观读藏在 `src/store/internals.ts` 按条目×字段维护版本、写前沿与失败投影；真实回执、读取序号与计数快照收敛，详见“乐观写入”Note。
- 布局（文章/社交/画廊/播客/通知）共享同一时间流虚拟化；键盘焦点、模态焦点陷阱、返回位置和社交折叠各有单点实现。
- 图片经后端代理与前端预算层（`image_requests`/`useImageResource`）取用；ready/active 字节预算与 lease 是资源规格，不是原生内存硬界。

## 并发与写预留纪律

- 所有确认会写的业务事务经 `db::write_transaction`（`BEGIN IMMEDIATE`）：先预留唯一写者，再读取决定本次变更的输入。纯读、WAL 快照与 TEMP 准备不排队。
- `db/writer.rs` 在 SQLite 预留之前提供同库进程内 FIFO 准入，用 Windows 文件 ID 识别同一数据库；不同文件互不阻塞。等待者每 25ms 检查取消信号，已有写者的事务完成后再归还准入。
- 锁顺序固定为业务锁/connection mutex → writer；底层不反向获取业务锁。异步宿主把会等待的同步 DB scope 放入 `state::database_work` 的阻塞 worker，锁不跨 `await` 持有。
- 写回滚失败、`ROLLBACK TO`/`RELEASE` 失败或事务仍 active 时，该文件进入 failed-closed 并拒绝后续写入，直到进程重启；不把恢复失败提交成成功。
- 维护路径使用 `try_database_work` 的非阻塞准入，遇忙即跳过，不推进 retention 游标。
- 前台响应与大库 SLO 不由锁类型或单次 slice 时长推断；性能边界见 [performance.md](performance.md) 与 [roadmap](roadmap.md)。
