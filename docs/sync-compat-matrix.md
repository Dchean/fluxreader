# 同步协议 × 服务端兼容矩阵（Google Reader / Fever）

TASK-112（二阶段④）。完成标准：「不同协议行为有明确说明」。本文面向维护者，
是同步状态语义的**用户可见对照表**；政策的代码单点在
`src-tauri/src/sync/conflict_policy.rs`（每格政策的常量定义、选择理由、历史依据
都在那里），本文与其同步维护——改政策 = 改政策点 + 改本文 + 改对应锁定测试。

**时点声明**：文中涉及外部服务端实现的事实（Miniflux / FreshRSS / 本仓
`tests/mock_greader.rs` 测试替身）来自 dev 分支源码查阅时点（2026-10），
**不代表部署服务的版本**；升级或更换服务端后须复核。

**版本验证声明（必读）**：本矩阵的「指定服务端版本已验证」列**只覆盖测试替身**
`tests/mock_greader.rs`——它是唯一被自动化测试锁定（GR 路由）的服务端形态。
**Miniflux / FreshRSS 的具体部署版本未逐一实机验收**；文中「协议/服务端支持」列
涉及外部服务端的事实来自 dev/main 分支源码查阅时点（2026-10），
**不代表所有部署版本都已验收**。升级或更换服务端后三者都须复核。
（审计 P2-8：三列必须分开——协议/服务端事实、本客户端实现现状、实际验收过的版本，
三者不是一回事；把「本客户端未实现」写成「协议做不到」会掩盖可补齐的能力。）

范围：状态与条目同步（push 队列 + pull 合并/对账）。订阅层（feeds 阶段的
订阅/分类增删改）不在本矩阵内。两协议共用同一条
`sync_queue → plan_push / exec_push` 推送管线与同一条 `merge_pulled_entry`
合并函数，协议差异集中在**对账方向**与**游标/条目获取能力**两处。

## 1. 协议 × 操作差异矩阵

三列口径：**协议/服务端支持** = 协议规范或当前参考实现（Miniflux main 等）
事实上的能力；**本客户端已实现** = FluxReader 当前代码真正落地的行为；
**指定服务端版本已验证** = 实际被测试/实机验收覆盖到的服务端形态。
某格为「客户端选择的策略」时，协议列不写、只在实现列写并标注（审计 P2-8①）。

| 维度 | 协议/服务端支持 | 本客户端已实现 | 指定服务端版本已验证 |
|---|---|---|---|
| 标读（推送） | GR `edit-tag`（`a=user/-/state/com.google/read`）按 entry id 批量；Fever `mark=item&as=read` **只接受单个 id**（逗号分隔无效，须逐个调用） | 绑定 entry + **全部同文副本**（read 广播政策，§3）；GR 多 entry 聚合单请求；Fever 逐 id 调用（同一 `plan_push`，协议无关） | mock_greader 替身锁定 GR 推送往返；Fever `mark=item` 仅模块头 curl 实证 + live 测试（`#[ignore]`，CI 外人工执行），无自动化版本验收 |
| 取消标读（推送） | GR `edit-tag` remove read；Fever `mark=item&as=unread`（单 id） | **仅绑定 entry**，不广播 | 同上一行（GR mock 锁定；Fever 仅实证/live） |
| 收藏 / 取消收藏（推送） | GR starred tag add/remove（显式 add-remove 语义，非 toggle）；Fever `mark=item&as=saved/unsaved`（单 id） | 仅绑定 entry | 同上传动路径（GR mock 锁定） |
| 轻量对账 · 读状态 | GR 权威集合只有「已读 id 集合」（`stream/items/ids?s=read`），**无显式「保持未读」信号**；Fever 只有 `unread_item_ids`（无已读集合） | **GR 单向 read-wins**（命中 → 本地已读；未命中不回写）；**Fever 双向权威**（unread 命中 → 本地未读可复活；未命中 → 本地已读）。**注：read-wins 是本客户端选择的冲突策略，不是 Google Reader 协议必然要求**（审计 P2-8②）；见 §2 | mock_greader 替身（read 集合路由）经自动化测试锁定；真实 Miniflux/FreshRSS 未逐一实机验收 |
| 轻量对账 · 星标 | GR starred stream / Fever `saved_item_ids` 都能给出「当前收藏 id 集合」，隐含「未命中 = 已取消」 | **双向权威**（命中 → 收藏；未命中 → 取消收藏）——两协议一致（同为客户端策略） | mock_greader；截断守卫由 `star_reconcile_truncation_e2e` 锁定 |
| 全量合并 · 读状态 | 协议本身不规定合并规则 | 与 Fever 共用 `merge_pulled_entry`：read-anywhere-wins（任何副本已读都接受）；unread 仅同源绑定 entry 接受（**跨源副本的未读不复活**）；跨源副本记账 + 远端已读即标读 | `dual_client_e2e` / `dedup_sync_e2e` 锁定客户端语义（无服务端版本维度） |
| 权威状态集合端点 | GR `stream/items/ids`（s=read / s=starred），continuation 分页；Fever `unread_item_ids` / `saved_item_ids`，单请求返回全集 | 两协议均拉全量集合用于轻量对账；GR 分页截断/中断按失败处理（P2-1） | mock_greader（含分页/截断形态）；Fever 集合端点仅 live 测试（`#[ignore]`） |
| 条目获取 | GR `item_ids`(reading-list, ot) 分页列举 + `item_contents`（100 条/批）；Fever `items` 支持 `since_id`（向更新）**与 `max_id`（向更旧，历史回溯）**——当前 Miniflux `internal/fever/handler.go` 约 227-267 行支持 max_id，重复直到返回空数组 | GR 全量 id 列举 + 正文分块；Fever 仅 `items&since_id` 增量（50 条/页升序）+ `items_recent` 首种子（最近 50 条）+ `with_ids` 补齐权威集合缺正文条目（50 条/批）；**未实现历史回溯（max_id 向更旧翻页）** | mock_greader 替身锁 GR 条目获取；Fever items 由 live 测试（`#[ignore]`）+ curl 实证，无自动化版本锁定 |
| 历史回溯（向更旧翻页） | **Fever `items&max_id` 支持**（当前 Miniflux main/dev，见 `handler.go` 约 227-267 行）；GR 无此概念（用 ot 时间窗口近似） | **未实现**（Fever 仅 since_id 增量；GR 用 ot 游标窗口）。「历史是否仍被服务端保留另当别论」 | 无（协议事实来自源码查阅时点，非实机验收；能力记录点见 `src-tauri/src/fever.rs` 模块头） |
| 增量游标 | GR `ot` 为服务端时间过滤参数（过滤列随实现而异）；Fever `since_id` 为条目 id 过滤 | GR `last_sync_ts`（unix 秒）**起点候选**（id 列举开始前取）+ 幂等合并 ⇒ 无漏无重；仅本轮「窗口拿全」（id 列举 + 分块零失败）才推进（TASK-068/069/097）；Fever `last_sync_entry_id`（本轮已合并条目的 max，恒写）+ `last_sync_ts`（仅零失败时写，供切换回 GR 后的首拉） | `pull_cursor_e2e` + mock_greader；真实服务端过滤列差异见 §5（源码查阅，未实机验收） |
| 时间过滤列 | GR `ot` 语义随服务端实现；Fever 无时间过滤（条目 id 单调递增游标） | 不依赖过滤列（起点候选 + 幂等合并） | mock 为 `changed_at >= ot`（含边界，自动化锁定）；真实 Miniflux main 为 `published_at > ot`（源码查阅，未实机验收） |
| 「失败 ≠ 空集合」守卫 | 协议本身不规定（纯客户端安全策略） | read/starred 集合任一拉取失败 → 本轮对账整体跳过（`greader_pull.rs` C-1 段）；unread/saved 任一失败 → `reconcile_ok = false`，整段对账跳过（`fever_pull.rs`） | GR：`star_reconcile_truncation_e2e`；Fever：逻辑覆盖 + live 测试 |
| pending 保护 | 协议本身不规定（纯客户端防乒乓策略） | `pending_ids`（sync_queue 未推送 read/unread/star/unstar）命中的条目整行跳过对账——两协议共享，见 §2 | `sync_phases_e2e` / `dual_client_e2e` |

## 2. 冲突政策表（对账方向）

**全部四格都是本客户端选择的冲突策略，不是协议必然要求**（审计 P2-8②）：
Google Reader 规范没有规定「远端已读集合未命中时本地该怎样」，Fever 也没有规定
「saved 集合未命中即取消收藏」。协议只提供集合端点；方向语义由本客户端选定、
写成常量、被锁定测试锚住。特别是 **GR × 读状态的 read-wins 单向**
是客户端为避免「历史条目成批翻回未读」而做的取舍，不是 Google Reader 的要求。

政策格与代码常量一一对应（`conflict_policy.rs`；行级落地
`apply_read_by_policy` / `apply_star_by_policy`——两者返回 `AppResult<usize>`，
DB 写失败向上传播不再吞成 0，审计 P2-8③）：

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
来源：DEC-refactor-roadmap-20261005 第 6 条（原文：owner 不在场时按保守
默认处置——保持现状、显式记录为政策并同步进用户可见文档；是否改为
「布局隔离优先」由 owner 后续决定，同文建模分离落地策略开关时不预设结论）。

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
  见 `greader_pull.rs` 的 TASK-097 注释；失败时保持旧游标、下一轮重拉同一
  窗口（REQ-103 失败守卫族）。边界语义按「服务端 `>=` 过滤」设计；
  服务端若为严格 `>`，同一秒内先列举后变更的条目会漏进下一轮（幂等合并
  下安全，但依赖后续轮次/全量补齐）。
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
  （2026-10 源码查阅时点）。这是**服务端能力**；本客户端未实现该方向
  （见 §1「历史回溯」行与 §6-L2）。
- **FreshRSS（Fever 端点）**：`/api/fever.php`，新版布局移至
  `/p/api/fever.php`；`api_key` **只读 POST form body**（`p/api/fever.php:172`），
  放 query 里一律无效。应对：端点解析按候选 404 顺延
  （`tests/fever_freshrss_e2e.rs`）。
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
  晚进入订阅源的文章不会命中增量窗口（审计结论）。兜底：手动/首连的
  全量对账（full）+ 失败守卫保证失败窗口重拉；彻底解法待服务端×版本
  兼容实测（audit「应补测试」清单）。
- **L2 Fever 首同步深度**：Fever 协议/当前 Miniflux 服务端**支持** `items&max_id`
  向更旧条目翻页（历史回溯），但**本客户端未实现历史回溯**；首同步仅最近 50 条
  作种子，更早的历史条目只有在其进入 unread/saved 权威集合时经 `with_ids` 补齐。
  「历史是否仍被服务端保留另当别论」。能力记录点：`src-tauri/src/fever.rs`
  模块头「历史回溯能力」段。
- **L3 GR 单向的对称缺口**：远端「显式标未读」不经 GR 轻量对账回流；
  该方向变更只能等全量同步（同源合并路径的 `accept_unread`）或由本端
  push 覆盖。跨源副本的未读在任何路径都不落地（§3）。
- **L4 kept-unread 不识别**：GR `kept-unread` 标签常量已定义但无消费方，
  「保持未读」意图按普通未读处理。
- **L5 Fever 截断不可检测**：unread/saved 集合是单请求全集响应，无
  continuation 可校验；GR 侧的 P2-1 截断检测纪律在 Fever 协议上无对应
  机制（依赖服务端响应完整性）。

测试索引（对账方向的锁定分层）：

- 政策格锁定（单元，本卡新增）：`greader_pull.rs` / `fever_pull.rs` 的
  `#[cfg(test)]`——GR 单向不复活、GR/Fever 星标双向、Fever 双向复活、
  双侧 pending 保护；`conflict_policy.rs` 政策格取值锚。
- 端到端（既有）：`sync_phases_e2e.rs`（远端陈旧已读经轻量对账收敛；
  pending 守卫）、`dual_client_e2e.rs`（跨源副本未读不复活；副本广播；
  pending 防乒乓）、`star_reconcile_truncation_e2e.rs`（GR starred 截断
  → 对账中止，失败守卫）、`pull_cursor_e2e.rs`（分块/id 列举失败游标
  不推进）、`dedup_sync_e2e.rs`（同源判定矩阵/墓碑）。
- 端到端（live，`#[ignore]`，CI 外人工执行）：`fever_sync_live_e2e.rs`、
  `fever_live_e2e.rs`（Fever 真实后端的集合/对账往返）。
