# 基线 · TASK-055（2026-09-18，TASK-054 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **136 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

工作区：`git status --porcelain` 干净（除本任务的台账改动）；`check` 全绿。

> `cargo test` 由 TASK-054 的 122 passed / 23 ignored 变为 136 passed / 9 ignored
> （136 = 122 + 14；9 = 23 − 14）。TASK-055 的基线取 TASK-054 之后的值。

## 缺陷描述（P1）：墓碑被误清除 → 已删订阅复活

### 代码位置与链路

1. `src-tauri/src/commands/folders.rs:192-205` `record_feed_deletion`：
   删除订阅时**先写墓碑**（`:198 db::add_feed_tombstone`）、再删本地行、返回 `Some((remote_id, feed_url))`。

2. `src-tauri/src/commands/folders.rs:216-218` 锁外 best-effort 调用
   `crate::sync::unsubscribe_remote(&state.db, &state.http, remote_id, &feed_url)`。

3. `src-tauri/src/sync/subscriptions.rs:42-45`：
   ```rust
   let ok = match client {
       Backend::GReader(c) => c.unsubscribe(remote_id).await.is_ok(),
       Backend::Fever(_) => false,
   };
   ```

4. `src-tauri/src/greader.rs:457-464` `unsubscribe` → `post_form_text(path, form)`；
   `greader.rs:323-334 post_form_text` 的判据是：
   ```rust
   if !resp.status().is_success() { return Err(...); }
   Ok(())
   ```
   **只要 HTTP 2xx 就返回 `Ok(())`。**

5. 回到 `subscriptions.rs:46-49`：
   ```rust
   if ok {
       let conn = db.lock().await;
       let _ = db::remove_feed_tombstone(&conn, feed_url);   // ← 误清除点
   }
   ```

6. 下一次 `feeds_phase` 的 pull 分支（`subscriptions.rs:196-207`）**只用墓碑挡复活**：
   ```rust
   for rf in &remote_subs {
       if tombstones.contains(&db::normalize_url(&rf.url)) {
           continue; // 本地已删除且远端仍列出：保留墓碑，跳过复活
       }
       ...
   }
   ```
   墓碑已被清 → 远端仍列出该订阅 → **重建本地 feed（复活）**。

### 为什么 2xx 不等于「远端已退订」

真实 GReader/MiniFlux 兼容后端在 token 失效、权限不足、或 `s=feed/<id>` 指向不存在的订阅时，
**可能以 200 + 错误体响应**。客户端只判 `is_success()`，看不到错误体，于是把
「请求被接受」误当成「远端确认删除」。

### 探针实证（2026-09-18，临时探针已删除、工作区已复原）

对 mock 注入「退订返回 2xx 但保留订阅」，实跑观察到：

```text
[probe B] unsubscribe_remote 返回: true      ← 把 2xx 当「远端确认」
[probe B] 墓碑已被清除: true                  ← 清掉唯一防线
[probe B] 同步后本地该订阅行数: 1             ← 已删订阅复活（缺陷成立）
```

对照组（远端确实移除）不复活，证明探针能区分两种结局、非环境噪声。

### 为何既有测试没抓到

`sync_gap_repro_e2e.rs::deleted_feed_stays_deleted_and_unsubscribes` 覆盖的是**正常路径**：
mock 的 `ac=unsubscribe` 分支**确实**把该订阅从 `subscriptions` 列表里删掉（`mock_greader.rs:579-591`），
随后第 ③ 段断言「远端不再列出 ⇒ 不复活」自然成立。
**缺陷恰在异常路径（2xx 但未生效）上**，此前无任何测试覆盖，故本任务必须新增。

## 修复方向（供参考，实现者可用更好的方案但要论证）

删除 `subscriptions.rs:46-49` 的清除点；墓碑只保留 `:201-202` 这一条清除条件
（远端订阅列表**确认已不含**该 URL）。

**须核对**：目录墓碑的对应清除点在 `:172-178`，判据取自远端 tag/list 的实际内容
（而非请求是否 2xx），初步看**不同源**；但任务要求实现者以代码为据明确回答，不得想当然。

## 兼容性影响

- `unsubscribe_remote` 的返回值今后仅表示「请求被后端接受」，不再等价于「远端已删除」。
  其唯一调用方 `commands/folders.rs:217` 忽略返回值（`let _ =`），故调用方无行为变化。
- 墓碑会**多留一段**（直到远端列表确认不含才清）；该表是 `app_settings` 里的 JSON 数组，
  有 `:201-202` 的收敛路径，不会无限增长。
- 不改任何对外请求的内容与顺序；不引入新依赖。
