# TASK-055 实施报告：修 `subscriptions.rs` 墓碑误清除 → 已删订阅被 pull 复活（P1）

- 运行：RUN-b7427f9111624a43b095cf8059657ff0
- 任务：TASK-055（批次 BATCH-201a8e98478f40e88c6f5a8dc54990b6，先补网后修缺陷的第二步）
- 授权：DEC-tombstone-and-ignored-tests-20260918
- 日期：2026-09-18

---

## 1. 缺陷与根因

`sync::unsubscribe_remote` 把 **HTTP 2xx 当作「远端已确认退订」**，据此清除删除墓碑：

```rust
let ok = match client { Backend::GReader(c) => c.unsubscribe(remote_id).await.is_ok(), ... };
if ok {
    let conn = db.lock().await;
    let _ = db::remove_feed_tombstone(&conn, feed_url);   // ← 误清除点
}
```

`ok` 的来源是 `greader.rs:323-334 post_form_text`，判据只有
`resp.status().is_success()`——它**看不到响应体**。真实 GReader/MiniFlux 兼容后端在
token 失效、权限不足、或 `s=feed/<id>` 指向不存在的订阅时，**可能返回 2xx + 错误体**。

墓碑是 `pull_feeds` 防复活的**唯一防线**：

```rust
for rf in &remote_subs {
    if tombstones.contains(&db::normalize_url(&rf.url)) {
        continue; // 本地已删除且远端仍列出：保留墓碑，跳过复活
    }
```

墓碑一旦被误清，而远端仍列出该订阅 → **已删订阅被重新建回本地**。
用户现象：**「删掉的订阅自己回来了」**。

## 2. 改动内容

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/sync/subscriptions.rs` | 删除 `:46-49` 的墓碑清除点；重写文档注释说明返回值语义 |
| `src-tauri/src/commands/folders.rs` | 更新调用处注释（不再声称「成功则清除墓碑」） |
| `src-tauri/tests/mock_greader.rs` | 新增故障注入开关 `unsubscribe_returns_2xx_without_removing`（仿既有 `fail_stream_ids` 写法） |
| `src-tauri/tests/sync_gap_repro_e2e.rs` | 新增复现测试；并**适配** A-1 既有测试中被本任务有意改变的断言 |

**墓碑清除条件现收口为唯一一处**：`pull_feeds` 中「远端订阅列表**实际已不含**该 URL」时清除
（`subscriptions.rs` 内 tombstone 收敛段）。这是唯一有证据支撑的判据。

**`unsubscribe_remote` 签名未变**，返回值语义细化为「请求是否被后端接受（2xx）」。
唯一调用方 `commands/folders.rs` 本就 `let _ =` 忽略返回值，故调用方行为无变化。

## 3. 验证：fail-before / pass-after（验收项要求）

新增测试 `unsubscribe_2xx_without_removal_keeps_tombstone_and_no_revive`，
注入「退订回 200 但服务端保留订阅」，断言三段：
① 墓碑必须保留；② 再次同步不得复活；③ 远端最终确认移除后墓碑才由 pull 收敛清除。

**把修复回退后用同一测试复跑**（并强制重编译，确认出现 `Compiling app`）：

```text
[FAIL-BEFORE] exit=101 compiled=True
    panicked at tests\sync_gap_repro_e2e.rs:291:9:
    （断言：仅收到 2xx 不足以确认远端已删除，墓碑必须保留）
[RESTORE] identical: True
[PASS-AFTER] exit=0 compiled=True
    test result: ok. 1 passed; 0 failed
```

即：**修复前该测试确实失败、修复后确实通过**，捕获性成立（非「恒真断言」）。

## 4. 对既有测试的影响（A-1 断言适配）

`deleted_feed_stays_deleted_and_unsubscribes`（A-1）第 ② 段原断言：

```rust
assert!(db::feed_tombstones(&conn).unwrap().is_empty(), "退订成功（远端确认）后墓碑应清除");
```

该断言**断言的正是本任务有意改变的旧行为**（「2xx ⇒ 墓碑清除」），故修复后必然失败。
处置：把该断言**适配**为「仅收到 2xx 时墓碑必须保留」，并把「墓碑应清除」的检查**移到第 ③ 段**——
即远端 mock 已按真实行为移除该订阅、pull 得以确认之后。该测试其余三段（墓碑写入、
远端收到 `ac=unsubscribe`、远端移除后不复活）**一字未改**。

这属于工作流 GATES 规定的 **adapt**（适配实现依赖），不是削弱：新断言比旧断言**更强**
（旧断言只要求「清掉」，新断言要求「在正确条件下清、在错误条件下保留」）。

## 5. 同类路径核对（验收项要求）

### 5.1 目录墓碑是否有同类误判 —— **没有**

`subscriptions.rs` 的 folder tombstone 收敛段判据取自**远端 tag/list 的实际内容**：

```rust
for stale in tombstones {
    if remote_labels.iter().any(|l| l == &stale) { active.push(stale); }
    else { let _ = db::remove_folder_tombstone(&conn, &stale); }
}
```

它比较的是**响应体里实际返回的分类 label**，与「请求是否 2xx」无关，因此**不存在同类误判**。
（该段不依赖任何写请求的成功与否。）

### 5.2 `remove_feed_tombstone` 全部调用点排查

全仓仅两处（修复后）：

| 位置 | 判据 | 是否可靠 |
| --- | --- | --- |
| `subscriptions.rs` tombstone 收敛段 | 远端订阅列表**实际已不含**该 URL | **可靠**（读响应体） |
| ~~`unsubscribe_remote` 内~~ | ~~HTTP 2xx~~ | **本次已删除** |

`remove_folder_tombstone` 亦仅一处，判据同为响应体内容（见 5.1）。**无其它「把 2xx/Ok 当远端确认」的清墓碑点。**

## 6. 门禁结果

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **137 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，241/241 |

**137 = 136（TASK-054 基线）+ 1（本任务新增测试）**；ignored 保持 9 未增。

## 7. 协议语义与兼容性影响

- **不改任何对外请求的内容与顺序**：`ac=unsubscribe`、`s=feed/<id>` 与请求时机完全不变。
  本次只改**收到响应后的本地处理**（不再据 2xx 清墓碑）。
- **不改推送顺序、冲突解决策略、对账口径**。
- **不改 `greader::post_form_text` 的通用 2xx 判据**（那波及全部写接口，超出本任务范围）。
- **墓碑会多留一段**，直到远端列表确认不含才清。该表是 `app_settings` 内的 JSON 数组，
  有 `pull_feeds` 的收敛路径，且远端确认后必被清除，**不会无限增长**。
- 未引入新依赖；`Cargo.toml` / `Cargo.lock` 零改动；未改前端。

## 8. 边界与遗留

- 未修 TASK-054 发现的 P1/P4 两个弱断言测试（独立后续项）。
- 未修 `folder_tombstones` 相关逻辑（经核对无同类缺陷，见 5.1）。
- 未修 REQ-007 其它未修项。
- 未改工作流（`.workflow-kit/scripts/**`、`binding.json`）。
