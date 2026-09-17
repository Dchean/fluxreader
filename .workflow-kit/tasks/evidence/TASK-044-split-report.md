# TASK-044 交互/实施报告：commands.rs 领域拆分

**性质**：纯文件搬运（模块拆分）。不改函数签名、不改行为、前端零改动。
**结论**：`cargo test` 120 passed / 0 failed / 23 ignored，与拆分前基线**逐项一致**。

## 1. 拆分结果

`src-tauri/src/commands.rs`（**1369 行**，已删除）→ `src-tauri/src/commands/`：

| 文件 | 行数 | 内容（原章节） |
| --- | --- | --- |
| `mod.rs` | 90 | 模块文档、共享项（`read_dedup_flag`/`schedule_state_push`/`sync_configured`）、子模块声明与 `pub use` 重导出 |
| `folders.rs` | 327 | Folders / Feeds |
| `articles.rs` | 209 | Articles + 刷新（直连抓取） |
| `settings.rs` | 212 | Settings + 全文提取 + 图片代理（含 `image_proxy_tests`） |
| `opml.rs` | 81 | OPML 导入导出 |
| `sync.rs` | 296 | 后端同步 |
| `ai.rs` | 216 | AI 引擎（含 `ai_event_tests`） |

最大单文件 327 行（验收要求 ≤400，成立）。

> **行数口径更正（见 §6 返工第 3 条）**：本表初版为 67/281/180/181/70/258/172，
> 系 PowerShell `Measure-Object -Line` 少算所致；上表为三法交叉验证后的可信值。

## 2. 三个提升到模块根的共享项（依赖分析决定，非随意）

拆前先做了跨区块符号引用分析，发现 3 个符号被多个领域模块共用，
若随某个模块下沉会造成循环依赖或跨模块私有引用：

| 符号 | 被谁用 | 处置 |
| --- | --- | --- |
| `read_dedup_flag` | folders / refresh / settings | 留在 `mod.rs` |
| `schedule_state_push` | articles（3 处） | 留在 `mod.rs` |
| `sync_configured` | folders（3 处）/ articles / sync | 从 articles 摘出，留在 `mod.rs` |

`referer_candidates` 定义在图片代理段（`settings.rs`），但 `ai.rs` 的**测试**要用它
→ 该测试模块随 `settings.rs`，`ai.rs` 内测试引用改为 `use super::settings::referer_candidates;`。

## 3. 路径不变性（关键约束）

`lib.rs` 的 `invoke_handler` 有 44 处 `commands::<fn>`，
`tests/sync_gap_repro_e2e.rs` 有 10 处 `app_lib::commands::<fn>`（含 `record_*` 非命令函数）。
为**不动**这两处，`mod.rs` 用 `pub use <mod>::*;` 重导出全部子模块项，
使 `crate::commands::<fn>` 路径逐字不变。

**验证**：`git diff HEAD -- src-tauri/src/lib.rs` 为空（未改）。

## 4. 「纯搬运」的证明（不是我说，是比对出来的）

写脚本把原 `commands.rs` 与新 `commands/*.rs` 都按顶层 `fn` 切分，
去掉注释与空白后逐一比对函数体：

```
原文件函数数: 61
新文件函数数: 61
  缺失（原有无、新无）: 无
  新增（原无、新有）  : 无
  函数体有改动        : 无
=> ✓ 纯搬运成立
```

## 5. 门禁结果（本会话实跑，均重定向后读退出码）

| 门禁 | 结果 |
| --- | --- |
| `cargo build` | exit 0，0 error / 0 warning |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --all-targets -- -D warnings` | exit 0，Finished |
| `cargo test` | exit 0，**120 passed / 0 failed / 23 ignored**（基线同为 120/0/23） |

## 6. 实施过程中的两次返工（留痕）

1. **生成器过度 import**：第一版按"符号是否出现"补 `use`，但未剥离注释，
   导致文档注释里的词也被算作使用 → 14 个 `unused import` 警告（clippy `-D warnings` 会拒绝）。
   改为**先剥注释再判定**后归零。
2. **`mod.rs` 漏搬 header 区块**：第二版重建 `mod.rs` 时只搬了 `state_push`，
   漏掉定义 `read_dedup_flag` 的头部区块 → `E0432: unresolved import super::read_dedup_flag`（3 处）。
   补齐后通过；随后又因重复保留 `//!` 模块文档行触发 `E0753: expected outer doc comment`，一并修正。

两次都是**编译器抓出来的**，说明「生成代码后必须真编译」不是可选步骤。

3. **行数口径错误（我自己发现的，且是独立审查者临终前正在查的那一点）**：
   本报告与基线的行数初版取自 PowerShell `Get-Content … | Measure-Object -Line`，
   该口径**会少算**——对 327 行的 `folders.rs` 它报 281，对 1369 行的原 `commands.rs` 它报 1157。
   用三法交叉验证（LF 字节数 / Python `splitlines()` / .NET `ReadAllLines()`）后确认可信值，
   两处文档已同步更正。
   **为什么危险**：验收项「单文件 ≤400 行」如果建立在少算的口径上，就可能把超限文件判为合格。
   本次两种口径下最大值分别为 327 / 281，**均未超限**，故结论不变——但这是运气，不是方法。
   **教训**：报数必须指定并固定计量口径，且用第二种方法抽验；
   `Measure-Object -Line` 在本项目不可作为行数依据。

4. **任务卡 objective 里的括注行数不准确**：objective 写作「`commands.rs`（1157 行）」，
   真实为 1369。该数字是**描述性括注、非验收判据**（验收项只约束「拆分后单文件 ≤400 行」），
   且 objective 属受控字段（改动会变更 `definition_digest`），故**本次不擅自改写**，
   在此如实登记，交由独立审查者判断是否需要走订正流程。

## 7. 未做的部分（避免越界）

- 未拆 `sync.rs`（1163 行）与 `ingestion.rs`（660 行）——独立后续任务。
- 未改 `db/` 下的领域模块。
- 未改任何函数逻辑、错误文案、命令名。
- 前端零改动。
