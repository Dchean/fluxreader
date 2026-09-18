# 基线 · TASK-058（2026-09-18，TASK-057 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **141 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**255/255**（既有 26 + 新增 229） |

工作区：`git status --porcelain` 干净（除本轮新增证据文件）；`check` 全绿。

## 缺陷：后端已上报失败，前端不消费

### 证据 1：全仓无任何代码读取 `SyncReport.errors`

```
$ grep -rn '\.errors' src/ tools/
src/lib/api.ts:103:   errors: string[];      ← 仅类型声明，无读取
（tools/ 零命中）
```

即：`errors` 字段**只被写入、从未被读取**。

### 证据 2：后端在 errors 非空时仍返回 Ok（故 catch 不触发）

`src-tauri/src/sync/phases.rs`：

```rust
pub async fn feeds_phase(db, http) -> AppResult<SyncReport> {
    ...
    push_feeds(db, &client, &mut report).await;
    pull_feeds(db, &client, &mut report).await;
    Ok(report)          // ← 即使 report.errors 非空也返回 Ok
}
```

因此前端 `.catch()` **只**捕获真正的 reject（未连接、IPC 失败），
**不会**因「同步有失败项」而触发。

### 证据 3：两处调用点都丢弃 report

`src/components/settings/SyncTab.tsx:86-101`：

```tsx
void api
  .syncPhase('feeds')
  .then(async () => {                     // ← 回调不收参数，report 被丢弃
    await reloadFromBackend();
    showToast('已拉取订阅源，正在同步文章状态…');
    return api.syncPhase('states', true); // ← 同上
  })
  ...
  .then(() => {
    useAppStore.setState({ syncStatus: 'synced', syncConnected: true });
    showToast('后端同步完成');             // ← 无条件报成功
  })
```

`src/store/slices/sync.ts:74-83`：

```ts
void api
  .syncPhase('feeds')
  .catch(() => null)
  .then(async (feedsReport) => {
    if (feedsReport) {                    // ← 只判真，不读 .errors
      get().showToast('订阅同步完成，正在同步文章状态…');
      return api.syncPhase('states', true);
    }
    return null;
  })
```

### 证据 4：端到端实测（TASK-056 期间所做）

用测试库 `BEFORE INSERT` 触发器注入真实插入失败，驱动**真实运行的应用**：

```text
后端 sync_phase 返回：
  {"pushed_states":0,"pushed_feeds":0,"pulled_feeds":0,"pulled_entries":0,
   "merged_states":0,"fallback_entries":0,
   "errors":["拉取订阅 http://127.0.0.1:8898/uncat.xml 建本地失败: [db] e2e injected insert failure",
             "拉取订阅 http://127.0.0.1:8898/cat.xml 建本地失败: [db] e2e injected insert failure"]}

界面 toast：已拉取订阅源，正在同步文章状态…  →  后端同步完成
本地结果：feeds = 0
```

**即：后端如实报了 2 条失败，用户看到的却是「后端同步完成」，且数据零进来。**

## 为什么既有测试没抓到

前端 255 条断言覆盖 store 状态机、分页口径、文案与提示映射等，
但**没有一条**涉及 `SyncReport.errors` 的消费——因为该字段此前**根本没有消费者**，
写测试的人自然不会去断言一个没人读的字段。这与 TASK-056 的「零 folder 新库」同属
**结构性盲区**：测试跟着实现走，实现没走的路测试也不会走。

## 修复方向（owner 已授权，最小改动）

在两处 `syncPhase` 调用点读取返回的 report，当 `errors.length > 0` 时以既有
`showToast` 明示「已同步，其中 N 项失败」并给出具体原因（至少首条）。

## 边界

- **不改 Rust**：后端已在 TASK-056 完成上报，本任务只做前端消费（`src-tauri/**` 零改动为硬性证据项）；
- **不把 errors 非空改成抛错**（`Ok(report)` 是有意语义）；
- 成功路径（errors 为空）文案与顺序**逐字不变**；
- 不引入新依赖；
- **不写入用户真实数据库**（端到端测试如需改数据，先备份、结束后逐字节还原并给哈希证据）。
