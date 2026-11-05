# 同步协议 × 服务端兼容矩阵（Google Reader / Fever）

本文是同步状态语义的**用户可见对照表**。政策的代码单点在
[`src-tauri/src/sync/conflict_policy.rs`](../src-tauri/src/sync/conflict_policy.rs)
（每格政策的常量定义、选择理由、历史依据都在那里），本文与其同步维护——
**改政策 = 改政策点 + 改本文 + 改对应锁定测试**。政策为什么以常量形式存在、
三类事实为什么要分列，见
[同步冲突政策单点](../.agents/notes/implemented/architecture/2026-10-07-同步冲突政策单点.md)。

**时点声明（2026-11-05）**：本轮外部事实复核采用 2026-11-03 留存的固定源码，
出处记录在 `tmp/audit-sync-core/upstream/provenance.json`：
[Miniflux faecfa2](https://github.com/miniflux/v2/tree/faecfa2fac61a91f4f69550bb1afd506c4b51749)、
[FreshRSS 4e8a258](https://github.com/FreshRSS/FreshRSS/tree/4e8a258b011aad190597d7953b7122fc43f4658f)。
下文另行标明的旧协议材料保留原日期；测试替身是独立实现，不能充当部署版本。
固定源码、客户端行为与真实部署验收分列；升级或更换服务端后须复核。

**版本验证声明（必读）**：本矩阵的「指定服务端版本已验证」列**只覆盖测试替身**
（`src-tauri/tests/mock_greader.rs` 的宽松形态，以及
`src-tauri/tests/greader_compat_e2e.rs` 的 Miniflux/FreshRSS 严格夹具、
`src-tauri/tests/fever_compat_e2e.rs` 的 FreshRSS/Miniflux 形态 Fever 严格夹具）——它们是
被自动化测试锁定的服务端形态。
**Miniflux / FreshRSS 的具体部署版本未逐一实机验收**；文中「协议/服务端支持」列
涉及外部服务端的事实来自上述固定源码，**不代表所有部署版本都已验收**。
升级或更换服务端后三者都须复核。三列必须分开——协议/服务端事实、本客户端实现现状、
实际验收过的版本不是一回事；把「本客户端未实现」写成「协议做不到」会掩盖可补齐的能力。

范围：状态与条目同步（push 队列 + pull 合并/对账）。订阅层（feeds 阶段的
订阅/分类增删改）不在本矩阵内。两协议共用同一条
`sync_queue → plan_push / exec_push` 推送管线与同一条 `merge_pulled_entry`
合并函数，协议差异集中在**对账方向**与**游标/条目获取能力**两处。

## 1. 协议 × 操作差异矩阵

三列口径：**协议/服务端支持** = 协议规范或文首固定参考实现
事实上的能力；**本客户端已实现** = FluxReader 当前代码真正落地的行为；
**指定服务端版本已验证** = 实际被测试/实机验收覆盖到的服务端形态。
某格为「客户端选择的策略」时，协议列不写、只在实现列写并标注。

| 维度 | 协议/服务端支持 | 本客户端已实现 | 指定服务端版本已验证 |
|---|---|---|---|
| 标读（推送） | GR `edit-tag`（`a=user/-/state/com.google/read`）按 entry id 批量；Fever `mark=item&as=read` **只接受单个 id**（逗号分隔无效，须逐个调用） | 绑定 entry + **全部同文副本**（read 广播政策，§3）；GR 多 entry 聚合单请求；Fever 逐 id 调用；成功响应后仍须读回确认 | `mock_greader`、`sync_ack_verification_e2e` 与 `fever_compat_e2e` 的 HTTP 夹具锁定推送/读回；真实部署版本未逐一验收 |
| 取消标读（推送） | GR `edit-tag` remove read；FreshRSS/Miniflux Fever 支持扩展 `mark=item&as=unread`（单 id，见 L8） | **仅绑定 entry**，不广播；读回一致才出队 | 同上一行；不把协议扩展泛化为所有 Fever 实现 |
| 收藏 / 取消收藏（推送） | GR starred tag add/remove 是显式赋值。固定 FreshRSS 4e8a258（api_version 4）用 `markFavorite(id,true/false)`；固定 Miniflux faecfa2（api_version 3）用条件判断 + `SetEntriesStarredState(true/false)`，也是绝对赋值。**api_version 3 不等于 toggle**；历史 toggle 材料与该固定版本不可混称 | 仅绑定 entry；`push_star_state` 对 api_version ≥4 直接写，版本 3 或尚未知时先按 id 读 `is_saved`，仅当前态不同才发 mark，以兼容历史 toggle。探测缺条/缺字段保留为未知；写后另行读回验证，不因写请求返回成功就清队 | `fever_compat_e2e` 分别覆盖 Absolute 与 Toggle 夹具；`toggle_fixture_*` 锁定取反、丢响应重发和 stale 队列，不声称全部 Miniflux v3 都是 toggle |
| 条目读/藏三态 | GR 用 `categories`，Fever 用独立 `is_read/is_saved`；未提供字段不等于明确 false | GR 缺失/null 分类为 Missing，显式 `[]` 为 false；Fever 缺失为 Missing，0/1 为明确值，其余数值拒绝。Missing 只更新内容，不覆盖既有读/藏；明确状态仍受字段版本、pending 与冲突政策保护 | `greader.rs` / `fever.rs` 三态解析、`sync/entries.rs` 既有状态保持测试 |
| 写回 ACK 与读回 | HTTP 200/OK 或 Fever auth=1 只能证明请求得到成功外观，不能证明目标存在并已更新；当前端点没有权威删除信号 | 读回按请求 ID 集合判完整性，重复/无关 ID 不能凑数。主目标存在且状态一致才出队；空/短/缺字段/非法响应或相反状态进入 `retry_wait/verification`，不推断 `target_missing`。读广播只以主目标结算，缺副本记 partial；同一 states 轮已尝试 queue id 不补推第二次 | `sync_ack_verification_e2e`、`fever_compat_e2e` 的空/短/错 ID/缺状态/主副本与单轮不重发用例 |
| 轻量对账 · 读状态 | GR 权威信号：`stream/items/ids` 可拉「已读 id 集合」（`s=read`），也可经官方 `xt` 排除参数拉「当前未读全集」（`s=reading-list&xt=read`，三家服务端均支持）；**无显式「保持未读」信号**；Fever 只有 `unread_item_ids`（无已读集合） | **GR 单向 read-wins（2026-11-01 起为未读差分形态）**：light 轮不再全量拉 read 集合，改拉 `reading-list + xt=read` 当前未读全集，与本地已绑定未读行差分出「远端新变已读」，仍经 `conflict_policy` 单向落地；差分候选经 `item_contents` 存在性校验（远端已删除的未读条目不误标），未读集合不完整（页上限/请求失败/损坏 continuation）整轮跳过差分（fail-closed）。**Fever 双向权威**不变（unread 命中 → 本地未读可复活；未命中 → 本地已读）。**注：read-wins 是本客户端选择的冲突策略，不是 Google Reader 协议必然要求**；见 §2 | mock_greader 替身（xt 排除路由 + `unread_diff_e2e` 四等价性用例）与 greader_compat_e2e 严格夹具（未读覆盖/失败隔离）经自动化测试锁定；真实 Miniflux/FreshRSS 未逐一实机验收 |
| ot×状态流增量 | 固定 Miniflux 将 ot 映射为 `published_at > ot`；固定 FreshRSS 是 `id >= ot×1000000 OR lastModified >= ot`，读/藏变化只动 `lastUserModified`。因此不能依靠 ot 完整发现旧文状态变化；Bazqux 的既有官方材料也警告勿用 ot | 状态对账使用完整 unread/starred 集合；普通正文增量仍用 ot，权威集合缺身份/正文回补独立于 ot | mock/严格夹具锁定客户端替代机制；FreshRSS 认证形态夹具共享 Miniflux 时间模型，不证明真实 FreshRSS 的过滤列 |
| 轻量对账 · 星标 | GR starred stream / Fever `saved_item_ids` 都能给出「当前收藏 id 集合」，隐含「未命中 = 已取消」 | **双向权威**（命中 → 收藏；未命中 → 取消收藏）——两协议一致（同为客户端策略） | mock_greader；截断守卫由 `star_reconcile_truncation_e2e` 锁定 |
| 全量合并 · 读状态 | 协议本身不规定合并规则 | 与 Fever 共用 `merge_pulled_entry`：read-anywhere-wins（任何副本已读都接受）；unread 仅同源绑定 entry 接受（**跨源副本的未读不复活**）；跨源副本记账 + 远端已读即标读 | `dual_client_e2e` / `dedup_sync_e2e` 锁定客户端语义（无服务端版本维度） |
| 权威状态集合端点 | GR `stream/items/ids`（s=starred；未读集合 = s=reading-list + xt=read），continuation 分页，单请求 `n` 上限 **10000**（GR 官方 wiki；Miniflux `MaxEntryIDsLimit=10000`；FreshRSS 无 clamp）；Fever `unread_item_ids` / `saved_item_ids`，单请求返回全集——**协议下界：无 continuation、无增量参数，集合只能整段重拉，不存在增量空间**（2026-11-01 登记，防重复立项） | 两协议均拉全量集合用于轻量对账；GR 单页 n=10000（200 页上限语义不变），分页截断/中断按失败处理；Fever 响应**缺字段是协议错误**（不当空集合）、CSV 非法项显式报错 | mock_greader（含分页/截断形态；Fever 路由按 action 补对应字段）；Fever 严格形态（缺字段/非法 CSV）由 `fever_compat_e2e` 锁定；live 测试（`#[ignore]`）另行 |
| 条目 id 取值形态 | Fever：FreshRSS `fever.php` 的 `id` 是 PHP numeric-string（JSON **字符串**，如 `"1791440000000000"`），Miniflux 是 JSON 数字；GR `stream/items/ids` 的 `id` 是 64 位**十进制字符串**，`item_contents` 是长格式 tag（尾部 hex） | `FeverId` 两种形态都接受、一律按十进制（不猜 hex、不经 f64；非法/负值/溢出显式错误）；`greader::parse_item_id` 按协议形状（前缀 tag→hex；无前缀纯数字→十进制） | `fever_compat_e2e`（字符串长 id 全链路往返、JSON 数字形态、非法/溢出/负值/浮点报错）；`greader.rs` 单测（前缀 hex 含 a-f、无前缀十进制） |
| 条目获取 | GR `item_ids`(reading-list, ot, xt) 分页列举 + `item_contents`；Fever `items` 支持 `since_id` 向新和 `max_id` 向旧取历史，重复到空页 | GR 单页 n=10000、正文 100 ID/批；light 从完整 unread/starred 集合独立回补缺身份或正文条目，每次 pull 每集合至多 2000 唯一 ID并持久轮转（L7）。普通 contents 要求覆盖全部唯一请求 ID，额外 ID 不合并。GR 逐语句落库，块失败保留已提交数据但不推进主游标。Fever 增量/历史为 50 条/页、with_ids 为 50 ID/批；每页数据与该页游标同短事务，失败整页及内存统计/maps/seen 回滚。两协议响应都有 16 MiB 逐块读取硬界 | `greader_compat_e2e` 覆盖晚到未读、缺正文、2001 ID 轮转与错 ID；`sync_phases_e2e` 覆盖回补绑定；`fever_compat_e2e` 覆盖完整历史、分页/页事务及读取硬界；均为受控夹具 |
| 历史回溯（向更旧翻页） | **Fever `items&max_id` 支持**（当前 Miniflux main/dev 与 FreshRSS `fever.php`：取 `id < max_id` 的最近 50 条，两家固定实现均 `ORDER BY id DESC`）；GR 无此概念（用 ot 时间窗口近似） | **已实现**：首连/full 同步 `items_before` 循环直到空页取尽保留历史（含已读非收藏）；游标取页内最小 id（与页内顺序无关）且必须严格减小；分页失败/游标不前进显式记 `report.errors` 并保留已拉进度。「历史是否仍被服务端保留另当别论」 | `fever_compat_e2e`（125 篇全可达、页序无关鲁棒性、分页失败重试、游标不前进快速失败、幂等与增量只走 since_id） |
| 历史状态与自动续取（OPT-005 R1/R2/R3） | 协议不规定（客户端策略） | 内部 settings `fever_history_state` 显式三态（无 schema 迁移）：缺失=**Unknown（绝不当完成**，旧库只有 since 也自动从顶部补旧历史）；`pending:<max_id>`=未完成；`complete`=已取尽；损坏=**显式错误且不动游标**。每页「页数据+checkpoint/since」同事务；「先初始化 Pending 成功才允许更大的 since 落库」；每轮至多 200 页（1 万条），预算/失败/游标不前进都保留 Pending 续取点、不推进完成时间；**含自动 light** 从 Pending 续取，只有走到空页才 Complete；手动 full 从顶部重放（幂等）。**R3：map（匹配映射）读取失败——初次构建或页回滚后重建——记 report 并立即终止本轮，保留 Pending/已确认 since，不执行后续 history/with_ids/完成时间；绝不退化成空映射继续**（空 feed 映射会让 merge 合法跳过却照常确认游标/写 Complete）。`max_id` 严格 `<` 漏掉的 `i64::MAX` 边界条目由 `with_ids(MAX)` 顶覆盖 | `fever_compat_e2e`（状态写失败不推 since→light 重试、页 INSERT 失败整页回滚+Pending 保留→下轮补回、Unknown 旧库自动补旧历史、损坏状态报错不改游标、full 失败→light 自动补齐、200 页预算→下轮续取 10050 条、i64::MAX 已读非收藏可达、R3 map 列类型错误两反例：初始读取失败无假 Complete/since/time、页失败后重建失败终止整轮） |
| 增量游标 | GR `ot` 的过滤列随服务端实现；Fever `since_id` 为条目 id 过滤 | GR 在列举前取起点候选；主列举、正文块、绑定和权威集合/回补失败时保留旧游标，幂等重拉。起点候选不承诺涵盖所有发布时间边界。Fever 非空短页持续翻到空页；`last_sync_entry_id` 只随连续成功的增量页或顶部历史页事务提交，with_ids、Pending 续取页与失败页不推进；完成时间仅在零失败且历史完成时写 | `pull_cursor_e2e` / `pull_window_e2e` 与 `fever_compat_e2e`；时点与提交边界见 §4 |
| 时间过滤列 | 固定 Miniflux 为 `published_at > ot`；固定 FreshRSS 为 `id >= ot×1000000 OR lastModified >= ot`；Fever 无该时间过滤参数 | GR 普通窗口依赖服务端过滤语义；light 的完整 unread/starred 集合回补与状态对账不依赖 ot。已读未收藏的晚到旧文仍需全量同步 | mock 与严格夹具的时间模型是 Miniflux 形态；FreshRSS 实际过滤来自固定源码，未逐部署实测 |
| 「失败 ≠ 空集合」守卫 | 协议本身不规定（纯客户端安全策略） | GR unread（未读差分）/starred 按字段独立：成功侧照常对账，仅失败侧跳过（未读集合不完整时 read 差分整轮不落地）；unread/saved 任一失败 → `reconcile_ok = false`，整段对账跳过（`fever_pull.rs`） | GR：`greader_compat_e2e` 两侧失败隔离、`star_reconcile_truncation_e2e`、`unread_diff_e2e`；Fever：逻辑覆盖 + live 测试 |
| 字段版本守卫（pending 保护升级，OPT-007） | 协议本身不规定（纯客户端防乒乓策略） | 字段粒度：`articles.read_version` / `star_version` 随本地读/藏写入**同事务**递增（推送成功清队后版本仍保留），pull 起点快照 + 当前 DB 版本 + 该字段 pending 三输入判定——快照不等或有 pending 即跳过该字段（另一字段不受牵连）；版本快照必须在任何 HTTP 之前抓取。两协议共享，见 §2 | `pull_field_versions_e2e`（屏障网络时序）；单元：`conflict_policy.rs` / `greader_pull.rs` / `fever_pull.rs` |

## 2. 冲突政策表（对账方向）

**全部四格都是本客户端选择的冲突策略，不是协议必然要求**：
Google Reader 规范没有规定「远端已读集合未命中时本地该怎样」，Fever 也没有规定
「saved 集合未命中即取消收藏」。协议只提供集合端点；方向语义由本客户端选定、
写成常量、被锁定测试锚住。特别是 **GR × 读状态的 read-wins 单向**
是客户端为避免「历史条目成批翻回未读」而做的取舍，不是 Google Reader 的要求。

政策格与代码常量一一对应（`conflict_policy.rs`；行级落地
`apply_read_by_policy` / `apply_star_by_policy`——两者返回 `AppResult<usize>`，
DB 写失败向上传播不再吞成 0）：

| 政策格 | 常量 | 取值（客户端策略） | 理由（摘要） |
|---|---|---|---|
| GR × 读状态 | `GR_READ_DIRECTION` | 单向 read-wins（**客户端选择**） | 当前未读集合不提供每次用户操作的历史，客户端不据此批量复活本地已读。远端显式标未读不经 light 对账回流；本地显式标未读则经 push 发出。缺身份/正文回补保持这一政策 |
| GR × 星标 | `GR_STAR_DIRECTION` | 双向权威（**客户端选择**） | 收藏低频强意图、取消是显式动作；截断集合的误判由失败守卫拦截（`star_reconcile_truncation_e2e`） |
| Fever × 读状态 | `FEVER_READ_DIRECTION` | 双向权威（unread 集合，**客户端选择**） | Fever 拿不到已读集合，「未命中 = 已读」是唯一信号，不双向则两端读状态都无法收敛。前提：Miniflux 按 URL 去重 entry，Fever 视角无跨源副本；误判代价由失败守卫 + 字段版本守卫兜底 |
| Fever × 星标 | `FEVER_STAR_DIRECTION` | 双向权威（**客户端选择**） | 同 GR × 星标（saved 集合） |

共享守卫（两协议一致，属政策的一部分）：

- **字段版本守卫 + 字段 pending（OPT-007）**：接受远端某个字段前要求
  「pull 起点版本快照 == 当前 DB 版本 且 该字段无未推送 pending」——本地
  写入在同事务递增对应版本，即使随后推送成功、队列已清，版本仍保留，陈旧
  网络快照不能回滚用户刚确认的意图；read 与 star 两列彼此独立（本地只改读
  时收藏照常收敛，反之亦然）。快照必须在**任何 HTTP 之前**抓取，失败后按
  DB 重建映射时保持快照原值（不回刷）。消费点：两个 reconcile 循环与
  `merge_pulled_entry` 的 `field_accepts_remote`（`conflict_policy.rs`）；
  共享查询/快照 `db::sync_match_maps`。
- **「失败 ≠ 空集合」**：权威集合拉取失败（网络/服务端错误、分页截断）时
  不得当空集合。GR 按 read/starred 字段隔离失败，失败侧不对账；Fever 的
  unread/saved 任一失败则整段对账跳过。数据库或会话代际错误可终止整轮。

**本地意图与 schema**：当前数据库迁移末版为 v34；队列生命周期首次在 v32
引入，时间来源在 v33 引入，抓取代际/源名分域在 v34 引入。direct 条目没有
remote_id 时，按所属 feed 身份区分 `local_only` 与 `bindable_retry`，不计入
可推送的 waiting。超过 30 天仅转为 `expired_unconfirmed`，不删除队项或本地
读/藏值。同步设置的显式恢复以摘要 `account_generation` 复核当前账号，在事务内
按文章**当前**读/藏目标创建新操作 ID，重新分为 bound/local-only/bindable；
缺文章仍保留过期，恢复不等于远端确认。换号/断开会放弃旧状态队列，确认界面
说明本地直连读/藏保留但不会跨账号自动补推。决定见
[队列生命周期](../.agents/notes/implemented/architecture/2026-11-03-同步队列生命周期与本地意图.md)与
[GR 权威集合回补与过期意图恢复](../.agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md)。

方向锁定测试：`src-tauri/src/sync/greader_pull.rs` /
`src-tauri/src/sync/fever_pull.rs` 的 `#[cfg(test)]`（政策格逐格锁死，含
「政策被翻转必红」的判别力声明）；端到端锁定见 §6 测试索引。

## 3. 同文副本读状态传播政策

**保持现状：读状态向同文副本跨源传播（read 广播）。**
来源：核心一致性重构路线第 6 条——owner 不在场时按保守默认处置（保持现状、
显式记录为政策并同步进用户可见文档）；是否改为「布局隔离优先」由 owner 后续
决定，同文建模分离落地策略开关时不预设结论。决策背景见
[核心一致性重构路线](../.agents/notes/implemented/architecture/2026-10-05-核心一致性重构路线.md)。

现状语义（消费点 `push.rs` 的 `plan_push`）：

- `read` 动作推送对象 = 绑定 entry + 全部已记账同文副本 entry
  （双端动机：Read You 等客户端不去重，桌面读完一篇，手机上另一源的副本
  也必须已读，否则同一篇在另一源里重新冒出未读）；
- `unread` / `star` / `unstar` 只推绑定 entry 本身，不广播；
- 全量合并路径：跨源副本记账（`add_article_dup_entry`）+ 远端已读即标读
  （read-anywhere-wins）；跨源副本的**未读**永不落地（`merge_pulled_entry`
  的 is_own 判定 + `merge_remote_status` 的 same-feed 判定）。

## 4. 增量游标语义

- **GR（时间游标）**：本轮游标候选在 id 列举**开始前**取（起点墙钟），
  主列举、正文块、绑定与权威集合/回补的失败计数全零才尝试写入
  `last_sync_ts`。正文合并逐语句提交，块内失败保留已提交条目，旧游标保留以便
  下轮幂等重拉；不能把此路径称为整页原子回滚。起点候选避免使用拉取结束时间
  扩大窗口，但不把发布时间变成变更时间，也不消除严格 `>` 的同秒边界。
  未读/收藏的缺身份和缺正文由完整集合回补；已读未收藏的晚到旧文仍可能需要 full。
- **Fever（条目 id 游标）**：`since_id` 向新分页，非空短页继续到空页。
  `last_sync_entry_id` 只在连续成功的增量页或从顶部连续走下的历史页事务内
  推进，与页数据同 commit；后页失败不撤销先前成功页。`with_ids` 补齐、
  Pending 续取页与失败页不推动它，避免点查大 ID 跳过未拉区间。历史 checkpoint
  同页提交，失败/预算到达保留续取点；时间戳只在零失败且历史已完成时维护，
  供切回 GR 后首拉使用。

## 5. 服务端已知实现差异

以下均为外部源码查阅时点事实（见文首声明），每条附本仓侧的应对：

- **Miniflux（GR 兼容端点）**：把 `ot` 映射为 `published_at > ot`
  （严格大于，过滤列是**发布时间**而非变更时间）。固定出处为
  [handler.go](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/googlereader/handler.go)
  的 `AfterPublishedDate` 与
  [entry_query_builder.go](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/storage/entry_query_builder.go)
  的 `e.published_at > $N`。起点候选不能保证旧发布时间条目进入窗口，回补与剩余边界见 L1/L7。
- **FreshRSS（GR 兼容端点，ot 列事实）**：固定
  [greader.php](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/p/api/greader.php)
  把 `setMinDate(ot)` 与 `setMinModifiedDate(ot)` 以 OR 组合；
  [EntryDAO.php](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/app/Models/EntryDAO.php)
  前者展开为 `id >= ot×1000000`，后者为 `lastModified >= ot`，不是 `date >= ot`。
  读/藏写操作更新 `lastUserModified`，因此 ot 不能完整发现这些状态变化。
  客户端以 ot=0 获取完整未读/收藏集合，避免把普通条目窗口当状态变更流。
- **Bazqux（GR 兼容端点，前瞻，未接入）**：主列举（`s=reading-list` + ot）
  不兼容——官方 API 文档对 ot 给出 Warning 明言勿用，推荐的同步方式即 xt 流
  （未读流）形态。应对：本仓未接入 Bazqux；接入前主列举须按未读差分同构改造
  （未读差分已与该推荐形态同构），ot 窗口语义不能假设成立。接入前置改造
  清单另含一条边界——**退订残留 × 未读差分误标**：Bazqux 保留退订源的历史
  条目，退订后本地残留的未读绑定行，其远端条目退出未读集合但 contents 按
  id 仍可取，「未读差分 + 存在性校验」会将其误标为已读（§6-L9 的具体形态）；
  接入前必须先落退订后的本地行清理/排除口径（如差分域剔除已退订 feed 的
  绑定行）。
- **Miniflux（Fever 端点）**：`{base}/fever/`；按 URL 去重 entry（Fever
  对账双向性的前提）；**items 端点支持 `max_id` 向更旧条目翻页（历史回溯，
  重复直到返回空数组）**——来源 `internal/fever/handler.go` 约 227-267 行
  （2026-10 源码查阅时点），**max_id 页为 `ORDER BY id DESC`**（与 FreshRSS
  同序；夹具里的升序页只是泛化鲁棒性对照，不是 Miniflux 事实）。
  应对：客户端已实现 `items_before` 循环（§1「历史回溯」行），游标取页内
  最小 id，与页内顺序无关。固定
  [handler.go](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/fever/handler.go)
  的 saved/unsaved 已是条件绝对赋值，而 `response.go` 仍报告 api_version 3；
  不能用版本号区分所有历史 toggle 与绝对赋值实现。客户端保守读前探测兼容两者，
  探测与写入之间的并发窗口仍没有服务端条件写保证。
- **FreshRSS（Fever 端点）**：`/api/fever.php`，新版布局移至
  `/p/api/fever.php`；`api_key` **只读 POST form body**（`p/api/fever.php:172`），
  放 query 里一律无效；条目 `id` 以 PHP numeric-string 序列化为 JSON **字符串**
  （64 位十进制）；`max_id` 页为 `ORDER BY id DESC`；`getItems()` 先 `LIMIT 50`
  再由 `EntryBeforeDisplay` 扩展 hook 过滤——**hook 对 items 全分支（含
  with_ids）统一生效**，被丢弃条目对 Fever 整体不可见；**过滤后的非空短页
  不代表没有后页**，客户端分页必须持续到空页才停。应对：端点解析按候选
  404 顺延（`src-tauri/tests/fever_freshrss_e2e.rs`）；id 字符串形态由
  `FeverId` 按十进制解析；短页与 hook-含-with_ids 语义由 `fever_compat_e2e`
  的「LIMIT 后过滤」夹具锁定（「列表覆盖不到但按 id 可取」只作合成用例标注）。
- **mock_greader（测试替身）**：ids 路由自 2026-10-31 起实现为
  `published_at > ot`（严格大于，与真实 Miniflux 对齐，见
  `tests/mock_greader.rs` 头注）。测试锁定客户端在该时间模型下的窗口、幂等、
  失败守卫与集合回补；FreshRSS 认证夹具也使用这个时间模型，不能外推为其实际 ot 过滤。

**版本验证声明（复述，见文首）**：以上是固定源码与受控夹具事实，
Miniflux / FreshRSS 的具体部署版本未逐一实机验收，不能当作已验收部署清单。

## 6. 已知限制与测试索引

已知限制：

- **L1 时间窗口的剩余范围**：Miniflux 普通增量不能单独发现发布时间早于或
  落在严格边界上的晚收录条目。2026-11-05 起，light 对完整 unread/starred
  集合中缺身份或正文的 ID 做有界回补，故未读与收藏不再依赖 RSS 当前窗口或
  ot 才能入库（见 L7）。剩余是已读且未收藏、又没有本地行的旧条目，需手动/首连
  full；远端已删除的正文不能补造。所属 feed 元数据已消失的孤儿历史也不会凭
  全局 URL 猜归属，保留有限兼容记账与可见的绑定未完成信息。
- **L2 Fever 首同步深度（已解决；R1/R2/R3 补齐恢复、事务与错误传播语义）**：
  Fever 协议/当前 Miniflux/FreshRSS 支持 `items&max_id` 向更旧条目翻页；客户端
  已实现历史回溯（OPT-005）：首连/full 同步循环取尽保留历史（含已读非收藏），
  增量走 `since_id`。历史状态为显式三态（Unknown/Pending/Complete；缺失/损坏
  不当完成），每轮至多 200 页（1 万条），到达/失败保留 Pending=可重拉游标，
  下一次同步（含**自动 light**）自动续取，只有走到空页才 Complete——不是永久
  上限，也不是手动 full 才能补。每页数据与该页游标同事务（失败整页回滚、内存
  统计/maps/seen 回滚）；**R3：map 读取失败（初次或页回滚后重建）即终止本轮、
  保留 Pending/已确认 since，退空映射继续的伪确认路径已封死**。`max_id` 严格
  `<` 边界外的 `i64::MAX` 条目由 `with_ids(MAX)` 顶覆盖。剩余边界是**服务端
  自身保留策略**（例如服务端清理过旧条目），客户端无法凭空补出服务端已删除的
  条目。实现与锁定：`src-tauri/src/fever.rs` 的 `items_before` 与 `chunk()`
  读取硬界、`src-tauri/src/sync/fever_pull.rs` 的页事务三段式与 map 错误传播、
  `src-tauri/src/db/sync_map.rs` 的三态 helpers、
  `src-tauri/tests/fever_compat_e2e.rs`。
- **L3 GR 单向的对称缺口**：远端「显式标未读」不经 GR 轻量对账回流；
  该方向变更只能等全量同步（同源合并路径的 `accept_unread`）或由本端
  push 覆盖。跨源副本的未读在任何路径都不落地（§3）。
- **L4 kept-unread 不识别**：GR `kept-unread` 标签（`user/-/state/com.google/kept-unread`）
  无识别/消费方（OPT-017F 已删除仅剩定义的常量），「保持未读」意图按普通未读处理。
- **L5 Fever 截断不可检测**：unread/saved 集合是单请求全集响应，无
  continuation 可校验；GR 侧的截断检测纪律在 Fever 协议上无对应
  机制（依赖服务端响应完整性）。
- **L6 GR 响应完整性（2026-11-05 扩展到普通 contents 的 ID 覆盖）**：GR 的
  `subscription/list`、`tag/list`、`stream/items/ids`、`item_contents` 返回
  200 但缺 `subscriptions` / `tags` / `itemRefs` / `items` 时，现均映射为
  `protocol`，不得当空集合；普通 `item_contents` 必须覆盖全部唯一请求 ID，
  空/短响应、重复或无关 ID 不能代替缺失目标，额外 ID 不参与合并，失败按块
  保留游标。ACK 专用读回同样按 ID 覆盖记录完整性，但允许返回未覆盖标记，
  由目标判定落 `retry_wait/verification`，不推断删除。非法 `itemRefs.id` 与
  非法 continuation 同样失败关闭并保留游标。
  continuation 另外记录已见值、要求严格前进并限制 200 页；重复数字不会无限
  分页。订阅绑定失败会写账号级持久 marker，独立 states 命令在首个 HTTP 前
  即阻断条目/权威集合请求与游标推进。marker 只在订阅遍历完整、所有远端订阅
  （含已有 `origin='local'` 匹配源）都完成 remote_id 绑定时清除；目录失败、
  feed/分类墓碑未知、插入失败和提前 break/continue 均保持 incomplete。
  例外仅为 known tombstone（`feed_url_tombstoned == Ok(true)`）的预期 continue：
  该源是用户本机主动删除，不要求重新绑定。
  新 feed 的 insert+bind 使用同一事务，绑定失败不留 remote 孤儿；历史
  `origin='remote'` 未绑定行仅当当前远端 URL 集已不含它时清理，local 源不受影响。
  itemRefs 为空但 continuation 合法也是 protocol 结构错误，主列表/read/starred
  都保留游标。
  HTTP 200 的 JSON 解码/schema 错误也统一为 `protocol`（传输/读体失败仍是
  `network`）。read/starred 权威集合改为字段级独立对账：单侧失败只跳过该侧；
  主列表/正文失败不再连带跳过另一成功集合。严格夹具见 `greader_compat_e2e`。
- **L7 GR 权威集合回补（2026-11-05）**：light 对完整 unread、starred 集合中
  缺身份或正文的 ID 分别回补，100 ID/请求、批内并发至多 3、每次 pull 每集合
  至多 2000 个唯一 ID。候选按数值排序，以 `{generation,last_id}` 持久轮转；
  本次预算内请求和合并全部成功才保存进度，空正文/缺 feed 会占预算但不会永远
  堵住尾部。预算到达本身不是错误，剩余下轮继续；原 states 订阅发现后最多
  再 pull 一次，故发生重拉的同一 states 轮每集合最多两份预算。
  合并复用精确 ID/同 feed URL 绑定与字段守卫，回补明确 unread 不覆盖已有
  本地已读；新条目可以按默认未读落地。请求/ID 覆盖/合并失败保留该集合进度及
  主游标，该字段不对账；进度读写或正文预过滤 DB 错误终止整轮。GR 已成功提交
  的条目不回滚，重试仍幂等。决定见
  [GR 权威集合回补与过期意图恢复](../.agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md)。
- **L8 Fever `as=unread` 为实现扩展（2026-11-01 登记）**：官方 Fever 规范的
  `mark` 写操作只定义 `as=read|saved|unsaved`，「恢复未读」的原语义是
  `unread_recently_read=1` 配合已读集合；本客户端使用的 `mark=item&as=unread`
  是 FreshRSS 与 Miniflux 各自实现的扩展（两者均支持，已核对 FreshRSS
  Fever API 文档与 Miniflux `internal/fever/handler.go`）。§1「取消标读
  （推送）」行按此口径理解；若未来接入仅按原规范实现的 Fever 服务端，
  该推送路径需按 `unread_recently_read` 收敛。
- **L9 未读集合「语义级截断/空集谎言」不可检测（2026-11-01 登记）**：wire 层
  截断（continuation 未耗尽 / 请求失败）由完整性守卫结构性拦截；但一份
  **格式完好而语义不完整**的未读集合（服务端缺陷把未读条目漏出 `xt=read`
  流，或 §5 Bazqux 的退订残留形态）客户端无法识别——这些条目会经「未读差分
  + contents 存在性校验（条目仍存在）」被误标为已读。与旧 read 全量对账的
  失败方向相反：旧形态缺条 = 漏标（无害），差分形态缺条 = 误标。缓解：xt=
  流是三家服务端官方支持的同步原语（Bazqux 官方推荐形态）；真实服务端实机
  验收未完成前，该残余风险不可关闭。

测试索引（对账方向的锁定分层）：

- 2026-11-05 回补与显式恢复：`greader_compat_e2e.rs` 的晚到旧未读、正文
  缺口、2001 ID 跨重开轮转、缺 feed 前缀与错 ID 失败；`db/sync_queue.rs` /
  `commands/sync.rs` 的 expired recovery 测试及 `sync-recovery.test.mjs` 的
  真实挂载代际/重复点击/旧摘要失败保护。各次实际执行范围与原始红绿记录在
  [归属 Note](../.agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md)，不把分批通过拼成未执行的单次全套数量。

- 政策格锁定（单元）：`greader_pull.rs` / `fever_pull.rs` 的
  `#[cfg(test)]`——GR 单向不复活、GR/Fever 星标双向、Fever 双向复活、
  双侧字段 pending + 字段版本守卫（含「本地碰过并改回原值也不放行旧快照」
  与两字段彼此独立）；`conflict_policy.rs` 政策格取值锚。
- 端到端（既有）：`sync_phases_e2e.rs`（远端陈旧已读经轻量对账收敛；
  pending 守卫）、`dual_client_e2e.rs`（跨源副本未读不复活；副本广播；
  pending 防乒乓）、`star_reconcile_truncation_e2e.rs`（GR starred 截断
  → 对账中止，失败守卫）、`pull_cursor_e2e.rs`（分块/id 列举失败游标
  不推进）、`dedup_sync_e2e.rs`（同源判定矩阵/墓碑）。
- 端到端（未读差分等价性，2026-11-01）：`unread_diff_e2e.rs`（远端新标
  已读 → 本地落已读；本地已读幂等 0 行；远端已删除的未读条目不误标；
  未读集合请求失败整轮跳过差分、恢复后收敛）。
- 端到端（OPT-007 字段版本与拉取可靠性，屏障时序）：`pull_field_versions_e2e.rs`
  （请求在途本地 star + 推送确认清队 → 旧远端 unstar 快照不回滚；快照早于
  最早 HTTP；字段彼此独立；新入库后同轮后续保护；GR map 构建失败不空映射
  假成功、批失败重建 maps 后重拉不跳过）。
- 端到端（Fever 严格夹具，OPT-005）：`fever_compat_e2e.rs`（字符串长 id 十进制
  往返、JSON 数字形态、125 篇历史全可达/幂等、增量只走 `since_id`、分页失败可
  重试、游标不前进快速失败、重叠页去重、with_ids 补齐、缺字段/非法 id/非法 CSV
  显式错误、mark 逐条单 id）。
- 端到端（Fever R1 恢复语义）：`fever_compat_e2e.rs`（LIMIT 后 hook 过滤的非空
  短页继续翻页、full 第二页失败→自动 light 续取、增量失败游标不被 with_ids
  大 id 推动、`i64::MAX` 已读非收藏顶覆盖、200 页预算→下轮 light 续取
  10050 条、17 MiB 响应被 16 MiB 界限拒绝）。
- 端到端（Fever R2 事务/状态/硬界）：`fever_compat_e2e.rs`（真 DB trigger：
  状态键写失败→不推 since、light 重试成功；页内 remote-100 INSERT 失败→整页
  回滚+统计回滚+Pending 保留→下轮补回且不提前 Complete；旧库 since 无状态→
  自动补旧历史；损坏状态报错不改游标；chunked 无长度与 brotli 解压膨胀都被
  16 MiB 读取硬界拒绝；hook 含 with_ids；合成列表遗漏单列标注）。
- 端到端（Fever R3 map 错误传播）：`fever_compat_e2e.rs`（真实列类型错误两反例：
  ① 初始 `sync_match_maps` 读取失败（url_norm=BLOB）→ 立即终止本轮，0 条目、
  状态保持 Pending、since/ts 不推进、无 items/with_ids 请求；② 页失败后重建
  读取失败（页 1 成功提交的 trigger 污染列）→ 终止整轮，since 停在连续成功
  范围 100、状态保持 Pending、无顶部回溯伪确认）。
- 端到端（live，`#[ignore]`，CI 外人工执行）：`fever_sync_live_e2e.rs`、
  `fever_live_e2e.rs`（Fever 真实后端的集合/对账往返）。

## 7. GReader 鉴权与分类契约（OPT-004 补记）

本节超出主线「状态同步对账」范围（订阅层与认证层），为回溯便利列入本文件。
三列口径同 §1：协议/服务端支持来自**固定上游源码**（FreshRSS `219eaf58` 的
`p/api/greader.php`、Miniflux `internal/googlereader/middleware.go`；时点 2026-10-08，
见 `tmp/optimization-20261008/upstream/`）；实现列是 FluxReader 代码真实行为；
验证列只覆盖**测试替身**，不代表真实服务端实测。

| 维度 | 协议/服务端支持 | 本客户端已实现 | 验证（夹具，非真实服务端） |
|---|---|---|---|
| 请求认证 | Miniflux：GET 读 `Authorization: GoogleLogin auth=<auth>`，POST **只读**表单 `T`；FreshRSS：所有请求读 `Authorization`（设用户上下文），自身忽略 `T` 的宽容分支除外 | 所有请求（GET/POST）统一携带 Authorization；POST 表单 `T` 用 action token | 两个严格夹具：Miniflux 形态 POST 只认 `T=auth`；FreshRSS 形态缺 Authorization 一律 401 |
| action token | 两端都有 `/reader/api/0/token`：Miniflux 返回登录 auth 本身；FreshRSS 返回 `str_pad(sha1(salt+user+apiPasswordHash), 57, 'Z')`（与 auth 不同） | 首写前 `GET /token`，`OnceCell` 单次缓存；Miniflux/FreshRSS 两种返回值都接受 | `greader_compat_e2e.rs`：token 仅取一次；FreshRSS 写请求 `T` 必须等于 `/token` 返回的另一字符串 |
| token 失败边界 | 协议未规定 | **仅 404 回退**用登录 auth 当 `T`；401/403/5xx/空体/网络失败如实报错，不回退不猜 URL | 夹具注入 401/500/空体/404/拒连，逐项锁定「报错 vs 回退」方向 |
| 分类识别 | Miniflux tag `{id,label,type:"folder"}`；FreshRSS tag 只有 `{id,type:"folder"}`（无 label）、subscription category 只有 `{id,label}`（无 type） | `greader::category_name` 单点：type folder 或「type 缺失 + label 前缀 id」；label 优先，缺则取 `user/.../label/` 后缀（中文/斜杠原样保留）；state tag 与 `type:"tag"` 一律不成目录 | 严格夹具经 `sync::feeds_phase` 验证：FreshRSS 形态目录/归属正确、state tag 不污染；Miniflux 形态回归不变 |
| 写操作响应体 | 成功约定是文本 `OK`（FreshRSS `edit-tag`/`subscription/edit` 均 `exit('OK')`；Miniflux 同形态）；协议未规定错误体，实现可能回 `FAIL` | `post_form_text` 要求 200 **且** trim 后 == `OK`；FAIL/空体/其它正文 → 脱敏协议错误（不回显响应体），调用方不得按成功 prune 队列 | 严格夹具注入 `200+FAIL` / `200+空体`：直接客户端报错不假成功；`sync::push_states_now` 队列保留；` OK \n` 容忍、清除注入后成功路径仍 prune |
| 写分类 stream id | `subscription/edit` 的 `a` 是 label stream id（`user/-/label/<名>` / `user/<user>/label/<名>`）；FreshRSS 按前缀解析，裸名解析为空 → 落默认分类 | `edit_subscription` 的 `dest_label`（用户裸目录名）无条件前置一次为 `user/-/label/<名>` 再交 `reqwest.form` 编码；名字本身含前缀字面量不误判 | FreshRSS/Miniflux 严格夹具**实际移动分类**并可断言（中文/斜杠名、字面量前缀名）；既有 `sync_gap_repro_e2e` 断言更新为真实 wire 形态（未减断言） |

已知限制：真实 Miniflux/FreshRSS 部署版本未实机验收；夹具只锁定客户端契约与
固定源码的实现形态（`mock_greader.rs` 补 `/token` 路由并按真实 wire 解析 `a`，
不校验认证；认证严格校验在 `greader_compat_e2e.rs` 的独立夹具）。
