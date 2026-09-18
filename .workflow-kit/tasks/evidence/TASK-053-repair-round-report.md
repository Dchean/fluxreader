# TASK-053 修复轮（repair）实证报告

- 轮次类型：repair（处理独立审查 4 条 findings）
- 候选：`git diff HEAD -- src-tauri` = 3 文件 / +201 −17（与审查轮候选形状逐字一致）
- 审查报告：`.workflow-kit/tasks/evidence/TASK-053-review-report.json`（verdict=FAIL）
- 本报告只补实证与结论；**未 git commit**，**未改 `.workflow-kit/**`**，**未改前端**，
  **`Cargo.toml`/`Cargo.lock` 零改动**，**未启动 Tauri 应用**，**未触碰用户数据库**。

---

## 0. 复跑前置：先固定「哪一条 diff 行造成哪个行为」

审查者的 (a)(c) 两条都建立在同一个隐含假设上：
> `mark_all_read` 的 `schedule_state_push` 被改为**无条件**后，未配置时也会真的走到
> `feeds_phase`。

本报告用**逐行隔离**回答这个问题，而不是接受该假设。改动共两处语义：

| 行 | 位置 | 内容 |
|---|---|---|
| L1 | `articles.rs:183-186` | 入队循环去掉 `sync_configured` 前置分支（**本任务的目标变更**） |
| L2 | `articles.rs:149-156` | `schedule_state_push(&state)` 从 configured 分支内移到**无条件** |

隔离方法：只回退 **L1**（保留 L2），与候选版对照跑同一序列。

---

## 1. finding (a)：A-1 墓碑漂移 —— **反证成立（审查者命题为假），但过程中挖出一个真实的 P1 前存缺陷**

### 1.1 审查者的推理链与其断裂点

审查者链条：
> 离线删源 → 连接 → **投递**（`sync_local_feeds` 放行）→ `feeds_phase` → `pull_feeds`
> 拉取成功 → `subscriptions.rs:201-202` 清除墓碑 → 改前不会 → 削弱 TASK-035 防复活。

链条在**「投递」这一步**断裂。`mark_all_read` 的调度入口是
`schedule_state_push`（`commands/mod.rs:39-68`），它 spawn 出来的任务**只调用**
`crate::sync::push_states_now`（`mod.rs:61`）。而：

- `push_states_now`（`sync/push.rs:156-189`）**只** `plan_push`/`exec_push`，**不含** `pull_feeds`；
- 后台轻量同步 `sync_light`（`sync/phases.rs:96-101`）**只**调 `states_phase(false)`，**同样不含** `feeds_phase`；
- `feeds_phase` 的字符串字面量只出现在 `commands/sync.rs:130/192/254` 与 `sync/phases.rs:86`，
  **没有一处位于 `mark_all_read` 可达的路径上**。

即：**L1 与 L2 都无法让 `feeds_phase` 在「连接后」被 `mark_all_read` 触发。**
「离线时入队、连接后补推」这条投递路径消费的是**状态队列**（`states_phase` 推送段），
不是 feeds 阶段；而墓碑清除副作用只存在于 feeds 阶段。

### 1.2 实证：三组探针，两版对照

探针产物：`.probe053/probe_tombstone_drift.rs`、`.probe053/probe_mark_all_read_path.rs`
（含一个可注入「远端订阅列表滞后」的 mock 变体 `.probe053/mock_greader.rs`）。
**注意：这些探针文件为临时产物，按硬约束 7 已在门禁前全部删除，未进入候选。**

#### 探针组 A（`probe_mark_all_read_path`）：L1 的双世界对照

同一序列（配置态 → `mark_all_read` → `states_phase(false)` → 删源 → 退订 → `feeds_phase`）：

| 观测点 | 世界 A（L1 回退＝改前语义） | 世界 B（L1 候选语义） |
|---|---|---|
| `queued_after_mark_all_read`（配置态） | 2 | 2 |
| `configured` | true | true |
| `state.http` 投递后可删源？ | 是 | 是 |
| `unsubscribe_target` | **true** | **true** |
| `unsubscribe_ok` | true | true |
| `tombstones` | `[]` | `[]` |
| `feed_rows_before_pull` | 0 | 0 |

**两版逐项完全一致。** 唯一差异出现在**离线**场景（回退版 `queued=0`，
候选版 `queued=2`），而该差异**只体现在状态队列**，与墓碑路径不相交。

#### 探针组 B（`probe_tombstone_drift`）：墓碑清除**只由 `pull_feeds` 触发**

`path_tombstone_clear_is_pull_only` 探针**完全不调用 `mark_all_read`**，只做
「删源 → 退订 → `feeds_phase`」：

```
PROBE053-path C tombstones_after_unsubscribe=[]
PROBE053-path C tombstones_after_pull=[] feed_rows=1
```

与调用 `mark_all_read` 的版本结果**完全相同** ⇒ 墓碑清除与 `mark_all_read` **无关**。

#### 探针组 B 对照：清除规则的唯一决定变量是「远端列表是否已不含」

| 远端列表 | `feeds_phase` 后墓碑 | 是否复活 |
|---|---|---|
| 已不含（紧耦合，退订立即生效） | 清除 | 否 |
| **仍含（列表滞后）** | 清除 | **是（复活）** |

#### 结论

- **审查者命题为假**：在「离线删源 → 连接 → 投递」序列中，改动前后墓碑状态**相同**；
  本改动（L1/L2）**不改变**该序列的行为。
  断裂点：**「投递」不等于「feeds 阶段」**——`mark_all_read` 的投递只到 `push_states_now`/`states_phase`，
  而墓碑清除只在 `feeds_phase → pull_feeds` 里。
- 审查者给出的第三组观测（`unsubscribe_target=true`）**在改动前后都为 true**，
  它取决于删除发生时是否已配置（本例两版都已配置），**与本次改动无关**。

### 1.3 但探针挖出一个**真实且前存**的 P1 缺陷（不在审查者命题内）

`grep` 全文确认：`sync/subscriptions.rs:48` 的 `remove_feed_tombstone`（位于
`unsubscribe_remote` 内）在**本改动前后逐字相同，未被本任务触碰**。但它是**语义上过强**的：

```rust
let ok = match client {
    Backend::GReader(c) => c.unsubscribe(remote_id).await.is_ok(),   // 仅判 HTTP 2xx
    Backend::Fever(_) => false,
};
if ok { db::remove_feed_tombstone(&conn, feed_url); }                 // ← 未核实服务端列表
```

`unsubscribe_remote` 的注释与 A-1 测试（`:219`「退订成功（远端确认）后墓碑应清除」）
都把 `ok` 当作**「远端已确认」**，但 `ok` 实际只是**「HTTP 请求返回成功」**。
当远端列表滞后（GReader→Miniflux 等代理/最终一致场景），`ok=true` 而远端列表**仍含**该订阅，
此时墓碑被清除，随后任意一次 `feeds_phase` 的 `pull_feeds` 就会把已删订阅**复活**：

```
PROBE053 probe3 unsubscribe_ok(list_stale)=true
PROBE053 probe3 remote_still_lists_feed10=true
PROBE053 probe3 tombstones_after_unsubscribe_fact=[]     ← 墓碑已被清除
PROBE053 probe3 tombstones_after_pull=[] feed_rows_after_pull=1   ← 复活
```

**性质判定**：这是 TASK-035/A-1 的**前存缺陷**，**既非本改动引入、也非本改动可触发**
（探针组 A 已证 L1 两版行为一致；探针组 B/C 已证与 `mark_all_read` 无关）。
它与本任务的「是否入队」边界不同源 —— 修它要动 `unsubscribe_remote` 的**成功判据**，
而任务卡 non_goals 明列「改变除入队条件以外的同步协议语义」，且审查者亦确认本任务
「只被授权改『是否入队』」。

**处置：记录为独立发现，不在本任务内修改**（见 §5 未完成/越界项）。
存在一个**更干净**的修法可选（供后续任务采用）：把 `unsubscribe_remote` 内的墓碑清除
**删除**，只保留 `pull_feeds` 的清除路径（`subscriptions.rs:201-202`，判据是**权威列表**
`!remote_norm.contains(t)`）。该修法**只减少墓碑清除点、只增强防复活保护**，
且既有测试 `deleted_feed_stays_deleted_and_unsubscribes` 的 ③ 段（`:224-237`）
会在 pull 之后重新验证墓碑已清且不复活，因此**不会**破坏该测试。但它确实改变了
「是否清除墓碑」这一协议语义，**超出本任务授权边界**，故本轮不改。

---

## 2. finding (b)：既有回归网空白 —— **基本属实，已独立复现并更正**

### 2.1 独立复现（b-1）

做法：**只回退 L1**（保留 L2），再用 `git show HEAD:src-tauri/tests/sync_gap_repro_e2e.rs`
取出 HEAD 版 `offline_read_change_pushed_after_connect`，逐字复制进当前测试文件（重命名以免冲突，
并加 `#[allow(dead_code)]`），**不做任何逻辑改动**运行：

```
=== (b) GATE PROBE on REVERTED code ===
test result: ok. 1 passed; 0 failed; 0 ignored; 0 filtered out; finished in 0.03s
```

**缺陷存在（`mark_all_read` 仍然 configured-gated）时，该既有 A-5 保护测试照样通过。**
门控有效性控制实验：对该复刻测试加 `if true { return; }` 后同样 `ok. 1 passed`，
证明门控确实生效、上述结果不是「测试根本没跑」。

**结论：该测试只覆盖 `record_read_state`（`set_read` 路径），完全不覆盖 `mark_all_read`。**

### 2.2 系统性覆盖清单（b-2）

不止复核审查者那一条，而是**把 `commands/articles.rs` 里三条状态变更路径逐一回退**，
跑**完整** `cargo test`，看是否有任何既有测试失败。

| 路径 | 缺陷版入队条件 | 回退后失败的测试 | 既有回归网是否保护 |
|---|---|---|---|
| `record_read_state`（`set_read`） | `if sync_configured` | **`offline_read_change_pushed_after_connect`** | ✅ **有保护** |
| `record_star_state`（`set_starred`） | `if sync_configured` | **（无任何测试失败）** | ❌ **空白** |
| `apply_mark_all_read`（`mark_all_read`） | `if sync_configured` | **`offline_mark_all_read_queued_and_pushed_after_connect`**（本任务**新增**） | ⚠️ **原本空白，本任务新建** |

原始输出见 `.probe053/coverage_matrix.out`（临时产物，已删除；关键行摘录）：

```
################ PATH=read ################
test offline_read_change_pushed_after_connect ... FAILED
test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured

################ PATH=star ################
（全部 test result: ok.，无任何 FAILED）

################ PATH=markall ################
test offline_mark_all_read_queued_and_pushed_after_connect ... FAILED
test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured
```

**更正任务卡的不实陈述**：任务卡 `test_review` 中
> 「既有 Rust 同步测试是直接回归网」

**不实**。本缺陷（`mark_all_read` 离线不入队）在既有回归网中**原本未受保护**；
保护是本任务**新增**的 `offline_mark_all_read_queued_and_pushed_after_connect`
才建立的。既有 `offline_read_change_pushed_after_connect` 对 `mark_all_read` **零覆盖**。

**顺带发现的第二处空白（新信息，审查者未报）**：`record_star_state`（`set_starred`）
的「离线入队」**同样未被任何测试保护** —— 把它的入队改回 configured-gated，
**121 个测试无一失败**。即 `set_starred` 的 A-5 语义目前**裸奔**。
仓内另外两处 `record_star_state` 调用（`sync_gap_repro_e2e.rs:561` 的 C-1 测试、
`:976` 的 A-8 对照）**都在已配置态**下调用，故不覆盖离线分支。

### 2.3 前端路径清单（b-2 续）：`toggleEntryFlag` / `markEntriesReadBulk` 同源

前端不存在独立的 bulk 后端命令，两者都收敛到已有的 per-item 命令：

| 前端入口 | 后端命令 | 后端真实逻辑 | 离线入队是否被测试覆盖 |
|---|---|---|---|
| `toggleEntryFlag(id,'isRead')` | `set_read` | `record_read_state` | ✅ 有（`offline_read_change_...`） |
| `toggleEntryFlag(id,'isStarred')` | `set_starred` | `record_star_state` | ❌ **无（同 2.2 空白）** |
| `markEntriesReadBulk(ids)` | 循环 `set_read` | `record_read_state` | ✅ 有（同上，同一后端路径） |
| 滚动标读 `markEntriesReadBulk` | 循环 `set_read` | `record_read_state` | ✅ 有（同上） |
| 「全部已读」菜单 `api.markAllRead` | `mark_all_read` | `apply_mark_all_read` | ⚠️ 本任务新建后才覆盖 |

### 2.4 处置

- **未覆盖项一律只记录不改**（硬约束：本任务只授权改「是否入队」，不授权扩充无关测试网）。
  唯一例外是本任务**同源**路径 `mark_all_read`，其保护已由新增测试建立。
- `set_starred` 的离线入队空白**上报**，建议开独立任务补一条与
  `offline_read_change_pushed_after_connect` 同形的 star 版测试（含显式
  `read_credentials(&conn).is_none()` 前置断言）。

---

## 3. finding (c)：`feeds_phase` 未配置时返回 Err 且被吞 —— **核对结论：该 Err 在本改动后不可能被触达，描述不成立**

### 3.1 逐项核对（探针 `.probe053/probe_feeds_phase_err.rs`，临时产物已删除）

```
PROBE053-c F1  feeds_phase_err_code=notConnected msg=未配置同步后端（Google Reader / Fever 凭据）
PROBE053-c F1b push_states_now_completed_no_error_channel=true
PROBE053-c F1b queue_after_unconfigured_push=1
PROBE053-c F1c states_phase_err_code=notConnected
```

| 审查者陈述 | 核对结果 |
|---|---|
| `feeds_phase` 未配置时 `build_client` 为 None → `Err(notConnected)`（`phases.rs:18-23`） | ✅ **属实**（F1 实测 code=`notConnected`；且**不发任何 HTTP 请求**） |
| `mark_all_read` 改为无条件调度「连带使 `feeds_phase` 未配置时也被调」 | ❌ **不成立**。`schedule_state_push` 的唯一被调函数是 `push_states_now`（`mod.rs:61`），后者**不含** `feeds_phase`；后台同步 `sync_light` 也只用 `states_phase`。`feeds_phase` 不在 `mark_all_read` 可达路径上（同 §1.1） |
| 该 Err 当前被 `let _ =` 吞掉 | ❌ **不成立**。全仓 `feeds_phase` 调用点：`sync.rs:130` 经 `sync_phase` 的 `match` 以 `?` **上抛**给前端；`sync.rs:192`/`254` 均 `?` 上抛。**不存在 `let _ = feeds_phase(...)`** |
| 与 A-5「静默跳过」语义相抵 | ❌ **不成立**。A-5 的「静默跳过」发生在**推送段**：`push_states_now` 在 `build_client` 为 None 时 `return`（`push.rs:156-159`，签名返回 `()`，**结构上无错误面**）。F1b 实测：未配置时推送段静默跳过、**队列保留 1 条**待连接后补推 |

### 3.2 处置与理由

**不改。** 理由：

1. **无可修之处**：审查者所指的耦合（`mark_all_read` → `feeds_phase`）**不存在**。
   本改动（L1/L2）都没有把 `feeds_phase` 引入任何新路径，故不存在新增的 Err 面。
2. **`feeds_phase` 未配置返回 Err 是既有的、正确的显式 API 语义**：它是
   `sync_phase(which="feeds")` 这个**用户显式动作**的实现，未连接时理应报错让前端提示，
   这正是 A-5 注释里区分开的「显式操作须提示未连接」的守卫语义
   （与 `sync.rs:147` `sync_local_feeds` 的 `Err("未连接后端")` 同族）。
3. **改它会越界**：若真去给 `feeds_phase` 加「未配置提前短路返回 `Ok(默认report)`」，
   那是修改 `phases.rs` 的**阶段语义**（把「显式同步失败」变成「静默成功」），
   直接违反 task card non_goals「改变除入队条件以外的同步协议语义」。
   **按任务卡的硬要求，此处明确停下来报告：本项不改，也不建议在本任务内改。**
4. **不需要「明确记录该 Err」**：既然该 Err 在 `mark_all_read` 路径上不可达，
   `let _ =` 吞掉的问题也就不存在；无从记录。

结论：**(c) 是一条基于错误路径假设的 finding；核对该路径的实际行为后判定为不成立。**

---

## 4. 门禁（自跑，退出码重定向到文件后读取）

| 门禁 | 命令 | 退出码 | 结果 |
|---|---|---|---|
| 1 | `cargo test`（`src-tauri`） | **0** | **121 passed / 0 failed / 23 ignored** |
| 2 | `npm run lint` | **0** | **0 warnings / 0 errors**（56 files / 116 rules） |
| 3 | `npm run build` | **0** | `✓ built in 1.02s` |
| 4 | `npm run test:frontend` | **0** | **241/241**（既有回归 26/26 + 新增 store 断言 215/215） |

基线对照：`cargo test` 121/0/23 ✅ 通过数未减少、**`#[ignore]` 未增加（23 → 23）**。
`npm run lint` 通过 `npm.cmd` 执行（本机 PowerShell 执行策略禁止 `npm.ps1`）。

> 复跑提示：跑门禁 1 时曾出现一次 `offline_mark_all_read_queued_and_pushed_after_connect`
> FAILED —— 原因是覆盖矩阵探针脚本在**同一 mtime 秒内**反复覆写 `articles.rs`，
> cargo 的 freshness 指纹未察觉内容变化而**跳过了重建**（`Finished in 0.36s`，无 `Compiling`）。
> 对源文件 touch 后重建即恢复 `121/0/23`。**这不是候选缺陷**，但值得记录为
> 「探针脚本快速换文件会让 cargo 误判 fresh」的陷阱。

---

## 5. 未完成 / 不确定项（不掩饰）

1. **§1.3 的前存 P1 缺陷未修**（`unsubscribe_remote` 以 HTTP 2xx 当作「远端确认」，
   列表滞后时清墓碑 → 复活）。**理由**：非本改动引入、非本改动可触发、修它超出
   「是否入队」授权边界。已给出更干净的修法建议（删除 `subscriptions.rs:48` 的清除点，
   只留 `:201-202` 的权威列表判据），供后续独立任务采用。
2. **`record_star_state` 离线入队无测试保护**（§2.2 第二处空白），**只记录不改**。
3. `probe3` 的「远端列表滞后」是靠**注入**（`stale_unsubscribe`）构造的；真实 GReader
   服务端是否会出现「`ac=unsubscribe` 返回 2xx 但 `subscription/list` 仍含该订阅」，
   本环境**无法用真实服务端验证**（无凭据、且未启动应用）。该场景在代理型后端
   （如 GReader→Miniflux）中是可发生的，但**属推断而非实测**。
4. 审查者报告中的一处**细节订正**：其 `review_checks.regression` 称漂移会「放行
   `sync_local_feeds`」——`sync_local_feeds` 是**用户显式命令**（首连弹窗/手动按钮），
   不是 `mark_all_read` 投递路径的一部分；该措辞会让人误以为它被自动触发。
5. 探针文件（`.probe053/**`、`src-tauri/tests/probe_*.rs`、mock 变体）按硬约束 7
   **已全部删除**；`.git/info/exclude` 中我加的一行本地忽略项也已移除。

---

## 6. 最终改动

**本轮未产生任何候选改动。** `git status -- src-tauri` 仍只有预期 3 文件，
`git diff HEAD --numstat -- src-tauri` 仍为 `36/16`、`1/1`、`164/0`（= +201 −17），
与审查轮候选**逐字一致**；`articles.rs` 与探针前备份 SHA 级内容相等。
**本轮的产出是实证与结论，不是代码。**

### 6.1 逐 finding 的「修前失败 → 修后通过」证据

| finding | 是否修了 | 证据 |
|---|---|---|
| (a) 墓碑漂移 | **否（反证）** | 双世界探针逐项一致；C 探针证明墓碑清除只由 `pull_feeds` 触发、与 `mark_all_read` 无关。**无「修前失败」可言，因为命题不成立** |
| (a) 附带发现的前存 P1 | **否（越界，仅记录）** | `probe3` 输出 `tombstones_after_unsubscribe=[]` + `feed_rows_after_pull=1`；**该行为在改动前后相同**（`git` 未触碰该行） |
| (b) 回归网空白 | **是（补测已在候选内）** | 修前：回退 L1 后 `offline_mark_all_read_queued_and_pushed_after_connect` **exit 101**，`left: 0 / right: 2`；修后：**ok，12 passed / 0 failed**。既有测试在回退版上 **exit 0 / 1 passed**（= 空白证据） |
| (c) `feeds_phase` Err | **否（不成立）** | F1/F1b/F1c 实测；`feeds_phase` 不在 `mark_all_read` 可达路径上；无 `let _ =` 吞掉点 |
| (d) 披露 | **是（本报告）** | 全篇 |

---

## 7. 实际执行的命令

```
# 现场核对
git status --short
git log --oneline -5
git diff HEAD --stat -- src-tauri
git diff HEAD -- src-tauri/src/commands/articles.rs
git diff HEAD -- src-tauri/src/commands/mod.rs
git show HEAD:src-tauri/src/commands/articles.rs | Select-String -Pattern "enqueue_sync"
git show HEAD:src-tauri/tests/sync_gap_repro_e2e.rs > .probe053/sync_gap_repro_e2e.HEAD.rs
grep enqueue_sync / sync_configured / feeds_phase / remove_feed_tombstone （全仓）

# 探针（临时，跑完删除）
cargo test --test probe_tombstone_drift -- --nocapture --test-threads=1
cargo test --test probe_mark_all_read_path -- --nocapture --test-threads=1
cargo test --test probe_feeds_phase_err -- --nocapture --test-threads=1
cargo test --test sync_gap_repro_e2e gate_probe_head_offline_read_change        # (b) 复现
cargo test --test sync_gap_repro_e2e offline_mark_all_read_queued_and_pushed_after_connect
cargo test --test sync_gap_repro_e2e                                            # 12 passed
pwsh -NoProfile -File .probe053/coverage_matrix.ps1                            # (b) 覆盖矩阵

# 门禁（退出码重定向到文件后读取）
cd src-tauri; cargo test > ..\.probe053\gate_cargo_test.out 2> ..\.probe053\gate_cargo_test.err
npm.cmd run lint          > .probe053\gate_lint.out     2> .probe053\gate_lint.err
npm.cmd run build         > .probe053\gate_build.out    2> .probe053\gate_build.err
npm.cmd run test:frontend > .probe053\gate_frontend.out 2> .probe053\gate_frontend.err

# 还原核验
git status --short -- src-tauri
git diff HEAD --numstat -- src-tauri
```

---

## 8. 提交给总控的判定建议

- (a) **反证成立**：审查者命题为假，改动前后的墓碑行为**逐项相同**；断裂点在
  「投递 ≠ feeds 阶段」。反证同时**发现并实证**了一个独立的前存 P1 缺陷（已记录、未修，越界）。
- (b) **属实**：任务卡「既有同步测试是直接回归网」应更正为「原本未受保护，保护由本任务新建」；
  另**新增发现** `set_starred` 离线入队同样无保护。
- (c) **不成立**：基于错误的路径假设；`feeds_phase` 未配置返回 Err 是既有的正确显式动作语义，
  且不在本改动可达路径上，无 `let _ =` 吞掉点。按任务卡要求，此处已停止并报告，未越界改动。
- 四门禁全绿，候选与审查轮**逐字一致**（本轮零代码改动）。
