<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-036 · 订阅改名/移动目录接线：edit_subscription 推送远端（A-2）

**状态**：done

**目标**：修复 A-2：update_feed 此前只更新本地（greader.edit_subscription 已实现但全仓零调用，Backend 未暴露）——改名/移动目录后远端标题与分类永久分歧。实现：Backend 暴露 edit_subscription（GReader 真实调用；Fever 协议无编辑端点，按 no-op 跳过）；命令层抽出 record_feed_edit（本地更新 + 返回待推送目标：remote_id、新标题、目标分类 label），update_feed 在锁外 best-effort 推送远端，失败仅记日志不影响本地生效。A-2 复现测试转正为必过。

**依赖**：TASK-035
**参考方案**：REF-001
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- 改名后向远端发送 ac=edit 且 t=新标题（mock 断言）
- 移动目录后发送 a=目标分类名（mock 断言）
- Fever 后端为 no-op，不报错、不影响本地更新
- 未绑定远端或未配置同步时仅本地更新（不推送、不报错）
- 原复现测试 feed_rename_never_reaches_backend 转正为必过；fmt 与 cargo test 全绿

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线（TASK-035 后）：cargo test 111/0。行为变更：订阅改名/移动目录现在会 best-effort 推送远端（GReader ac=edit），此前仅本地生效——用户确认的缺陷修复（FINDINGS-SYNC-GAP.md A-2，REQ-002 范围）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-sync-wiring-batch3-20260915
- 保留：Rust 默认测试集与既有同步契约；既有行为不得回归；验证：rust-test
- 适配：feed_rename_never_reaches_backend 复现测试转正；A-2 修复后改名会推送远端，断言反转为已推送；验证：rust-test
- 补充：移动目录推送（a=分类名）回归断言；改名与移动目录为两条独立参数路径；验证：rust-test

## 执行与恢复

- 首次开始：2026-09-15T14:16:12.871780Z
- 原截止时间：2026-09-15T15:46:12.871780Z
- 当前截止时间：2026-09-15T15:46:12.871780Z
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T14:16:12.944130Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T14:24:37.003341Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T14:24:43.890207Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T15:09:25.651284Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T15:12:53.475807Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-036.json)

- [RUN-29d7d97da4e5462aa387986fc0f33347](../runs/RUN-29d7d97da4e5462aa387986fc0f33347.json)
- [RUN-4dab8cfd113b45b69752423529b40e8e](../runs/RUN-4dab8cfd113b45b69752423529b40e8e.json)
- [RUN-e6fb6491154b498ca606e6777677a2aa](../runs/RUN-e6fb6491154b498ca606e6777677a2aa.json)
- [RUN-aa106880d22c4764aac1ff05b1d56bb9](../runs/RUN-aa106880d22c4764aac1ff05b1d56bb9.json)
- [RUN-e894342f8329469a8127b5f4c1b4bbcf](../runs/RUN-e894342f8329469a8127b5f4c1b4bbcf.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
