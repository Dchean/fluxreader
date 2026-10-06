# 同步协议 × 服务端兼容矩阵（Google Reader / Fever）

TASK-112（二阶段④）。完成标准：「不同协议行为有明确说明」。本文面向维护者，
是同步状态语义的**用户可见对照表**；政策的代码单点在
`src-tauri/src/sync/conflict_policy.rs`（每格政策的常量定义、选择理由、历史依据
都在那里），本文与其同步维护——改政策 = 改政策点 + 改本文 + 改对应锁定测试。

**时点声明**：文中涉及外部服务端实现的事实（Miniflux / FreshRSS / 本仓
`tests/mock_greader.rs` 测试替身）来自 dev 分支源码查阅时点（2026-10），
**不代表部署服务的版本**；升级或更换服务端后须复核。

范围：状态与条目同步（push 队列 + pull 合并/对账）。订阅层（feeds 阶段的
订阅/分类增删改）不在本矩阵内。两协议共用同一条
`sync_queue → plan_push / exec_push` 推送管线与同一条 `merge_pulled_entry`
合并函数，协议差异集中在**对账方向**与**游标/条目获取能力**两处。

## 1. 协议 × 操作差异矩阵

| 维度 | Google Reader（GR） | Fever |
|---|---|---|
| 标读（推送） | `edit-tag` add read：绑定 entry + **全部同文副本**（read 广播政策，见 §3）；多个 entry 聚合单请求 | `mark read`：绑定 entry + 全部同文副本（同一 `plan_push`，协议无关） |
| 取消标读（推送） | `edit-tag` remove read：**仅绑定 entry**，不广播 | `mark unread`：仅绑定 entry，不广播 |
| 收藏 / 取消收藏（推送） | add / remove starred tag（显式 add-remove 语义，非 toggle）：仅绑定 entry | `mark saved / unsaved`：仅绑定 entry |
| 轻量对账 · 读状态 | **单向 read-wins**：read 集合命中 → 本地已读；未命中不回写（不复活未读） | **双向权威**（unread 集合）：命中 → 本地未读（可复活）；未命中 → 本地已读 |
| 轻量对账 · 星标 | **双向权威**（starred 集合）：命中 → 收藏；未命中 → 取消收藏 | **双向权威**（saved 集合）：同左 |
| 全量合并 · 读状态 | 与 Fever 共用 `merge_pulled_entry`：read-anywhere-wins（任何副本已读都接受）；unread 仅同源绑定 entry 接受（**跨源副本的未读不复活**）；跨源副本记账 + 远端已读即标读 | 同左 |
| 权威状态集合端点 | `stream/items/ids`（s=read / s=starred），continuation 分页；分页截断/中断按失败处理（P2-1） | `unread_item_ids` / `saved_item_ids`，单请求返回全集 |
| 条目获取 | `item_ids`(reading-list, ot) 分页列举 + `item_contents`（100 条/批） | 无全量历史端点：`items&since_id` 增量（50 条/页升序）或 `items_recent` 首种子（最近 50 条）+ `with_ids` 补齐权威集合中缺正文的条目（50 条/批） |
| 增量游标 | `last_sync_ts`（unix 秒）：**起点候选**（id 列举开始前取）+ 幂等合并 ⇒ 无漏无重；仅本轮「窗口拿全」（id 列举 + 分块零失败）才推进（TASK-068/069/097） | `last_sync_entry_id`（本轮已合并条目的 max，恒写）+ `last_sync_ts`（仅零失败时写，供切换回 GR 后的首拉，TASK-068/069） |
| 时间过滤列 | **随服务端实现而异**（见 §5）：mock 为 `changed_at >= ot`（边界含入），真实 Miniflux main 把 ot 映射为 `published_at > ot`（严格大于） | 无时间过滤（条目 id 单调递增游标） |
| 「失败 ≠ 空集合」守卫 | read/starred 集合任一拉取失败 → 本轮对账整体跳过（`greader_pull.rs` C-1 段） | unread/saved 集合任一拉取失败 → `reconcile_ok = false`，整段对账跳过（`fever_pull.rs`） |
| pending 保护 | `pending_ids`（sync_queue 未推送 read/unread/star/unstar）命中的条目整行跳过对账——两协议共享，见 §2 | 同左 |

## 2. 冲突政策表（对账方向）

政策格与代码常量一一对应（`conflict_policy.rs`；行级落地
`apply_read_by_policy` / `apply_star_by_policy`）：

| 政策格 | 常量 | 取值 | 理由（摘要） |
|---|---|---|---|
| GR × 读状态 | `GR_READ_DIRECTION` | 单向 read-wins | GR 只能拿到「已读 id 集合」，没有「明确保持未读」信号；把「不在已读集合」解释成「刚取消已读」会成批翻回历史条目。远端显式标未读由 push 段承担 |
| GR × 星标 | `GR_STAR_DIRECTION` | 双向权威 | 收藏低频强意图、取消是显式动作；截断集合的误判由失败守卫拦截（`star_reconcile_truncation_e2e`） |
| Fever × 读状态 | `FEVER_READ_DIRECTION` | 双向权威（unread 集合） | Fever 拿不到已读集合，「未命中 = 已读」是唯一信号，不双向则两端读状态都无法收敛。前提：Miniflux 按 URL 去重 entry，Fever 视角无跨源副本；误判代价由失败守卫 + pending 保护兜底 |
| Fever × 星标 | `FEVER_STAR_DIRECTION` | 双向权威 | 同 GR × 星标（saved 集合） |

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
  对账双向性的前提）。
- **FreshRSS（Fever 端点）**：`/api/fever.php`，新版布局移至
  `/p/api/fever.php`；`api_key` **只读 POST form body**（`p/api/fever.php:172`），
  放 query 里一律无效。应对：端点解析按候选 404 顺延
  （`tests/fever_freshrss_e2e.rs`）。
- **mock_greader（测试替身）**：ids 路由实现为 `changed_at >= ot`（边界
  含入）。与真实 Miniflux 的差异意味着**测试锁定的是客户端语义（起点候选
  + 幂等合并 + 失败守卫），不是任何特定服务端的过滤列**。

## 6. 已知限制与测试索引

已知限制：

- **L1 游标变更时间假设**：增量窗口不能单独保证「晚到的旧文章」被拉到
  ——真实 Miniflux 把时间过滤应用于发布时间，一篇发布时间早于游标、但
  晚进入订阅源的文章不会命中增量窗口（审计结论）。兜底：手动/首连的
  全量对账（full）+ 失败守卫保证失败窗口重拉；彻底解法待服务端×版本
  兼容实测（audit「应补测试」清单）。
- **L2 Fever 首同步深度**：Fever 无全量历史端点，首次仅最近 50 条作种子；
  更早的历史条目只有在其进入 unread/saved 权威集合时经 `with_ids` 补齐。
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
