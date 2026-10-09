# 同步协议 × 服务端兼容矩阵（Google Reader / Fever）

本文是同步状态语义的**用户可见对照表**。政策的代码单点在
[`src-tauri/src/sync/conflict_policy.rs`](../src-tauri/src/sync/conflict_policy.rs)
（每格政策的常量定义、选择理由、历史依据都在那里），本文与其同步维护——
**改政策 = 改政策点 + 改本文 + 改对应锁定测试**。政策为什么以常量形式存在、
三类事实为什么要分列，见
[同步冲突政策单点](../.agents/notes/implemented/architecture/2026-10-07-同步冲突政策单点.md)。

**时点声明**：文中涉及外部服务端实现的事实（Miniflux / FreshRSS / 本仓
`src-tauri/tests/mock_greader.rs` 测试替身）来自源码查阅时点（2026-10），
**不代表部署服务的版本**；升级或更换服务端后须复核。

**版本验证声明（必读）**：本矩阵的「指定服务端版本已验证」列**只覆盖测试替身**
（`src-tauri/tests/mock_greader.rs` 的宽松形态，以及
`src-tauri/tests/greader_compat_e2e.rs` 的 Miniflux/FreshRSS 严格夹具、
`src-tauri/tests/fever_compat_e2e.rs` 的 FreshRSS/Miniflux 形态 Fever 严格夹具）——它们是
被自动化测试锁定的服务端形态。
**Miniflux / FreshRSS 的具体部署版本未逐一实机验收**；文中「协议/服务端支持」列
涉及外部服务端的事实来自源码查阅时点（2026-10），**不代表所有部署版本都已验收**。
升级或更换服务端后三者都须复核。三列必须分开——协议/服务端事实、本客户端实现现状、
实际验收过的版本不是一回事；把「本客户端未实现」写成「协议做不到」会掩盖可补齐的能力。

范围：状态与条目同步（push 队列 + pull 合并/对账）。订阅层（feeds 阶段的
订阅/分类增删改）不在本矩阵内。两协议共用同一条
`sync_queue → plan_push / exec_push` 推送管线与同一条 `merge_pulled_entry`
合并函数，协议差异集中在**对账方向**与**游标/条目获取能力**两处。

## 1. 协议 × 操作差异矩阵

三列口径：**协议/服务端支持** = 协议规范或当前参考实现（Miniflux main 等）
事实上的能力；**本客户端已实现** = FluxReader 当前代码真正落地的行为；
**指定服务端版本已验证** = 实际被测试/实机验收覆盖到的服务端形态。
某格为「客户端选择的策略」时，协议列不写、只在实现列写并标注。

| 维度 | 协议/服务端支持 | 本客户端已实现 | 指定服务端版本已验证 |
|---|---|---|---|
| 标读（推送） | GR `edit-tag`（`a=user/-/state/com.google/read`）按 entry id 批量；Fever `mark=item&as=read` **只接受单个 id**（逗号分隔无效，须逐个调用） | 绑定 entry + **全部同文副本**（read 广播政策，§3）；GR 多 entry 聚合单请求；Fever 逐 id 调用（同一 `plan_push`，协议无关） | mock_greader 替身锁定 GR 推送往返；Fever `mark=item` 仅模块头 curl 实证 + live 测试（`#[ignore]`，CI 外人工执行），无自动化版本验收 |
| 取消标读（推送） | GR `edit-tag` remove read；Fever `mark=item&as=unread`（单 id） | **仅绑定 entry**，不广播 | 同上一行（GR mock 锁定；Fever 仅实证/live） |
| 收藏 / 取消收藏（推送） | GR starred tag add/remove（显式 add-remove 语义，非 toggle）；Fever `mark=item&as=saved/unsaved`（单 id） | 仅绑定 entry | 同上传动路径（GR mock 锁定） |
| 轻量对账 · 读状态 | GR 权威集合只有「已读 id 集合」（`stream/items/ids?s=read`），**无显式「保持未读」信号**；Fever 只有 `unread_item_ids`（无已读集合） | **GR 单向 read-wins**（命中 → 本地已读；未命中不回写）；**Fever 双向权威**（unread 命中 → 本地未读可复活；未命中 → 本地已读）。**注：read-wins 是本客户端选择的冲突策略，不是 Google Reader 协议必然要求**；见 §2 | mock_greader 替身（read 集合路由）经自动化测试锁定；真实 Miniflux/FreshRSS 未逐一实机验收 |
| 轻量对账 · 星标 | GR starred stream / Fever `saved_item_ids` 都能给出「当前收藏 id 集合」，隐含「未命中 = 已取消」 | **双向权威**（命中 → 收藏；未命中 → 取消收藏）——两协议一致（同为客户端策略） | mock_greader；截断守卫由 `star_reconcile_truncation_e2e` 锁定 |
| 全量合并 · 读状态 | 协议本身不规定合并规则 | 与 Fever 共用 `merge_pulled_entry`：read-anywhere-wins（任何副本已读都接受）；unread 仅同源绑定 entry 接受（**跨源副本的未读不复活**）；跨源副本记账 + 远端已读即标读 | `dual_client_e2e` / `dedup_sync_e2e` 锁定客户端语义（无服务端版本维度） |
| 权威状态集合端点 | GR `stream/items/ids`（s=read / s=starred），continuation 分页；Fever `unread_item_ids` / `saved_item_ids`，单请求返回全集 | 两协议均拉全量集合用于轻量对账；GR 分页截断/中断按失败处理；Fever 响应**缺字段是协议错误**（不当空集合）、CSV 非法项显式报错 | mock_greader（含分页/截断形态；Fever 路由按 action 补对应字段）；Fever 严格形态（缺字段/非法 CSV）由 `fever_compat_e2e` 锁定；live 测试（`#[ignore]`）另行 |
| 条目 id 取值形态 | Fever：FreshRSS `fever.php` 的 `id` 是 PHP numeric-string（JSON **字符串**，如 `"1791440000000000"`），Miniflux 是 JSON 数字；GR `stream/items/ids` 的 `id` 是 64 位**十进制字符串**，`item_contents` 是长格式 tag（尾部 hex） | `FeverId` 两种形态都接受、一律按十进制（不猜 hex、不经 f64；非法/负值/溢出显式错误）；`greader::parse_item_id` 按协议形状（前缀 tag→hex；无前缀纯数字→十进制） | `fever_compat_e2e`（字符串长 id 全链路往返、JSON 数字形态、非法/溢出/负值/浮点报错）；`greader.rs` 单测（前缀 hex 含 a-f、无前缀十进制） |
| 条目获取 | GR `item_ids`(reading-list, ot) 分页列举 + `item_contents`（100 条/批）；Fever `items` 支持 `since_id`（向更新）**与 `max_id`（向更旧，历史回溯）**——当前 Miniflux `internal/fever/handler.go` 约 227-267 行与 FreshRSS `findEntries` 均支持 `max_id`，重复直到返回空数组 | GR 全量 id 列举 + 正文分块；Fever `items&since_id` 增量（50 条/页）+ `items_before(max_id)` 历史回溯 + `with_ids` 补齐权威集合缺正文条目（50 条/批）；历史页/with_ids 按 id 去重；**每页为独立短事务**（页数据+该页游标同 commit，任何行失败整页回滚、内存统计/maps/seen 一并回滚）；响应按 `chunk()` 逐块累计的 16 MiB 读取硬界（覆盖无 Content-Length/压缩膨胀） | mock_greader 替身锁 GR 条目获取（另按 action 补 Fever 宽松空集字段）；Fever 条目获取由 `fever_compat_e2e` 严格夹具自动化锁定（长 id/125 篇/去重/with_ids/页事务回滚/17 MiB 与 chunked·brotli 超限）；live 测试（`#[ignore]`）另行 |
| 历史回溯（向更旧翻页） | **Fever `items&max_id` 支持**（当前 Miniflux main/dev 与 FreshRSS `fever.php`：取 `id < max_id` 的最近 50 条，两家固定实现均 `ORDER BY id DESC`）；GR 无此概念（用 ot 时间窗口近似） | **已实现**：首连/full 同步 `items_before` 循环直到空页取尽保留历史（含已读非收藏）；游标取页内最小 id（与页内顺序无关）且必须严格减小；分页失败/游标不前进显式记 `report.errors` 并保留已拉进度。「历史是否仍被服务端保留另当别论」 | `fever_compat_e2e`（125 篇全可达、页序无关鲁棒性、分页失败重试、游标不前进快速失败、幂等与增量只走 since_id） |
| 历史状态与自动续取（OPT-005 R1/R2/R3） | 协议不规定（客户端策略） | 内部 settings `fever_history_state` 显式三态（无 schema 迁移）：缺失=**Unknown（绝不当完成**，旧库只有 since 也自动从顶部补旧历史）；`pending:<max_id>`=未完成；`complete`=已取尽；损坏=**显式错误且不动游标**。每页「页数据+checkpoint/since」同事务；「先初始化 Pending 成功才允许更大的 since 落库」；每轮至多 200 页（1 万条），预算/失败/游标不前进都保留 Pending 续取点、不推进完成时间；**含自动 light** 从 Pending 续取，只有走到空页才 Complete；手动 full 从顶部重放（幂等）。**R3：map（匹配映射）读取失败——初次构建或页回滚后重建——记 report 并立即终止本轮，保留 Pending/已确认 since，不执行后续 history/with_ids/完成时间；绝不退化成空映射继续**（空 feed 映射会让 merge 合法跳过却照常确认游标/写 Complete）。`max_id` 严格 `<` 漏掉的 `i64::MAX` 边界条目由 `with_ids(MAX)` 顶覆盖 | `fever_compat_e2e`（状态写失败不推 since→light 重试、页 INSERT 失败整页回滚+Pending 保留→下轮补回、Unknown 旧库自动补旧历史、损坏状态报错不改游标、full 失败→light 自动补齐、200 页预算→下轮续取 10050 条、i64::MAX 已读非收藏可达、R3 map 列类型错误两反例：初始读取失败无假 Complete/since/time、页失败后重建失败终止整轮） |
| 增量游标 | GR `ot` 为服务端时间过滤参数（过滤列随实现而异）；Fever `since_id` 为条目 id 过滤 | GR `last_sync_ts`（unix 秒）**起点候选**（id 列举开始前取）+ 幂等合并 ⇒ 无漏无重；仅本轮「窗口拿全」（id 列举 + 分块零失败）才推进；Fever：`since_id` 分页**非空页持续到空页**（服务端可先 `LIMIT 50` 再经 hook 过滤产生非空短页，短页不是结束）；`last_sync_entry_id` **只由成功提交的页事务推进**（增量页或顶部连续页；与页数据同 commit，失败回滚不推进）——with_ids 补齐、Pending 续取页与失败页都不推动（防跳过未拉区间）+ `last_sync_ts`（仅零失败且历史无未完成时写，供切换回 GR 后的首拉） | `pull_cursor_e2e` + mock_greader；Fever 增量与游标隔离由 `fever_compat_e2e`（LIMIT 后过滤短页继续、增量第二页失败不被 with_ids 大 id 推动、`since_id=上次 max` 起）；真实服务端过滤列差异见 §5（源码查阅，未实机验收） |
| 时间过滤列 | GR `ot` 语义随服务端实现；Fever 无时间过滤（条目 id 单调递增游标） | 不依赖过滤列（起点候选 + 幂等合并） | mock 为 `changed_at >= ot`（含边界，自动化锁定）；真实 Miniflux main 为 `published_at > ot`（源码查阅，未实机验收） |
| 「失败 ≠ 空集合」守卫 | 协议本身不规定（纯客户端安全策略） | read/starred 集合任一拉取失败 → 本轮对账整体跳过（`greader_pull.rs` C-1 段）；unread/saved 任一失败 → `reconcile_ok = false`，整段对账跳过（`fever_pull.rs`） | GR：`star_reconcile_truncation_e2e`；Fever：逻辑覆盖 + live 测试 |
| pending 保护 | 协议本身不规定（纯客户端防乒乓策略） | `pending_ids`（sync_queue 未推送 read/unread/star/unstar）命中的条目整行跳过对账——两协议共享，见 §2 | `sync_phases_e2e` / `dual_client_e2e` |

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
| GR × 读状态 | `GR_READ_DIRECTION` | 单向 read-wins（**客户端选择**） | GR 只能拿到「已读 id 集合」，没有「明确保持未读」信号；把「不在已读集合」解释成「刚取消已读」会成批翻回历史条目。远端显式标未读由 push 段承担 |
| GR × 星标 | `GR_STAR_DIRECTION` | 双向权威（**客户端选择**） | 收藏低频强意图、取消是显式动作；截断集合的误判由失败守卫拦截（`star_reconcile_truncation_e2e`） |
| Fever × 读状态 | `FEVER_READ_DIRECTION` | 双向权威（unread 集合，**客户端选择**） | Fever 拿不到已读集合，「未命中 = 已读」是唯一信号，不双向则两端读状态都无法收敛。前提：Miniflux 按 URL 去重 entry，Fever 视角无跨源副本；误判代价由失败守卫 + pending 保护兜底 |
| Fever × 星标 | `FEVER_STAR_DIRECTION` | 双向权威（**客户端选择**） | 同 GR × 星标（saved 集合） |

共享守卫（两协议一致，属政策的一部分）：

- **pending 保护**：本地有未推送变更的条目跳过远端快照对账（防「刚标读/
  刚收藏」被陈旧快照回滚，防乒乓）。消费点：两个 reconcile 循环开头的
  `pending_ids.contains → continue`；共享查询 `db::sync_match_maps`。
- **「失败 ≠ 空集合」**：权威集合拉取失败（网络/服务端错误、分页截断）时
  整轮对账跳过，绝不把失败当成「远端什么都没有」做双向回写。

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
  拉取零失败才写入 `last_sync_ts`。论证（起点候选 + 合并幂等 ⇒ 无漏无重）
  见 `greader_pull.rs` 的注释；失败时保持旧游标、下一轮重拉同一窗口
  （失败守卫族）。边界语义按「服务端 `>=` 过滤」设计；服务端若为严格 `>`，
  同一秒内先列举后变更的条目会漏进下一轮（幂等合并下安全，但依赖后续
  轮次/全量补齐）。
- **Fever（条目 id 游标）**：`since_id` 升序分页；`last_sync_entry_id` 按
  已合并条目 max 推进（只计已合并，本就安全）；时间戳游标并行维护仅服务
  于「切换回 GR 后的首拉窗口」。

## 5. 服务端已知实现差异

以下均为外部源码查阅时点事实（见文首声明），每条附本仓侧的应对：

- **Miniflux（GR 兼容端点）**：把 `ot` 映射为 `published_at > ot`
  （严格大于，过滤列是**发布时间**而非变更时间）。应对：起点候选 +
  幂等合并不依赖过滤列；「晚到旧文章」风险见 §6-L1。
- **Miniflux（Fever 端点）**：`{base}/fever/`；按 URL 去重 entry（Fever
  对账双向性的前提）；**items 端点支持 `max_id` 向更旧条目翻页（历史回溯，
  重复直到返回空数组）**——来源 `internal/fever/handler.go` 约 227-267 行
  （2026-10 源码查阅时点），**max_id 页为 `ORDER BY id DESC`**（与 FreshRSS
  同序；夹具里的升序页只是泛化鲁棒性对照，不是 Miniflux 事实）。
  应对：客户端已实现 `items_before` 循环（§1「历史回溯」行），游标取页内
  最小 id，与页内顺序无关。
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
- **mock_greader（测试替身）**：ids 路由实现为 `changed_at >= ot`（边界
  含入）。与真实 Miniflux 的差异意味着**测试锁定的是客户端语义（起点候选
  + 幂等合并 + 失败守卫），不是任何特定服务端的过滤列**。

**版本验证声明（复述，见文首）**：以上「协议/服务端支持」事实来自源码查阅
时点（2026-10），**除 mock_greader 测试替身（自动化锁定）外，Miniflux /
FreshRSS 的具体部署版本未逐一实机验收**——本列不能当作「已验收版本清单」。

## 6. 已知限制与测试索引

已知限制：

- **L1 游标变更时间假设**：增量窗口不能单独保证「晚到的旧文章」被拉到
  ——真实 Miniflux 把时间过滤应用于发布时间，一篇发布时间早于游标、但
  晚进入订阅源的文章不会命中增量窗口。兜底：手动/首连的全量对账（full）+
  失败守卫保证失败窗口重拉；彻底解法待服务端 × 版本兼容实测。
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
- **L4 kept-unread 不识别**：GR `kept-unread` 标签常量已定义但无消费方，
  「保持未读」意图按普通未读处理。
- **L5 Fever 截断不可检测**：unread/saved 集合是单请求全集响应，无
  continuation 可校验；GR 侧的截断检测纪律在 Fever 协议上无对应
  机制（依赖服务端响应完整性）。

测试索引（对账方向的锁定分层）：

- 政策格锁定（单元）：`greader_pull.rs` / `fever_pull.rs` 的
  `#[cfg(test)]`——GR 单向不复活、GR/Fever 星标双向、Fever 双向复活、
  双侧 pending 保护；`conflict_policy.rs` 政策格取值锚。
- 端到端（既有）：`sync_phases_e2e.rs`（远端陈旧已读经轻量对账收敛；
  pending 守卫）、`dual_client_e2e.rs`（跨源副本未读不复活；副本广播；
  pending 防乒乓）、`star_reconcile_truncation_e2e.rs`（GR starred 截断
  → 对账中止，失败守卫）、`pull_cursor_e2e.rs`（分块/id 列举失败游标
  不推进）、`dedup_sync_e2e.rs`（同源判定矩阵/墓碑）。
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
