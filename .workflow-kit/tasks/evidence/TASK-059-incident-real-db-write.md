# TASK-059 事故记录：实机验证误写用户真实数据库（已还原）

- 任务：TASK-059（批次 BATCH-6108ba756ba5490f924879b7f22b2f18）
- 发生/发现：2026-09-18 夜 ~ 2026-09-19 01:00（本地）
- 处置：**已逐字节还原，哈希一致**（见文末证据）
- 影响范围：`%APPDATA%\com.fluxreader.app\fluxreader.db` 及其 `-wal` / `-shm`

## 1. 事故是什么

任务卡明文要求：

> **用户真实数据库不得写入**：`%APPDATA%\com.fluxreader.app` 只读使用；
> 端到端测试如需改数据，须先备份并在结束后逐字节还原（给出哈希一致的证据）。

我为实机验证启动了 debug 构建的应用，并**以为**用环境变量把它指向了隔离目录：

```bash
APPDATA='C:\Users\A\AppData\Local\Temp\t059-appdata' ./target/debug/app.exe
```

**这个做法无效。** Tauri 的 `app.path().app_data_dir()`（`src-tauri/src/lib.rs:125`）
走 Win32 已知文件夹 API（`FOLDERID_RoamingAppData`），**不读 `APPDATA` 环境变量**；
应用因此照常打开并写入了**真实库**。

破坏性的一步是「保存并同步」：`sync_save` 检测到「换账号」
（协议/endpoint/username 与旧值不同）后，会**执行 `purge_remote_data`**
——清理上一账号从服务端拉取的订阅与文章（`src-tauri/src/commands/sync.rs`）。
我的测试凭据 `http://127.0.0.1:8901` + `demo` 与任何真实账号都不同，
因此这一分支必然触发。

## 2. 怎么发现的

驱动脚本最后一节本应打印「后端请求序列」，实测 `total: 0`。
据此怀疑请求没打到预期端口，进而核对应用数据目录，发现

```
%APPDATA%\com.fluxreader.app\fluxreader.db-wal   2575032 字节  ← 我的测试写入了这里
%APPDATA%\com.fluxreader.app\fluxreader.db          4096 字节
```

而不是我以为的隔离目录。**发现方式是「证据对不上」而不是「我记得我隔离了」**——
与 TASK-058 记录的「深色截图实测是浅色」属同一类：我记录的是**我以为的状态**。

## 3. 还原方法（WAL 帧重放）

WAL 模式下，checkpoint 之前的历史帧仍留在 `-wal` 里，因此可以按帧序重建历史状态。

1. **先保全现场**：把事故后的 db / wal / shm 三件套复制到
   `%TEMP%\t059-incident\`（原件未动），记录哈希；
2. 解析 `-wal` 的 24 字节帧头，按页号重放 `settings` 表所在页（rootpage=10），
   逐帧打印历史版本（`tools/t059_restore_db.py`、`tools/t059_wal_recover.py`）；
3. 定位「我的会话接管」的那一帧：**frame 77** 首次出现测试端点 `http://127.0.0.1:8899`；
4. 重放 `[0, 76]` 帧得到**会话开始前**的状态，与我在会话一开始读到的事实**逐项吻合**：

   | 项 | 还原结果（frame 76） | 会话开始时实测 |
   | --- | --- | --- |
   | feeds / folders / articles | 0 / 0 / 0 | 0 / 0 / 0（侧栏「全部 0」） |
   | sync_endpoint / username / password | `''` / `''` / `''` | `''`（Endpoint 输入框空） |
   | sync_last_sync | `'0'` | 未连接 |

   两者一致，说明 frame 76 就是我的会话接手时的状态；
5. 用该还原结果覆盖真实库，并**删除** `-wal` / `-shm`（还原目标状态下它们不存在），
   逐文件比对哈希。

## 4. 关于「用户原有的 miniflux 配置」

WAL 历史里确实存在过一份**真实账号**配置：

```
frame 71:  greader_endpoint = 'https://reader.miniflux.app/'
           greader_username = 'demo'
           greader_password = 'dpapi:AQAAANCMnd8BFdERjHoAwE/Cl+sBAAAA…'   ← DPAPI 密文
           sync_last_sync  = '1789737254'
```

但紧接着在同一段历史内，配置被**清空**（frame 73→76：endpoint/username/password 置空、
`sync_last_sync` 归 0，即「断开连接」的效果），**且全部发生在 frame 77 之前**——
也就是发生在我的会话开始之前。因此：

- 我的测试**覆盖**的是「已断开、空配置」的状态，**不是**这份 miniflux 配置；
- 还原目标取 frame 76，与该状态一致，**没有把用户的配置改动或删除**；
- 我**没有**擅自把 frame 71 的 miniflux 配置写回——那会把用户在更晚时刻做出的
  「断开连接」选择反向覆盖掉。这是用户自己的状态决定，不该由我替他改。

> **请注意**：此处判断依据是 WAL 帧序。若 owner 认为「断开连接」是误操作、
> 希望恢复 miniflux 账号，则 frame 71 的状态可从
> `%TEMP%\t059-incident\fluxreader.db-wal` 重放取回（DPAPI 密文需在**同一 Windows 用户**
> 下才能解密，还原时不可跨用户/跨机器搬运）。**这属于用户数据决定，需 owner 明确指示。**

## 5. 逐字节还原证据

还原来源：会话开始前的快照（由 frame 76 重放得到，另存 `%TEMP%\t059-pre-launch.db`）

```
fluxreader.db       expected 419b118e050a08c204a50603f2d5188d8132fa94d75ea417392fe7e984293550
                    actual   419b118e050a08c204a50603f2d5188d8132fa94d75ea417392fe7e984293550
                    → MATCH
fluxreader.db-wal   还原为「不存在」（快照状态下无此文件）
fluxreader.db-shm   还原为「不存在」（快照状态下无此文件）
```

还原后核对（`tools/t059_check_real_db.py`，只读复制后打开）：

```
settings count = 7 ；feeds = 0 ；folders = 0 ；articles = 0
同步设置：greader_endpoint='' greader_username='' sync_last_sync='0' sync_protocol='greader'
```

`tools/t059_ui_e2e.mjs` 现已在结果 JSON 里固定记录 `0.`（改动前备份哈希）与
`Z.`（还原哈希校验，`allOk: true`）两节，见
`TASK-059-ui-e2e-result.json`。

## 6. 防再犯（已落到工具里）

1. **驱动强制备份 + 还原**：`tools/t059_ui_e2e.mjs` 在改动应用状态前备份三件套，
   结束时 `taskkill` 应用（Windows 上运行中的应用持有 db 句柄，直接覆盖会
   `UNKNOWN: unknown error, copyfile` —— 这个失败**实测踩到过一次**，
   那轮「还原」等于没做）、覆盖回去、逐文件校验哈希，并把结果写进证据 JSON；
2. **推荐用 `--restore-db`**：传入**应用启动之前**取的真实库快照，
   还原目标就是「验证之前的字节」，而不是「驱动开始跑时」的字节；
3. **不再声称「已隔离」**：文件头以事故记录写明
   `APPDATA` 对 Tauri 无效，避免下一个人重犯；
4. **报告须如实**：本文件即按「先写现象、再写根因、再写证据」的次序记录，
   不美化、不省略。

## 7. 保留的现场文件（未随还原删除）

| 路径 | 内容 |
| --- | --- |
| `%TEMP%\t059-incident\fluxreader.db{,-wal,-shm}` | **事故后**的三件套原件副本（含全部 WAL 帧） |
| `%TEMP%\t059-incident\pre-restore-fluxreader.db` | 还原动作前的库副本 |
| `%TEMP%\t059-pre-launch.db` | 还原所用的会话前快照（哈希 `419b118e…`） |
| `%TEMP%\t059-restored-pre-session.db` | 同上，另存一份 |

如需复核或取回 frame 71 的账号配置，用
`python tools/t059_restore_db.py --db <incident>/fluxreader.db --wal <incident>/fluxreader.db-wal --stop-before 72`。