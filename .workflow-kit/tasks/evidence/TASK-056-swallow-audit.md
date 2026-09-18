# TASK-056 同类吞错审计（修复轮 1 补全）

- 触发：第 1 轮独立审查 FINDING 3——§4 的审计表**不完整**，且对已列项的论证**不完整**。
- 范围：全仓 `src-tauri/src/sync/**` 中所有「写入/查询失败被静默忽略」的位置。
- 日期：2026-09-18

## 判定口径

| 类别 | 含义 | 处置 |
| --- | --- | --- |
| **上报** | 失败意味着用户可见的数据丢失或功能不可用 | 写入 `report.errors` |
| **保留（有据）** | 失败是**正常语义**（0 行 = 无需变更）或**可自愈**且上报会造成噪声 | 保留 `let _ =` / 条件忽略，并说明理由 |

## 一、本次修复（上报）

| 位置 | 原形态 | 为何必须上报 |
| --- | --- | --- |
| `subscriptions.rs` pull 建订阅 | `if let Ok(fid) = inserted` 无 else | 插入失败 = 订阅根本没进来，用户却看到「同步完成」（本缺陷本体） |
| `entries.rs` pull 建条目 | `if let Ok((aid,_))` 无 else | 同上，文章数静默不变 |
| `subscriptions.rs` 建远端分类目录 | `let _ = create_folder(...)` | **FINDING 3 指出**：静默失败 → 后续 `find_folder_by_name` 落空 → 该分类下订阅被改挂「未分类」，**用户目录结构无声丢失**，与本缺陷同族 |

## 二、保留项（逐条给出理由，含审查补充的 Err 情形）

| 位置 | 形态 | 保留理由（含被审查指出的缺口） |
| --- | --- | --- |
| `entries.rs:63,213` `let _ = set_article_remote_id` | 忽略 | 绑定 remote_id 是**优化**（下次可直接匹配）；失败只导致退化为 URL 兜底匹配，功能不缺。**失败无用户可见后果。** |
| `entries.rs:92,225,260` `let _ = sync_set_article_status` | 忽略 | 同条目状态写入：下一轮对账会重新收敛（幂等）。**自愈，上报只会每轮刷屏。** |
| `entries.rs:132` `let _ = add_article_dup_entry` | 忽略 | 智能去重的**辅助索引**写入；缺一条只会少挡一次重复，不致数据错。 |
| `entries.rs:137` `let _ = sync_mark_read_if_unread` | 忽略 | 幂等收敛操作（`WHERE is_read=0` 守卫），下一轮重试即可。 |
| `subscriptions.rs:119,228,282` `let _ = set_feed_remote_id` | 忽略 | 同 `set_article_remote_id`：绑定是优化，失败退化为 URL 匹配。 |
| `subscriptions.rs:182,206` `let _ = remove_*_tombstone` | 忽略 | 墓碑**多留一轮**无副作用（下一轮 pull 会再次尝试清除）；且它们发生在「远端已确认不含」之后，属清理动作。 |
| `greader_pull.rs:166-174` / `fever_pull.rs:183-193` `if let Ok(n) = sync_mark_*` | 条件忽略 | **审查补充的缺口已在报告与本节更正**：`n` 为受影响行数，`AND is_read=0` 类守卫使 `Ok(0)` 属**正常语义**（无需变更）。但 `if let Ok` **同时丢弃了真正的 `Err`**（SQLITE_BUSY / IO / 磁盘满）——**这一点此前未说明，现如实补上**。保留的理由：这些是逐条状态标记，单条失败会在下一轮对账自我修复；若为每条失败都推 errors，一次数据库繁忙会刷出成百条噪声，反而淹没真正的错误。**这是一个有意识的取舍，不是疏漏。** |
| `greader_pull.rs:50,132` `if let Ok(id) = it.id.parse::<i64>()` | 条件忽略 | `ItemRef.id` 是 `String`，需转数字才能送 `item_contents(&[i64])`；**非数字 id 无法被下游使用**，跳过是唯一选项。 |
| `backfill_entry_content` 内 `let _ = backfill_article_content` | 忽略 | 对**已存在**条目做 COALESCE 补内容/封面；失败不影响条目存在，下一轮重试。属 best-effort 补全。 |
| `entries.rs` `let _ = db::set_article_remote_id`（merge 分支内） | 忽略 | 同 `:63`。 |

## 三、覆盖范围说明（含第 2 轮审查的订正）

> **订正（第 2 轮审查指出，作者采纳）**：本节初版写「**已穷举** `src-tauri/src/sync/` 全部 `.rs` 中的
> `if let Ok(`、`let _ = db::`、`let _ = crate::` 形态」，**这个「穷举」是不成立的**——
> 机械统计实际有 **22 处 `let _ = db::`**，而本表只列了约 13 处。
> 这属于**又一次措辞上的过度声称**（与本会话反复出现的『结论先于验证』同源），故如实改正如下。

**实际覆盖情况**：

- `if let Ok(` 共 **9 处真实位点**（7 处 `sync_mark_*` + 2 处 id 解析）——**已全部列入本表** ✓；
- `let _ = crate::` 在 `sync/**` 中 **0 处**（该断言平凡成立）✓；
- `let _ = db::` 共 **22 处**，本表列入约 13 处；**未逐一列入**的有：
  `prune_sync` ×3（`subscriptions.rs:139`、`phases.rs:55,74`）、`purge_remove_feed_zombies`（`subscriptions.rs:143`）、
  `set_last_sync_ts` ×2（`greader_pull.rs:118`、`fever_pull.rs:155`）、`set_last_sync_entry_id`（`fever_pull.rs:154`）、
  `update_feed_title_if_empty`（`subscriptions.rs:238`）。
  **判定：可保留。** 它们均为**幂等/自愈**的清理或游标写入——`prune_sync` 失败只导致下轮重推（队列行仍在）、
  游标写失败只导致重拉同一窗口、`purge_remove_feed_zombies` 是遗留空操作。**要触发它们需要数据库级错误，而非本任务修复的逻辑缺陷。**
- **另有一类未纳入本表声明形态**：查询侧的 `.ok().flatten()` / `.unwrap_or_default()` 吞错
  （`entries.rs:81`、`push.rs:44,54`、`subscriptions.rs:116,176,189,207,222,257`）。
  其中最值得注意的是 `folder_tombstones()` / `feed_tombstones().unwrap_or_default()`——
  数据库错误会**静默得到空墓碑集合**，理论上可能让已删订阅复活。
  **判定：需数据库级错误才可达**（非本次的逻辑缺陷），**此处仅登记、记为后续候选**，不在本任务扩大改动。
- 命令层（`commands/`）与 `config_sync.rs` **不在本任务范围**（本任务只改 sync 拉取路径的失败可见性）；
  其中 `config_sync.rs:188,215` 的 `create_folder(...).unwrap_or(0)` 与 `subscriptions.rs:191` 形态相似
  （失败后回落 0 可能引发外键问题），**但属独立模块**，此处仅登记、不在本任务扩大改动。

## 四、结论

- **上报 3 处**（本任务修复，其中 1 处为审查 FINDING 3 新增）；
- **保留 10 类**，逐条给出理由，并补上了审查指出的「`if let Ok` 同时吞 Err」这一此前未说明的缺口；
- 修正了此前 §4 表格不完整的问题（原表漏列 `entries.rs` 的 `let _ =` 系列与 `create_folder`）。
