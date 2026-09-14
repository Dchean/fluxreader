<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-023 · db.rs 内部模块化（db/ 领域子模块，对外路径不变）

**状态**：verified

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

- 首次开始：2026-09-14T13:36:22.243409Z
- 原截止时间：2026-09-14T16:36:22.243409Z
- 当前截止时间：2026-09-14T16:36:22.243409Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-14T13:36:22.703484Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-14T14:06:29.563619Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-14T14:18:56.696386Z：Interrupted run recovered: Interrupted verification RUN-511a462034ab444790b12ea60958937e: the enclosing shell call hit its timeout mid cargo-test-baseline; recorded pid no longer alive. fmt and clippy gates had already PASSED on candidate 9ff76a08; the test gate must be re-run in a background process.；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-14T14:22:23.683087Z：Required gate failed: cargo-test-baseline；下一步：先核对已有文件及原始日志，再处理 test_failure；不要新建任务或重置预算
- 2026-09-14T14:24:59.468084Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-14T14:26:13.717068Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-14T14:27:07.465650Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-14T14:27:26.722025Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-023.json)

- [RUN-63f2ab66fa5f44c9b04d4baad31b862f](../runs/RUN-63f2ab66fa5f44c9b04d4baad31b862f.json)
- [RUN-511a462034ab444790b12ea60958937e](../runs/RUN-511a462034ab444790b12ea60958937e.json)
- [RUN-13fb284109ce46e18ae971e0515d96ab](../runs/RUN-13fb284109ce46e18ae971e0515d96ab.json)
- [RUN-ae24bc57d3ee499581ed400e63de7357](../runs/RUN-ae24bc57d3ee499581ed400e63de7357.json)
- [RUN-b8288335579f46c5addac028c670cf9a](../runs/RUN-b8288335579f46c5addac028c670cf9a.json)
- [RUN-84d97d3dab0f4626810b73e1501aeb1d](../runs/RUN-84d97d3dab0f4626810b73e1501aeb1d.json)
- [RUN-c069f640cd7f46878e35640e213178d1](../runs/RUN-c069f640cd7f46878e35640e213178d1.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
