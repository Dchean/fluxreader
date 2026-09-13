# Agent Note: M2 模块处置方案与 SQL 边界收敛试点

Status: proposed

## Problem

ARCHITECTURE 明确三项现状：SQL 并未全部集中在 db.rs（commands.rs 13 处、sync.rs 17 处直接 SQL，ISSUE-008）；store.ts 1494 行聚合 22+ 职责（ISSUE-009）；目标架构从未形成。M2 要求给出逐模块处置并完成一条代表性试点。

## Proposal

处置方案全文见 [docs/runs/M2-module-disposition-20260913.md](../../../docs/runs/M2-module-disposition-20260913.md)（TASK-016 分析，管理 agent 抽查量化数据一致）。结论：

- **保留**（不整理）：api.ts、selectors.ts、全部组件层、ingestion/extraction/sanitize/scheduler/config_sync/github_auth/opml/credentials/media/ai/greader/fever——职责单一、SQL 边界清晰、支撑 OPT-001～010。
- **整理**（非重写）：commands.rs（13 处 SQL 移入 db.rs + 按领域拆命令组）、db.rs（全量收敛 SQL + 内部模块化）、sync.rs（17 处 SQL 移入 + Push/Pull/协议分层）、store.ts（按领域拆 slice，保持 Zustand 单 store 与对外 API 不变）。
- **代表性试点（TASK-017）**：commands.rs 的 13 处 SQL 收敛至 db.rs 类型化函数——范围最小（约 100 行）、现有测试已覆盖主路径、直接代表 ISSUE-008 的解决模式。验收：`grep execute/query_row/prepare commands.rs` 归零 + 新函数有测试 + 全量回归通过。
- 后续序列（编号顺延既有任务）：store.ts 拆 slice → db/sync 全量收敛 → sync 分层 → commands 拆组。

## Alternatives considered

### 不做：维持现状，靠审查约定保持边界

最强理由是零回归风险，现有 83 项 Rust 测试全绿。

不采用的理由：ISSUE-008 的 30 处散布 SQL 是每次数据层改动的重复风险源；db.rs 注释承诺与事实不符会持续误导接手者（ISSUE-005 同源）。M2 的用户指令（DEC-013 跑完优化重构）正是要求处理它。

### 用 ORM（Diesel/SeaORM）替换裸 SQL 顺带解决边界

最强理由是类型安全查询天然集中。

不采用的理由：强制重写全部 SQL 与迁移，SQLite 个人应用场景下裸 SQL 更直接可读，且引入新依赖属用户保留决定的高风险边界。

### 先拆 store.ts 再做 SQL（前端先行）

最强理由是前端职责数（22+）比后端 SQL 散布更显眼。

不采用的理由：SQL 收敛范围更小、纯后端、有更完整的回归测试网（migration_test + 同步 E2E），作为验证"目标边界"模式的试点风险最低；store.ts 拆分涉及组件 import 面，适合作为第二阶段。

## Acceptance criteria

- 试点（TASK-017）后 commands.rs 无直接 SQL；新增 db:: 函数有测试；全量回归通过。
- 后续各整理任务逐项验收后，本 Note 随最后一个整理任务转 implemented；任何任务不得缩减 FEATURES 保留能力。

## Risks

- SQL 迁移可能因锁纪律（锁内读写、锁外 HTTP）引入微妙变化——迁移仅移动语句位置，不改锁范围；由既有 E2E 与审查覆盖。
- db.rs 收敛后体量增大——内部模块化（db/ 子模块）随全量收敛任务执行，避免单文件继续膨胀。
