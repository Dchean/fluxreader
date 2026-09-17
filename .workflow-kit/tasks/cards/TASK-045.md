<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-045 · 后端单体拆分二：sync.rs 按领域拆分为子模块（可维护性）

**状态**：done

**目标**：按 BRIEF 验收目标「后端 commands.rs/sync.rs 拆分完成，拆分过程测试不回归」，承接 TASK-044（commands.rs 已完成），把 src-tauri/src/sync.rs（1352 行）按既有章节边界拆为 sync/ 子模块。目标结构：sync/mod.rs 保留模块文档、SyncReport 等公共类型、以及**总入口**函数（feeds_phase / states_phase / sync_now / sync_light / test_connection，因为它们是跨领域编排，需要用到 Push 与 Pull 两侧的内部项）；sync/credentials.rs（凭据：Backend、read_credentials、build_client 及各协议客户端方法）；sync/push.rs（① Push：本地状态变更 → 后端）；sync/pull.rs（② Pull：远端 → 本地，含 GReader/Fever 两条实现）。所有 pub 项签名与行为保持不变；crate::sync::<fn> 的既有调用路径经 mod.rs 的 pub use 重导出保持逐字不变。

**依赖**：TASK-044
**参考方案**：REF-003
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-17-task045.md, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- sync.rs 已拆为 sync/ 子模块，单文件均不超过 400 行（用可信口径：LF 字节数 / splitlines / ReadAllLines 三种方法交叉验证，不得使用 PowerShell Measure-Object -Line）
- 所有 pub 项的签名与可见性保持不变（可用脚本对 git show HEAD 版与新文件做函数体逐字比对证明）
- crate::sync::<fn> 的既有调用路径不变：lib.rs / commands/ / scheduler / ingestion 等调用点零改动
- 跨子模块共用的内部项只做**最小必要的可见性放大**（private → pub(super)），且必须在报告中逐项列出「哪些项、因何放大」，不得冒充零改动
- cargo test 全量保持 120 passed / 0 failed（与 TASK-044 后的基线一致），ignored 数不增加
- cargo fmt --all -- --check 与 cargo clippy --all-targets -- -D warnings 零告警
- 无行为改动：无新增依赖、无逻辑重写、无错误文案变更

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-17 拆分前基线（本会话实跑）：cargo test 合计 120 passed / 0 failed / 23 ignored；cargo fmt --all -- --check 与 cargo clippy --all-targets -- -D warnings 全绿。本任务为模块拆分，不改变任何 pub 项签名与行为；判据是同一套测试在拆分后仍全绿且 ignored 数不增。与 TASK-044 的区别：sync.rs 的总入口章节会用到 Push/Pull 的内部项，故无法做到「零可见性改动」，必须在报告中逐项声明最小必要的 private→pub(super) 放大，并按此复核。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-17-task045.md
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：src-tauri/tests 下 20 个 e2e 套件与 src/ 内联测试模块；拆分必须证明对外行为未变；这些测试是唯一的行为契约保护，必须逐条保持通过且不得删改断言；验证：cargo_test
- 保留：cargo fmt 与 cargo clippy 门禁；拆分产生新文件，仍需格式与 lint 达标；clippy 的 -D warnings 可捕获可见性放大导致的未使用项告警；验证：cargo_fmt, cargo_clippy
- 保留：crate::sync::<fn> 的全部既有调用点；mod.rs 用 pub use 重导出以保持路径不变，调用点零改动是「无行为改动」的直接证据；验证：cargo_test, cargo_clippy

## 执行与恢复

- 首次开始：2026-09-17T06:50:44.165254Z
- 原截止时间：2026-09-17T10:50:44.165254Z
- 当前截止时间：2026-09-17T10:50:44.165254Z
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-17T07:15:42.953013Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T07:28:34.692075Z：[WinError 5] 拒绝访问。: 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-f497bfbd1e844dbdbac44df49ae00a0d.json.d0adc11f0f9349e484adc3571439dbff.tmp' -> 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-f497bfbd1e844dbdbac44df49ae00a0d.json'；下一步：先核对已有文件及原始日志，再处理 environment；不要新建任务或重置预算
- 2026-09-17T07:28:48.194290Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T07:41:51.026415Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-17T07:42:44.632801Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-17T07:57:20.634635Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-17T07:57:47.474247Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-17T08:07:28.559014Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-045.json)

- [RUN-7130172f81a245d88917e5e1793fda6f](../runs/RUN-7130172f81a245d88917e5e1793fda6f.json)
- [RUN-42e12876a3a3400b8c3a2034ef3f4267](../runs/RUN-42e12876a3a3400b8c3a2034ef3f4267.json)
- [RUN-f497bfbd1e844dbdbac44df49ae00a0d](../runs/RUN-f497bfbd1e844dbdbac44df49ae00a0d.json)
- [RUN-7167281d8bb04c70bf3475c7d6b22972](../runs/RUN-7167281d8bb04c70bf3475c7d6b22972.json)
- [RUN-753547645c574634ae3b1592965d181a](../runs/RUN-753547645c574634ae3b1592965d181a.json)
- [RUN-1aa3a553fd514aa199ea4ece844ca736](../runs/RUN-1aa3a553fd514aa199ea4ece844ca736.json)
- [RUN-2a86101c239a4c02abc7c453cd6d2596](../runs/RUN-2a86101c239a4c02abc7c453cd6d2596.json)
- [RUN-a30233ae24264dab9f38fce22854e6d8](../runs/RUN-a30233ae24264dab9f38fce22854e6d8.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
