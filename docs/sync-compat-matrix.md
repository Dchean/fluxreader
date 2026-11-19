# 同步协议与服务端兼容矩阵

本页分开记录外部协议/服务端能力、客户端实际政策与验证覆盖。政策单点为 `src-tauri/src/sync/conflict_policy.rs`；改变政策必须同步实现、本文和锁定测试。选择理由见[冲突政策决定](../.agents/notes/implemented/architecture/2026-10-07-同步冲突政策单点.md)。

外部参照固定为 [Miniflux faecfa2](https://github.com/miniflux/v2/tree/faecfa2fac61a91f4f69550bb1afd506c4b51749) 与 [FreshRSS 4e8a258](https://github.com/FreshRSS/FreshRSS/tree/4e8a258b011aad190597d7953b7122fc43f4658f)。夹具模拟这些形态，不等于某个真实部署版本已经验收。

## 状态操作与冲突政策

| 操作 | 外部能力 | 客户端行为与限制 |
| --- | --- | --- |
| 标读推送 | GR `edit-tag` 可聚合 entry ID；Fever `mark=item&as=read` 只接受单 ID | 同源等价组每个成员独立入队；GR 合批/Fever 逐 ID，每个身份读回确认；不同源不传播 |
| 取消标读推送 | GR remove read；固定 FreshRSS/Miniflux 的 Fever 支持扩展 `as=unread` | 同源等价组各成员独立目标，不跨源。原 Fever 规范只定义 read/saved/unsaved，不能把此扩展当作所有 Fever 服务端支持 |
| 收藏/取消收藏推送 | GR starred add/remove 为赋值；固定 FreshRSS api_version4 与 Miniflux api_version3 都是绝对赋值 | 同源等价组各成员独立目标。Fever ≥4 直接写，3/未知先查 is_saved，仅不同才写以兼容历史 toggle；api_version3 不能等同 toggle |
| GR light 读状态 | 完整 reading-list 可用 `xt=read` 排除已读 | 双向：完整远端未读集合可将本地已读恢复未读；本地未读缺席时仍先查候选存在性再标读，集合失败不作该字段差分 |
| Fever light 读状态 | `unread_item_ids` 全集，没有已读集合 | 双向权威：命中可恢复未读，未命中置已读；依赖集合完整和字段保护 |
| 两协议 light 收藏 | GR starred 流、Fever saved 集合 | 双向权威；完整集合内命中收藏、不命中取消。不是协议强制的冲突政策 |
| full 合并 | 协议提供条目及状态，不规定本地合并方向 | Content 处理身份/正文及首次初始化，已绑定 flags 由 State 双向合并，跨源不传播；本机早到绑定必须同源与非空内容确证 |

四格方向常量集中在 `conflict_policy.rs`。用户选择双向未读，取代早期 GR read-wins 及跨源 read 广播；两协议按实际服务端接收状态对账，不承诺离线点击的全局真实时间顺序。源内相同内容保留每个 GUID/远端 ID 实体，仅分组显示；四向用户动作对所有组成员同事务入队，每个成员受字段版本/pending 与 ACK 读回保护。GR 的 `kept-unread` 标签目前没有专门消费逻辑，收到时仍按普通 read/unread 规则处理，不解释为 sticky-unread。

共享保护：HTTP 前还捕获源 `sync_id/lifecycle_version`，精确内容合并、GR 未读差分及 GR/Fever 权威对账提交时须仍属同一 active 生命周期；退订再重订不会接受旧生命周期的响应。失败重建映射同时保留源和字段快照。本地读/藏写入在同一事务递增各自 `read_version`/`star_version`，pull 在任何 HTTP 前捕获字段版本；落地只接受版本未变且对应字段无 pending 的结果。清队不清版本，read 的冲突不阻止无冲突 star，反之亦然。GR 权威集合失败按 read/star 字段隔离，成功侧仍可对账；Fever unread/saved 任一失败跳过该段对账。

## 条目身份、状态与确认

| 契约 | 当前实现及边界 |
| --- | --- |
| ID 形态 | Fever JSON 数字或十进制数字字符串都按整数处理，拒绝负值/溢出/浮点；GR 无前缀数字按十进制，带 Google item tag 的尾段按 hex，不凭长度猜进制 |
| 状态三态 | GR 缺失/null categories 是 Missing，显式 `[]` 是 false；Fever 缺字段 Missing、0/1 为明确值，其他数字拒绝。Missing 只更新可接受内容，不覆盖已有读藏 |
| ACK 不等于已应用 | 请求成功外观之后按目标 ID 读回；重复/无关 ID 不补足缺项。主目标存在且目标态一致才确认；空/短/缺状态/非法回包保留 retry_wait/verification，不猜目标删除 |
| 远端来源凭据 | 只有真实 Content 响应的 origin 建立 `remote_origin_feed_id`；裸 ID、已有正文和本机 feed 信息不构成证明。State 与 Push 只消费当前有效凭据；article ID/feed 或 source 身份/生命周期变化清证，发送前和 ACK 落库前再次核对，失效意图留队等待补证 |
| 成员确认 | 每个持久成员只确认自己的远端 ID；组内未确认字段保护同组，不能用一个成员成功代替全组。相同 states 轮已尝试 queue ID 不重复补推 |
| 入库归属 | 精确远端条目身份优先，URL 只缩小候选，未绑定本机早到条目还须同源、标题/附件与非空正文等价确证。有效远端 ID 且 feed 已绑定时，缺/空/空白 URL 仍可独立入库；空白链接不产生匹配键；新 GR 条目缺有效 origin 明确失败并保留游标 |
| 正文修订 | 同一精确远端 ID 可更新 RSS 正文/标题/媒体并拆分显示组，保留 readability 展示正文；AI 输入变化清旧产物，稀疏回包不清已有正文。图片来源显式 rss/page：完整非空 RSS 可删除旧 RSS 图，独立 page 封面保留；正文图集、封面及附件实际输出变化共用媒体版本触发器 |
| 无 GUID 直连 | URL/title+date 仅是候选；非空持久键采用规范 URL/无 URL 域与内容/标题/附件载荷，排除日期及原候选形式，有字节时复核。空正文使用规范 URL/标题/原始选定源日期 UTC 与附件的完整观察；弱空→非空不转移旧状态，真实 GUID/remote ID 仍原行补全 |
| 订阅身份 | 远端 feed ID 优先，仅未绑定本地行参与 URL 兜底。远端来源的同 ID 改 URL 保留本地 ID、文章、布局、分类与自定义标题；本地来源的已绑定源保留本地 URL。多重绑定、规范 URL 多候选或唯一 URL 冲突明确报错并保留原值。远端源重新出现恢复原稳定身份；本地退订墓碑仍阻止复活 |

有已证明远端身份的本地意图进入推送；缺身份或缺来源证明的条目按所属源区分 local_only 与 bindable_retry，不计作可推 waiting。超过 30 天转 expired_unconfirmed，保留队项和本地读藏。显式恢复用摘要 account_generation 复核当前账号、按文章当前目标建立新操作 ID；缺文章仍过期。断开/换号放弃旧状态队列，本地直连读藏保留但不会自动迁入新账号。详见[队列生命周期](../.agents/notes/implemented/architecture/2026-11-03-同步队列生命周期与本地意图.md)。

冷绑定由 Content 每轮最多 2000 条轮转补证，已有证明的状态变化不新增 origin 请求。GR 已有未读消失候选仍通过 contents 确认目标存在，该请求在 State 许可内。未完成订阅绑定或墓碑判断未知时，两通道都不发条目/权威集合请求。Fever 仍要求 unread/saved 成对成功，失败时已绑定 flags 连正文页也不得改写。

## 分页、回补与持久边界

| 路径 | 获取预算 | 提交与恢复 |
| --- | --- | --- |
| GR 主列举 | reading-list + ot，单页 n=10000，最多 200 页；continuation 必须合法、未重复且严格前进 | 在列举开始前取起点墙钟候选。普通正文 100 ID/块；逐语句落库，失败保留已提交内容并保留旧主游标，不能称整页回滚 |
| GR 权威集合 | unread/starred 用完整集合，不依赖 ot；分页不完整、空 itemRefs 却有 continuation 都报协议错误 | Missing/损坏集合不是空集；成功字段可独立对账。格式正常但语义漏报无法结构检测 |
| GR 缺身份/正文回补 | 每次 pull 每集合最多 2000 个唯一 ID，100/请求、批内并发至多 3；按数值 ID 与账号代际持久轮转 | 预算内请求和合并全部成功才存进度，缺 feed/空正文也占预算防头部饥饿；进度或预过滤 DB 错终止。states 发现订阅后最多重拉一次 |
| Fever 增量 | since_id 向新，每页 50；非空短页继续到空页 | 连续成功的增量页或从顶部连续历史页才推进 since；数据和该页游标同短事务，失败整页回滚，先前成功页保留 |
| Fever 历史 | max_id 向旧，页内最小 ID 为下页游标且严格减小；每轮最多 200 页 | Unknown/Pending/Complete，缺失 Unknown、损坏明确报错；预算或失败保留 Pending，自动 light 续取，full 从顶部幂等重放。先存 Pending 才允许更大 since；只有空页完成 |
| Fever 点查 | with_ids 50 ID/批；i64::MAX 顶边界另点查；未读/收藏正文回补无每轮总量预算（GR 为每集合 2000/轮，两协议不对称） | 点查与 Pending 续取页不推进连续 since，防跳过区间；初始或回滚后 map 读取失败即停止，不退空映射继续 |

GR/Fever 协议响应使用 16 MiB 流式读取硬界，含解压后的大小。Fever 完成时间写入只在无失败且历史完成后尝试，写入失败进入报告；GR 游标在主列举、合并、绑定和权威回补全部成功后推进。绑定不完整或新条目缺归属时保留旧游标，下一轮可重试。Fever 绑定失败恢复把 since 与历史状态一并回写轮前值：轮前 Complete 也原样回写，轮前 Unknown 时下一轮整段历史重走（每轮至多 200 页）。

认证重试累计首轮已提交变化与统计，只过滤已恢复的认证故障。首轮非认证 SQL/协议错误继续作为注明“当时状态”的历史事实保留；重试成功不把旧“待重试”文案冒充当前队项状态。远端已接受但本地确认出队失败时明确报告，队项留待重试。

GR/Fever 会话的每次实际读取/写请求前复核账号代次；GR 惰性 action token 返回后、POST 前再次复核，文章 edit-tag 还核对本次计划的来源凭据。Fever 保持逐 ID mark 和 50 ID 分块 ACK，51 条为 51 次 mark + 2 次读回；api_version<4 时每个收藏目标在 mark 前另有 1 次 with_ids 收藏态探测。断开后阻止新请求；已发写入或认证无法撤回。订阅 outbox 断开保留同账号意图，同身份重连可重绑代际后恢复；换身份按现有规则清账。

429 与 Retry-After 退避由 `sync_http.rs` 的 `send()` 统一处理：按请求 origin（协议、主机与端口）在进程内记录冷却；429 或带 Retry-After 的 503 触发，Retry-After 接受整数秒或 RFC2822 日期（日期按当前时刻折算），缺失或非法取 60 秒，结果下限 1 秒；503 无 Retry-After 不建立冷却。冷却期内同 origin 的新请求不发送，直接返回 `rateLimited`；该拒绝不睡眠等待，也不丢弃已持久化的本地意图。冷却只延长不缩短，进程重启清空；仅 GR/Fever 走该入口，WebDAV 传输不经过它。

## 固定服务端差异

- **Miniflux GR**：ot 为 `published_at > ot`，严格大于发布时间，不是状态修改时间；固定 [handler](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/googlereader/handler.go) 与 [query builder](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/storage/entry_query_builder.go) 支持此结论。未读/收藏完整集合回补绕开 ot，已读未收藏晚到旧文仍依赖 full；起点墙钟不能消除同秒或旧发布时间边界。
- **FreshRSS GR**：固定 [greader.php](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/p/api/greader.php) 将 MinDate/MinModifiedDate 用 OR 组合；[EntryDAO](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/app/Models/EntryDAO.php) 为 `id >= ot×1000000 OR lastModified >= ot`。读藏改的是 lastUserModified，所以不能把 ot 当状态变更流。
- **Fever 端点与短页**：Miniflux 常用 `{base}/fever/`；FreshRSS `/api/fever.php` 或 `/p/api/fever.php`，api_key 在 POST 表单而非 query。两者 max_id 页为 id DESC。FreshRSS 在 LIMIT 50 之后由 EntryBeforeDisplay 过滤，非空短页不等于结束。
- **Fever 收藏版本差异**：固定 Miniflux 虽报告 api_version3，saved/unsaved 已是条件绝对赋值。历史 toggle 夹具是兼容反例，不能当所有 v3 事实；保守先探测仍有多设备并发窗口。
- **订阅修改**：固定 Miniflux [Update Feed API](https://miniflux.app/docs/api.html#endpoint-update-feed) 与 [storage/feed.go](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/storage/feed.go) 允许同 ID 更改 feed_url，并按原 URL 精确去重；客户端不能用折叠 HTTP/HTTPS 的 URL 覆盖两个已知 ID。
- **Bazqux**：不支持；其他 RSS 服务与多账号不在范围，见 [roadmap](roadmap.md#accepted-tradeoffs)。

固定 Fever 集合端点没有 continuation 或已验证的状态增量参数，当前只能整段重拉。GR long-poll/ContinueWhatNewStream、Fever last_id_on_sync、订阅清单 ETag/304 在历史核对中缺少可采纳的固定版本依据，未实现为通用优化。选择理由见[会话复用与有界并发](../.agents/notes/implemented/architecture/2026-11-01-同步会话复用与有界并发与聚焦同步.md)。

## 鉴权、分类与订阅写回

| 维度 | 当前契约 |
| --- | --- |
| GR 认证 | 请求统一带 `Authorization: GoogleLogin auth=<auth>`；POST 用 action token 作为 T。Miniflux token 可等于 auth，FreshRSS 可为独立字符串；首写 OnceCell 缓存 |
| GR ClientLogin | 响应先按 JSON 取 `Auth`（Miniflux 尊重 `output=json`）；JSON 解析失败或 `Auth` 为空时回退经典文本行 `Auth=`（FreshRSS 忽略 `output=json`）；文本 `Error=` 作为认证失败带出原因，两者皆无则明确报错 |
| token 失败 | 仅 404 允许回退登录 auth；401/403/5xx/空体/网络错明确失败，不靠 URL 猜服务端 |
| 分类读取 | category_name 单点识别 type folder 或缺 type 且 label 前缀 ID；label 优先，缺省取 `user/.../label/` 后缀，中文/斜杠保留；state tag 和 type tag 不成目录 |
| 分类写入 | 裸目标名始终前置一次 `user/-/label/` 后用 form 编码，名字本身含前缀字面量不误判；移动需处理旧成员关系，不把追加 label 当移动 |
| GR 普通写响应 | HTTP 成功且 trim 后为 OK 才视作成功外观；FAIL/空体/其他正文脱敏报协议错，不回显凭据或服务端正文。状态写还需目标读回 |
| 订阅能力 | GR 支持客户端订阅/分类写回；Fever 相应操作仅本地。客户端当前分类改名/删除仍按成员操作表达，不宣称远端空分类已改 |
| 退订例外 | 既有 400/404 容忍可能把真实删除失败当目标不存在并清队；墓碑仍由 pull 确认回收，本机不复活但远端可残留。此上限与状态 ACK 读回是不同机制 |

能力声明由生产能力矩阵和持久订阅回执承担，详见[能力矩阵与持久回执](../.agents/notes/implemented/architecture/2026-10-15-订阅写回能力矩阵与持久回执.md)、[订阅执行器](../.agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md)。Gist、GitHub 同步授权与手动配置快照前后端已退役；GitHub Releases 更新检查保留。

## 纯本地 WebDAV

WebDAV 与活动 RSS 账号互斥。本地源按创建归属/原本地历史进入同步，远端 RSS 镜像不进入其 journal；分类按创建 origin 及本地源成员闭包归属，remote_id 不作身份推断。普通设置、源/组布局及 AI 行为开关同步；凭据、AI 服务配置/产物不传。read/star 独立按操作创建时固定的 Lamport/actor/seq 合并，离线冲突不承诺真实点击或上传先后；重试不生成新操作。

清单和状态/正文对象不可变，写后读取确认精确字节，不能当作全球 winner。接收缺块/错误不是空库，状态/正文分别重试；未读或收藏正文未补齐显示 pending。远端物理 DELETE 当前关闭（边界见 [roadmap](roadmap.md#sync-correctness)）：替换对象精确 ACK 和已知 actor 覆盖仍不能证明未知/离线读取者已无保护义务。未确认正文继续保留，不能以新设备缺正文作为允许回收的理由；远端空间治理仍待完成。正文对象超限以 `localSyncFormat` 拒绝、不做截断，大小上限见[契约单点](contracts.md)的「同步正文对象大小上限」；它与 GR/Fever 传输的 16 MiB 不是同一常量。

候选正文目录分页、内容唯一分块，便携 v4 从同一内容身份导出流式包。新库恢复新 actor 和主键高水位，断开凭据与旧队列。详细协议职责、真实备选和存储边界见[资料库决定](../.agents/notes/implemented/architecture/2026-11-09-纯本地资料库同步与便携恢复.md)，独立 WAL 连接的事务前提见[写预留决定](../.agents/notes/implemented/architecture/2026-11-09-多连接SQLite写预留与原子设置.md)。

早期真实 WsgiDAV 生产入口测试仅覆盖当时版本，不能背书之后的正文目录/原子设置/WAL 改动。完整规模、HTTP SLO 和原生窗口尚未验收；当前版本结果与剩余条件见 [roadmap](roadmap.md)。

## 真实服务端的限定实测

2026-11-07 用未改动的生产 live 测试（客户端基线 `dev@1bb698c` 加修复后的固定 `snapshot-cd`）在固定 Miniflux 2.3.5 与 FreshRSS 1.30.4 上各跑 2 源 120 篇合成文章：GR 3 项（认证/订阅、ID/正文、收藏写回恢复），Fever 4 项（认证/订阅、未读/收藏/条目、读藏写回恢复及空临时库的完整同步），逐项恰 1 passed 且 exit 0，完整未读/收藏 ID 集合前后相同。

完整同步实测只覆盖 Fever 引擎的 2 源 120 篇落库，不外推为 GR 引擎已跑完整同步。FreshRSS 在该配置下根地址自动发现失败、显式 `/api/greader.php` 通过，该失败不被显式路径的成功关闭，结论限定当前服务器路由形态。两套服务端均未覆盖其他版本、反代、多设备并发与全部异常输入；这是一次性定向运行证据，不替代安装版/WebView2、真实用户库与最终集成门禁；运行地址、构建版本与逐项输出未随库留存。

## 验证入口与边界

回归入口见 CI 的 rust job 与 `src-tauri/tests/*_e2e.rs`。测试名是覆盖入口，不是当前全部通过的结论；精确提交的门禁状态与最近证伪由固定 CI 与 [roadmap](roadmap.md) 维护，未解决行为留在 roadmap，避免矩阵复制轮次进度。真实服务端与容量用例是 `#[ignore]`，需按任务单独约定读写边界后运行，不由 CI 默认覆盖（约定见 [AGENTS.md](../AGENTS.md) 的门禁段）。
