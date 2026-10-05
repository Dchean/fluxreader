<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-105 · Rust 状态写入事务化——文章状态与待同步队列同生共死（REQ-002）

**状态**：ready

**目标**：统一单条状态写入的事务与同步语义（外部审计 AUDIT-20261005-core-consistency.md 点名的一致性缺口）：src-tauri/src/commands/articles.rs record_read_state / record_star_state 先 db::set_read/set_starred 再 db::enqueue_sync，无外层事务——第二步失败时「本地状态已改但没有待同步记录」，离线期间该变更将永不补推且可能被远端对账覆盖；set_read_bulk 批量路径逐条执行同样可能部分成功。修法要求：①record_read_state / record_star_state 的「状态写入+入队」复合操作包进单一 rusqlite 事务，与 db::mark_all_read_with_enqueue 既有事务口径一致（建议下沉为 db 层函数复用，commands 层只调度）；②set_read_bulk 改为整体单事务（任一 id 失败全部回滚，消除部分成功），语义与单条路径一致并在契约注释载明；③故障注入测试：入队步失败（返回 Err 的测试注入手法）→ 状态不落库，断言「状态变更与队列项同生共死」≥2 条（单条读/藏各一）；④不改命令签名、IPC 形状、push/对账行为；既有 cargo 测试不回退。coder 开工前先盘点 db::set_read/set_starred/enqueue_sync 的全部调用点，确认事务下沉不破坏其他调用方（如对账/导入路径）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src

## 验收标准

- ① record_read_state / record_star_state：状态写入与 enqueue_sync 在同一事务内，任一步失败全部回滚（cargo 测试锁定）
- ② set_read_bulk：整体单事务，无部分成功；契约注释载明语义（cargo 测试锁定）
- ③ 故障注入：入队失败 → 状态不落库（单条读/藏各 ≥1 条测试）
- ④ 既有命令签名/IPC 形状/push 行为不变；cargo test/fmt/clippy 全绿不回退；frontend 不回退
- ⑤ 独立审查（全新子代理，未参与编码）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@30cd2bc（v0.16.2 后）：外部审计 frontend 527/527；cargo 244 pass/0 fail/9 ignored（TASK-102 时代基线），本机 cargo check 预检通过。本卡为事务化加固，外部可见行为不变。
- 基线证据：.workflow-kit/docs/AUDIT-20261005-core-consistency.md
- 需求决定：DEC-refactor-roadmap-20261005
- 补充：故障注入测试：入队失败状态回滚（读/藏）+ bulk 全有全无；事务语义必须有失败路径证明；验证：cargo_test
- 保留：既有 cargo/frontend/fmt/clippy/lint/build 断言；不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-105.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
