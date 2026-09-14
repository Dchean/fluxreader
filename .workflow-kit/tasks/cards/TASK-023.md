<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-023 · db.rs 内部模块化（db/ 领域子模块，对外路径不变）

**状态**：ready

**目标**：把当前 3516 行的 src-tauri/src/db.rs 拆分为 db.rs（模块根：子模块声明 + pub use 再导出）加 db/ 领域子模块（按现有函数分组，如 migrations / folders / feeds / articles / search / settings / sync_queue / dedup / tests），函数体逐字移动，使 crate::db:: 的全部既有调用路径零变化。这是渐进重构的首个试点：先验证“大文件按领域拆分”可行且旧行为不变，再决定是否扩大到其他模块。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/db.rs, src-tauri/src/db/**, src-tauri/tests/**

## 验收标准

- cargo test 全量通过，通过数不少于基线 105 项（TASK-022 记录 105/0/23）
- cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings 退出 0
- cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check 退出 0
- db.rs 根文件不再包含函数实现体，仅保留模块声明与 pub use 再导出
- crate::db:: 既有调用路径零变化：lib.rs、commands.rs、sync.rs、scheduler.rs、ingestion.rs、config_sync.rs 等不得被修改
- 拆分前后 db 模块公开符号集合一致（管理 agent 对照函数签名清单）
- 不改变任何函数签名、SQL 语义、锁纪律与调用时序；不引入新依赖；无数据迁移

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-023.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
