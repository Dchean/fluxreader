# TASK-044 基线（2026-09-17）：commands.rs 拆分前

采集于提交 `5b3589b` 之上，工作区干净（`git status` 空）。

## 拆分前规模

| 文件 | 行数 |
| --- | --- |
| `src-tauri/src/commands.rs` | 1369 |
| `src-tauri/src/sync.rs` | 1352 |
| `src-tauri/src/ingestion.rs` | 766 |

> **口径更正（2026-09-17，本任务自曝）**：上表初版写的是 1157 / 1163 / 660，
> 那组数字来自 PowerShell `Get-Content ... | Measure-Object -Line`，该口径会**少算**
> （对 327 行的 `folders.rs` 它报 281）。可信行数用三法交叉验证并一致：
> LF 字节数、Python `splitlines()`、.NET `ReadAllLines()`。
> 更正后同步影响：`commands.rs` 真实为 1369 行；验收项「单文件 ≤400 行」在新口径下
> 依然成立（拆分后最大 327 行）。

`db.rs` 已完成拆分（TASK-023 试点）：`db/` 下 8 个领域子模块 + 3 个内联测试模块。

## Rust 测试基线（本会话实跑，非引用旧记录）

```
cargo test --manifest-path src-tauri/Cargo.toml
=> 120 passed / 0 failed / 23 ignored   （完整日志见 baseline-rust-tests-2026-09-17.log）
```

分布：内联单测 71；`sync_gap_repro_e2e` 11；`mock_greader` 5；`ingestion_e2e` 7；
`sync_content_e2e` 4、`dual_client_e2e` 4、`staged_refresh_e2e` 3、`config_sync_e2e` 3、
`account_lifecycle_e2e` 2、`migration_test` 2、`scheduler_e2e` 1、`regression_e2e` 3 等。

**注意**：`cargo test` 在本环境用管道读取时退出码可能被掩盖（曾见 `exit code: 1` 而实际全绿）。
故门禁一律**重定向到文件后读退出码**判定，并以日志里的 `test result:` 行交叉核对。

## commands.rs 的既有章节边界（拆分依据）

| 起始行 | 章节 |
| --- | --- |
| 22 | 即时状态推送调度（防抖合批） |
| 65 | Folders / Feeds |
| 384 | Articles |
| 553 | 刷新（直连抓取） |
| 589 | Settings |
| 617 | 全文提取（Readability） |
| 683 | 图片代理（防盗链兼容）——参考 Papr 方案 |
| 756 | OPML 导入导出 |
| 831 | 后端同步 |
| 1120 | AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi 中转） |
| 1312 / 1330 | 内联测试模块 |

拆分即**沿这些既有边界**搬运，不重新设计职责。

## 本任务的性质

纯文件搬运（模块拆分）：函数签名、可见性、命令名、注册列表全部不变，前端零改动。
判据 = **同一套测试拆分后仍全绿，且 ignored 数不增**（不得用 `#[ignore]` 掩盖问题）。
