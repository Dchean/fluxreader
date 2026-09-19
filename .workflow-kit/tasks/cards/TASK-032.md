<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-032 · 同步状态链路修复：对账防误判（C-1）+ 离线变更一律入队（A-5）

**状态**：done

**目标**：修复同步状态链路两个高优先缺口：(1) C-1——GReader/Fever 状态集合拉取失败时，当前 unwrap_or_default() 把错误当空集合，对账会静默清空本地收藏（GReader）或把本地全部改为已读并可能回滚未读（Fever）；改为任一集合拉取失败即跳过该轮对账并记 report.errors。(2) A-5——set_read/set_starred 仅在 sync_configured 时入队，离线变更无持久化待推记录、连接后不补推；改为无论是否 configured 都入队，推送段在未配置时静默跳过。同步把 offline 复现测试转正为必过（断言反转为：连接后 mock 收到 edit-tag），并新增对账失败跳过的测试。

**依赖**：无
**参考方案**：REF-001
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- GReader/Fever 任一状态集合拉取失败时跳过对账且 report.errors 有记录（新增测试覆盖）
- 未配置凭据时 set_read/set_starred 仍写入 sync_queue，配置后连接可补推
- offline_read_change_never_pushed_after_connect 转 正为必过并反转为期望行为
- A-1/A-2 复现测试保持 #[ignore] 不受影响；cargo test 默认集与 fmt 全绿

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线（8966ece）cargo test 108/0；TASK-030/031 后工作区 cargo test 108/0 且新增 3 个 #[ignore] 复现测试（实跑复现成立）。行为变更仅限：拉取失败不再当作空集合对账；离线状态变更改为入队待推。两者均为用户确认的缺陷修复方向（FINDINGS-SYNC-GAP.md 修复设计，REQ-002/003 范围）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-defect-batch2-20260915
- 保留：Rust 默认测试集与既有 mock e2e 同步契约；既有 push/pull/对账期望行为不得回归；验证：rust-test
- 适配：offline_read_change_never_pushed_after_connect 复现测试转正；A-5 修复后该场景行为变更为期望行为，断言反转为收到补推；验证：rust-test
- 补充：对账拉取失败跳过（不再当空集合）的回归测试；C-1 此前无覆盖，属破坏性路径；验证：rust-test

## 执行与恢复

- 首次开始：2026-09-15T10:00:22.456187Z
- 原截止时间：2026-09-15T11:30:22.456187Z
- 当前截止时间：2026-09-15T11:30:22.456187Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 34 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T10:00:22.505452Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T10:35:07.434330Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T10:35:13.221916Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T10:52:12.613965Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T10:55:05.384751Z：Required gate failed: rust-test；下一步：先核对已有文件及原始日志，再处理 network；不要新建任务或重置预算
- 2026-09-15T10:55:32.545495Z：[WinError 5] 拒绝访问。: 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-4f4b972a7a8641778f23d40f7e259624.json.ce752e6799ee4a34a56d0b924d70806b.tmp' -> 'D:\\fluxreader\\.workflow-kit\\tasks\\runs\\RUN-4f4b972a7a8641778f23d40f7e259624.json'；下一步：先核对已有文件及原始日志，再处理 environment；不要新建任务或重置预算
- 2026-09-15T10:57:48.934539Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T11:06:26.220400Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-032.json)

- [RUN-d25989b7e9264a4a920532cc117aa041](../runs/RUN-d25989b7e9264a4a920532cc117aa041.json)
- [RUN-1f23fb96af194297b85e141172527a70](../runs/RUN-1f23fb96af194297b85e141172527a70.json)
- [RUN-e54813bc695d4c848b308703d5a676c7](../runs/RUN-e54813bc695d4c848b308703d5a676c7.json)
- [RUN-f1ff8ca41b764144aca78422cebcfd0c](../runs/RUN-f1ff8ca41b764144aca78422cebcfd0c.json)
- [RUN-4f4b972a7a8641778f23d40f7e259624](../runs/RUN-4f4b972a7a8641778f23d40f7e259624.json)
- [RUN-da3e6bd5e55f4aefa4128126dc7d202d](../runs/RUN-da3e6bd5e55f4aefa4128126dc7d202d.json)
- [RUN-b27ce46217664f61a9ff42dabd3fe010](../runs/RUN-b27ce46217664f61a9ff42dabd3fe010.json)
- [RUN-d025a20594e04facb909be47421c4f12](../runs/RUN-d025a20594e04facb909be47421c4f12.json)
- [RUN-a1f4f2e1c7914ec08fabb0c57e007426](../runs/RUN-a1f4f2e1c7914ec08fabb0c57e007426.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
