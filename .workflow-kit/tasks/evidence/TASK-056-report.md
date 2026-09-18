# TASK-056 实施报告：修 pull 建订阅的外键违约 + 静默吞错

- 运行：RUN-7a6709f6a502438095cb1336d784e8b0
- 任务：TASK-056（批次 BATCH-68ca4c8d0dff468fb49faef5eea686fb）
- 授权：DEC-user-testing-bugs-20260918（owner 选择「两个都立，各一任务」+「一并修复静默吞错」）
- 用户报告：用 `https://demo.freshrss.org/api/greader.php` 登录成功，但**没有拉取到订阅**
- 日期：2026-09-18

---

## 1. 缺陷与根因

三处叠加，导致「登录成功、零订阅、零错误、前端报成功」：

### (1) 目录兜底值非法 → 外键违约

`src-tauri/src/sync/subscriptions.rs` pull 建本地订阅分支：

```rust
let folder_id: i64 = remote_folder_label
    .as_deref()
    .and_then(|label| db::find_folder_by_name(&conn, label).ok().flatten())
    .or_else(|| db::get_first_folder_id(&conn).ok().flatten())
    .unwrap_or(1);                       // ← folders 为空时给出不存在的目录 id
```

**新装应用 folders 表为空**（用户真实库复制件实测 `folders: 0`），
而 `feeds.folder_id INTEGER REFERENCES folders(id)` 且连接开启
`PRAGMA foreign_keys=ON`（`db/migrations.rs:235`）→ 插入必然
`FOREIGN KEY constraint failed`。

### (2) 错误被静默吞掉

```rust
if let Ok(fid) = inserted {   // ← 没有 else：错误在此消失
    ...
    report.pulled_feeds += 1;
}
```

失败既不记 `report.errors` 也不上抛，`feeds_phase` 照常返回 `Ok(report)`。

### (3) 前端因此报「成功」

`SyncTab.tsx` 在 `syncPhase('feeds')` 成功返回后 `showToast('已拉取订阅源…')`、
最终 `showToast('后端同步完成')`——用户观察到的「登录了但没拉到订阅」即此。

### 已有的正确兜底（此前未被 pull 使用）

`db::ensure_uncategorized_folder()`（`db/sync_map.rs:340`，注释即「确保『未分类』folder 存在」）
**早已存在**且被 `add_feed` 路径（`commands/folders.rs:152`）正确使用，pull 路径却没用它。

## 2. 改动内容

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/sync/subscriptions.rs` | 兜底改用 `db::ensure_uncategorized_folder()`（保证目录真实存在）；插入失败写入 `report.errors` |
| `src-tauri/src/sync/entries.rs` | **同类缺陷**：条目插入 `if let Ok((aid,_))` 同样无 else，改为 `match` 并在失败时写入 `report.errors` |
| `src-tauri/tests/sync_gap_repro_e2e.rs` | 新增 2 个测试（+119 行） |

## 3. 验证：fail-before / pass-after（附真实输出）

新增测试 **`fresh_empty_db_pull_creates_remote_subscriptions`**
（**零 folder 新库** + 远端一条无分类、一条有分类订阅）：

**修复前**（把兜底改回 `unwrap_or(1)`，强制重编译确认 `Compiling app`）：

```text
test result: FAILED. 0 passed; 1 failed
panicked at tests\sync_gap_repro_e2e.rs:416:5:
assertion `left == right` failed: 零 folder 的新库必须把远端订阅（含无分类的）真正建立到本地（TASK-056）；
report.errors=[]          ← 零订阅、零错误 = 用户看到的「静默失败」
  left: 0
 right: 2
```

**修复后**：同一测试通过。

新增测试 **`pull_feed_failure_is_reported_not_swallowed`**（可观测性契约）：
**把兜底与错误上报一并回退到修复前**后，该测试失败并打印：

```text
零订阅且零错误 = 静默失败，禁止（TASK-056）；local_feeds=0 report.errors=[]
```

两个测试互为补充：一个防「建不出」，一个防「静默」。

### 3.1 作者失误留痕（第二次自查纠正）

写测试时我第一次**只回退目录兜底、没有同时回退错误上报**，观察到该测试仍通过，
遂在注释里写下「它不能单独证明上报路径」的说法。**这个说法本身是错的**：
真正原因是我的测试**没有清空 mock 的 `folders`（tag/list）**，
于是 pull 先按远端分类建出了目录、走不到兜底分支——**测试当时是空转的**（vacuous）。
清空 mock folders 并给出**无分类**订阅后，它在回退时会真实失败（输出见上）。

**教训**：与 TASK-054 同源——**「测试通过」不等于「测试在测我以为是的那条路径」**；
构造缺陷场景时必须确认**前置条件真正成立**（此处是「本地 folders 必须为空且该订阅无分类」）。
本次靠「回退后仍通过」这一反常信号自查发现，未等审查指出。

## 4. 同类吞错排查（验收项要求）

全仓搜索 sync 路径的静默忽略点，逐条判定：

| 位置 | 形态 | 判定 |
| --- | --- | --- |
| `subscriptions.rs` pull 建订阅 | `if let Ok(fid)` 无 else | **已修**（本任务） |
| `entries.rs:250` pull 建条目 | `if let Ok((aid,_))` 无 else | **已修**（同类缺陷，一并处理） |
| `greader_pull.rs:166-174` / `fever_pull.rs:183-193` 状态标记 | `if let Ok(n) = sync_mark_*` | **保留**：这些是有条件更新（`n` 为受影响行数），0 行是**正常语义**（无需变更），非错误；且失败时另有对账路径收敛 |
| `greader_pull.rs:50,132` id 解析 | `if let Ok(id) = it.id.parse()` | **保留**：非数字 id 是协议的合法形态（如 Fever 的字符串 id），跳过属预期 |
| `backfill_entry_content` 内 `let _ =` | 忽略回填结果 | **保留**：回填是 best-effort 补内容，失败不影响条目已存在的事实；下一轮会重试 |

## 5. 门禁结果

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **139 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

**139 = 137（基线）+ 2（本任务新增）**；ignored 保持 9 未增。

## 6. 为何既有测试整体漏掉（结构性盲区，如实记录）

现有全部 Rust e2e 在插入 feed 前**一律先 `db::create_folder(...)`**，
且 mock 的 `folders`（tag/list）默认非空（`["Default", "Remote Cat"]`）——
**因此 `folders` 从不为空，`unwrap_or(1)` 这条兜底分支从未被执行**。
本任务补上了「零 folder 新库」这一此前完全无覆盖的场景。

## 7. 边界遵守

- 未改表结构或外键约束（约束本身正确，错的是兜底值非法 + 吞错）；
- 未改前端（后端 `report.errors` 通道已具备，前端既有 catch 会显示）；
- 未改同步协议语义、推送顺序、对账口径；
- 未引入新依赖；`Cargo.toml`/`Cargo.lock` 零改动；
- 未改工作流脚本；
- **未写入用户真实数据库**：诊断全程使用复制到临时目录的只读副本。

## 8. 遗留

- Bug 1（endpoint 文案/提示）为同批次 TASK-057，不在本任务范围；
- `db::get_first_folder_id` 在本任务后**仅剩其自身单测引用**（生产代码已无调用点）。
  因是 `pub` 且被 `db.rs` re-export，clippy 不报 dead_code。**未删除**——
  删除属独立清理决策，且它本身的单测仍有意义；此处如实记录供后续判断。
