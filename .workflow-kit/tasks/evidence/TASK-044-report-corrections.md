# TASK-044 报告更正说明（审查后补充，不改动已绑定证据）

独立审查（`TASK-044-review-report.json`，verdict=PASS）在结论中附带指出
`TASK-044-split-report.md` 有**两处行文不准确**，均**不影响任何验收判据**。
本文件用于更正，**刻意不改动原报告**——原报告已被该次审查的 evidence 绑定，
事后编辑会触发 `review evidence changed after approval` 自锁（本项目已记录该缺陷）。

## 更正 1：`lib.rs` 的 `commands::<fn>` 是 **43** 处，不是 44

原报告 §3 写「44 处」。实测三种独立方法一致：

```
Select-String -AllMatches 计数 : 43
逐行去重计数                   : 43
.NET ReadAllLines 计数         : 43
并且 == commands/ 下 #[tauri::command] 函数数 : 43
```

审查者推断「44 可能把 `lib.rs` 内另行注册的 `resolve_close` 计入」。
**影响**：无。验收判据是「`invoke_handler` 注册列表逐字未变」，
而 `git diff HEAD -- src-tauri/src/lib.rs` 为空（未改动一个字节），
与具体计数无关。

## 更正 2：`image_proxy_tests` 的归属与引用写法

原报告 §2 末句写「`ai.rs` 内测试引用改为 `use super::settings::referer_candidates`」——
**与代码不符**。实际（已核实）：

| 位置 | 内容 |
| --- | --- |
| `src/commands/settings.rs:174` | `mod image_proxy_tests`，其内 `use super::referer_candidates;`（第 175 行） |
| `src/commands/ai.rs:201` | `mod ai_event_tests`，其内 `use super::AiEvent;`（第 202 行） |

即：**两个测试模块各自随宿主文件迁移**——`image_proxy_tests` 跟着 `referer_candidates`
留在 `settings.rs`（同模块内 `super::` 正确解析），`ai_event_tests` 跟着 `AiEvent` 在 `ai.rs`。
`referer_candidates` 因此**保持 private，未放宽可见性**——这是正确处理，
与我原报告里描述的「跨模块引用改写」不同。

**为什么原描述是错的**：我的生成脚本里确实有一条「把 `use super::referer_candidates;`
改写为 `use super::settings::referer_candidates;`」的兜底逻辑，但该逻辑**从未触发**——
因为切分时我把 `image_proxy_tests` 放进了 `settings.rs` 的块，它与定义同处一个模块，
本就不需要跨模块路径。报告里我按「脚本会这么做」写了预期，**而非按实际产物写**。

**教训**：报告必须描述**实际产物**，不能描述生成脚本的意图或兜底分支。

## 复核方式（可复跑）

```powershell
Select-String -Path src-tauri\src\lib.rs -Pattern "commands::" -AllMatches | ForEach-Object { $_.Matches.Count } | Measure-Object -Sum
Select-String -Path src-tauri\src\commands\ai.rs,src-tauri\src\commands\settings.rs -Pattern "mod .*tests|use super::"
```
