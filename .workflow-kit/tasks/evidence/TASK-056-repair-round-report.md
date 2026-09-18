# TASK-056 修复轮 1 报告（回应第 1 轮审查的 3 条 findings）

- 任务：TASK-056（批次 BATCH-68ca4c8d0dff468fb49faef5eea686fb）
- 前序：第 1 轮独立审查 **FAIL**（候选 `b52bfacb…`），见 `TASK-056-review-r1.json`
- 日期：2026-09-18

---

## 对应处置总览

| Finding | 性质 | 处置 | 证据 |
| --- | --- | --- | --- |
| **1（主因）** 『失败必须可见』契约未被任何测试锁定 | 真问题（我先前把析取式断言当成了守卫） | **新增 2 个强断言测试**（注入确定性失败），并**更正原弱测试的定位说明** | 回退对应 Err 分支后两测试各自 exit 101（见 §1.3） |
| **2（主因）** 报告 §7『前端既有 catch 会显示』不实，且跳过 non_goal 闸门 | 真问题（**我未核对就写下的断言**） | **撤回该断言**；按 non_goal『停下报告』登记前端缺口与修复方向 | `TASK-056-frontend-errors-gap.md` |
| **3（次要）** §4 审计不完整 + 一处『保留』论证不完整 | 真问题 | **补全审计表**；并**修复**审查点出的 `create_folder` 静默失败（同族缺陷）；补上『`if let Ok` 同时吞 Err』的说明 | `TASK-056-swallow-audit.md` |

---

## 1. FINDING 1：让『失败必须可见』真正被锁定

### 1.1 我先前错在哪

我提交的 `pull_feed_failure_is_reported_not_swallowed` 断言是**析取式**：

```rust
assert!(local_feeds > 0 || !report.errors.is_empty(), ...);
```

目录兜底修好后**左式恒真**，于是这条断言对「静默吞错」毫无约束力。
审查者的 Probe C 实证：保留正确兜底、只把 `Err => report.errors.push(...)` 改回
`if let Ok(fid) = inserted`，**全量 cargo test 仍 139 passed / 0 failed / 9 ignored**。

**我自行复现确认**（本轮）：

```text
[PROBE C] compiled=True exit=0
[PROBE C] 139 passed / 0 failed / 9 ignored
[PROBE C] FAILED occurrences: 0
==> FINDING 1 CONFIRMED: reverting the error-reporting arm is invisible
```

我在报告 §3 写的「一个防『建不出』，一个防『静默』」**对已提交的测试集不成立**——
这是一条我未经验证就写下的结论。

### 1.2 修法：注入确定性失败 + 强断言

新增 **`pull_feed_insert_failure_must_be_reported`**：
在测试库上给 `feeds` 加 `BEFORE INSERT` 触发器，遇目标 URL 时 `RAISE(ABORT)`。
这是在测试库注入失败的标准做法——**不改生产代码、不依赖巧合**，
且与真实缺陷走**同一条代码路径**（无分类订阅 → 目录兜底 → 插入 → `Err` 分支）。

断言改为**强断言**（非析取式）：

```rust
assert!(!report.errors.is_empty(), "...report.errors={:?}", report.errors);
assert!(report.errors.iter().any(|e| e.contains(failing_url)), "...");
assert_eq!(n, 0, "触发器应阻止该订阅插入（前置条件校验）");
```

新增 **`pull_entry_insert_failure_must_be_reported`**：同样手法针对 `entries.rs`
（审查的 Probe D 证实该处 Err 上报回退后同样无人发现），对 `articles` 加触发器。

### 1.3 证明两个测试**确实**是承重的

回退对应 Err 分支（各含强制重编译、确认出现 `Compiling app`）：

```text
[A] subscriptions arm reverted -> pull_feed_insert_failure_must_be_reported:
    exit=101 compiled=True FAILS(good)
    panicked at tests\sync_gap_repro_e2e.rs:480:5:
    注入的插入失败必须写入 report.errors（不得静默吞掉，TASK-056）；report.errors=[]

[B] entries arm reverted -> pull_entry_insert_failure_must_be_reported:
    exit=101 compiled=True FAILS(good)
    panicked at tests\sync_gap_repro_e2e.rs:560:5:
    注入的条目插入失败必须写入 report.errors（不得静默吞掉，TASK-056）；report.errors=[]

restored subscriptions: True
restored entries      : True
==> both contracts locked: True
```

### 1.4 原弱测试的处置：**保留但更正定位**

未删除（它仍有价值：回退目录兜底后必失败，是**缺陷复现锁**），
但把文档注释中「与强测试互为补充、防静默」的说法**更正为如实描述**：
它**不能**单独守住可观测性契约，该契约已由新增的强测试承担。

---

## 2. FINDING 2：撤回不实断言，按 non_goal 停下报告

### 2.1 我错在哪

报告 §7 写：

> 未改前端（后端 `report.errors` 通道已具备，前端既有 catch 会显示）

**这是未经核对的断言。** 穷举 `src/`、`tools/`、`src-tauri/src/` 全部 `.errors` 引用后确认：
**前端没有任何代码读取 `SyncReport.errors`**。

- `src/lib/api.ts:96-104` **只声明类型**；
- `SyncTab.tsx:87-91` 与 `store/slices/sync.ts:75-80` 两处 `syncPhase` 调用**都丢弃了返回的 report**；
- `phases.rs` 在 `errors` 非空时**仍返回 `Ok(report)`**，
  故 `SyncTab.tsx:102` 的 `.catch()` 对该路径**永不触发**（我原先假设它会触发，这是错的）；
- 只有 `scheduler.rs:238` 记了 `errors.len()`（仅日志，不对用户可见）。

后果：对**非外键**的插入失败（磁盘满、SQLITE_BUSY 等），
UI **仍会弹「已拉取订阅源」「后端同步完成」**——spec 声称的可观测契约变化**只交付了后端一半**。

### 2.2 处置

TASK-056 `non_goals` 明确：**「若发现前端仍需改，停下报告」**。
该条件成立，故**不在本任务内扩大改动**，改为：

- 新增 `TASK-056-frontend-errors-gap.md`：登记问题、穷举证据、并给出两个候选修复方向；
- 明确指出该修复属**前端行为变更**，需 owner 授权（并可能需 UI 证据），
  TASK-057 已拥有 `SyncTab.tsx` 范围，是自然承载位置；
- **在报告中撤回原断言并留痕**（本节即为撤回记录）。

---

## 3. FINDING 3：补全审计 + 修复同族缺陷

### 3.1 审查点出的同族缺陷：**已修**

`subscriptions.rs` 建远端分类目录原为 `let _ = db::create_folder(&conn, label, "article")`。
静默失败会让后续 `find_folder_by_name` 落空 → **该分类下的订阅被改挂「未分类」**，
即**用户的目录结构无声丢失**——与本次修复的外键缺陷同族。已改为：

```rust
if let Err(e) = db::create_folder(&conn, label, "article") {
    report.errors.push(format!("建远端分类「{label}」失败: {e}"));
}
```

### 3.2 审计表补全与论证修正

新增 `TASK-056-swallow-audit.md`，穷举 `src-tauri/src/sync/**` 全部静默忽略点：

- **上报 3 处**（订阅插入、条目插入、建分类目录）；
- **保留 10 类**，逐条给出理由；
- **补上审查指出的缺口**：`if let Ok(n) = sync_mark_*(...)` 这类**同时丢弃了真正的 `Err`**
  （SQLITE_BUSY/IO/磁盘满），此前报告只论证了「0 行是正常语义」。现如实说明：
  保留是**有意识的取舍**（逐条状态标记可下一轮自愈；若每条失败都推 errors，
  一次数据库繁忙会刷出成百条噪声、淹没真正的错误），而非疏漏；
- 原 §4 漏列的 `entries.rs` 的 `let _ =` 系列与 `create_folder` **已全部补入**；
- 另登记范围外但形态相似的 `config_sync.rs:188,215` `create_folder(...).unwrap_or(0)`
  （仅登记，不在本任务扩大改动）。

---

## 4. 门禁结果（修复后）

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **141 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

**141 = 139（修复前）+ 2（新增的两个承重测试）**；ignored 保持 9 未增。

## 5. 改动文件

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/sync/subscriptions.rs` | 建分类目录失败改为上报（FINDING 3） |
| `src-tauri/tests/sync_gap_repro_e2e.rs` | 新增 2 个承重测试；更正原弱测试的定位说明 |
| `.workflow-kit/tasks/evidence/TASK-056-frontend-errors-gap.md` | 新增：前端缺口登记（FINDING 2） |
| `.workflow-kit/tasks/evidence/TASK-056-swallow-audit.md` | 新增：完整吞错审计（FINDING 3） |

## 6. 作者失误留痕

本轮三条 findings 中，**两条（1、2）都源于同一个毛病：把「我以为的」当成「已核对的」写进结论**——
以为析取式断言能守契约（未做 Probe C 那样的回退实验）、
以为 `.catch()` 会触发（未读 `phases.rs` 是否 `Ok(report)`、未 grep 前端有无消费）。

这与本会话此前的失误同源（`changed_files` 声明、`npm build` 笔误、worker-result 漏字段、
TASK-054 探针缩进致假阴性）。**共性：结论先于验证。**
本轮起，凡断言「某通道会被消费 / 某测试能守住某契约」，一律先用回退实验或穷举 grep 证实。
