# 基线 · TASK-056（2026-09-18，TASK-055 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **137 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

工作区：`git status --porcelain` 干净；`check` 全绿。

## 用户报告（原文）

> 我使用了 https://demo.freshrss.org/api/greader.php 登录，没有拉取到订阅

**注意「登录成功」**：`sync_save` 先跑 `test_connection`（会真实拉一次 `subscription/list` 并计数），
失败会直接报错、不保存凭据。用户能连上并保存，说明**认证与订阅列表拉取本身是通的**——
问题出在**把拉到的订阅写进本地**这一步。

## 实证定位

### 证据 1：用户真实库的 folders 表为空

对用户实际数据库（`%APPDATA%\com.fluxreader.app`，含 WAL 一起复制到临时目录后只读分析，
**未写入用户库**）：

```text
--- counts ---
  folders: 0
  feeds: 0
  articles: 0
  sync_queue: 0
```

`folders` 为空是**关键前提**——它决定了下列兜底值是否合法。

### 证据 2：外键违约可复现

表定义（复制件上读 `sqlite_master`）：

```sql
folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE
```

连接在 `db/migrations.rs:235` 执行 `pragma_update(None, "foreign_keys", "ON")`，
即**外键约束是开启的**。在 folders 为空、外键开启的库上执行 pull 等价的插入：

```text
--- REPRODUCE the app's pull insert with folder_id=1 ---
INSERT FAILED -> IntegrityError: FOREIGN KEY constraint failed
```

### 证据 3：缺陷代码

`src-tauri/src/sync/subscriptions.rs`（pull 建本地订阅分支）：

```rust
let folder_id: i64 = remote_folder_label
    .as_deref()
    .and_then(|label| db::find_folder_by_name(&conn, label).ok().flatten())
    .or_else(|| db::get_first_folder_id(&conn).ok().flatten())
    .unwrap_or(1);                                   // ← 硬编码 1：folders 为空时指向不存在的目录
let inserted = db::insert_feed_origin(..., folder_id, ...);
if let Ok(fid) = inserted {                          // ← 没有 else：错误在此消失
    if let Some(nid) = remote_feed_id {
        let _ = db::set_feed_remote_id(&conn, fid, nid);
    }
    report.pulled_feeds += 1;
}
```

三处叠加后果：

1. `remote_folder_label` 为空（用户远端无分类，或 label 在本地找不到）→ 落到 `get_first_folder_id`；
2. folders 为空 → `get_first_folder_id` 返回 `None` → **`unwrap_or(1)`** 给出非法目录 id；
3. 插入被外键拒绝 → `Err` 被 `if let Ok` **静默丢弃** → `report.errors` 保持为空，
   `feeds_phase` 返回 `Ok(report)`。

### 证据 4：用户看到的是「成功」

`src/components/settings/SyncTab.tsx:86-101` 在 `syncPhase('feeds')` 成功返回后：

- `showToast('已拉取订阅源，正在同步文章状态…')`
- 最终 `showToast('后端同步完成')`

由于后端返回的是 `Ok` 且 `report.errors` 为空，**前端没有任何机会知道订阅其实一条都没建**。
这与用户观察完全吻合：「登录了，但没拉到订阅」。

## 为什么既有测试整体漏掉

现有 Rust e2e（`sync_gap_repro_e2e.rs`、`sync_content_e2e.rs`、`sync_phases_e2e.rs`、
`dual_client_e2e.rs` 等）在插入 feed 前**一律先调用 `db::create_folder(...)`**
（如 `let folder_id = db::create_folder(&conn, "测试分类", "article").unwrap();`）。

因此 `folders` **从不被测试**为空，
`unwrap_or(1)` 这一兜底分支**从未被执行**，外键违约也从未发生。

**这是一个结构性盲区：没有任何测试覆盖「零 folder 的新库」。**
本任务必须补上该场景，否则同类缺陷仍会逃逸。

## 已有的正确兜底（未被 pull 使用）

`src-tauri/src/db/sync_map.rs:340`：

```rust
/// 确保「未分类」folder 存在：已有则返回其 id，无则创建后返回。
/// 用于 add_feed 未指定分类时的兜底逻辑。
pub fn ensure_uncategorized_folder(conn: &Connection) -> AppResult<i64> { ... }
```

它在 `commands/folders.rs:152`（`add_feed` 的 `None => db::ensure_uncategorized_folder(&conn)?`）
**已被正确使用**，但 pull 路径**没有用它**。

## 修复方向（实现者可用更好方案但须论证）

1. 兜底改为 `db::ensure_uncategorized_folder(&conn)?`（保证目录真实存在，满足外键）；
2. 插入失败**必须**写入 `report.errors`（含 URL 与原因），不得静默。

## 边界

- 不改表结构或外键约束（约束本身正确，错的是兜底值非法 + 吞错）；
- 不改前端（后端 `report.errors` 通道已存在，前端既有 catch 会显示）；
- 不改同步协议语义、推送顺序、对账口径；
- 不引入新依赖；
- **不写入用户真实数据库**（本次诊断全程用复制件只读分析）。
