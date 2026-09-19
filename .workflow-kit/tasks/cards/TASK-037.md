<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-037 · 同步接线收尾：push 挂分类（A-3）+ 分类改名/删除防复活（A-4）

**状态**：done

**目标**：修复两个订阅/分类同步缺口。A-3：push_feeds 只取 feed_url、丢弃队列 payload 里的 folder_id——OPML 导入/add_feed 选的分类在远端全部落到默认分类；改为锁内解析 payload 目标分类名（本地 folder id→name），quick_add 成功绑定 remote_id 后追加 edit_subscription(remote_id, None, Some(label))。A-4：分类改名/删除后，pull_feeds 按远端 tag/list 旧 label 重新 create_folder（改名产生重复空目录、删除目录连带订阅一起复活）；引入 folder 墓碑（复用 feed 墓碑的 settings JSON 机制）：rename_folder 记录旧 label 墓碑、delete_folder 记录 label 墓碑并为其内每个 feed 补 feed 墓碑（否则订阅随目录复活），pull_feeds 建目录/建订阅前查墓碑并按「远端已不含即清墓碑」收敛。新增回归测试覆盖两个缺口。

**依赖**：TASK-032, TASK-036
**参考方案**：REF-001
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/**, src-tauri/tests/**

## 验收标准

- add_feed 携带分类的队列项在推送后，远端收到 edit 且 a=目标分类名（新测试断言）
- 改名后的旧分类名不再被 pull 复活为空目录；新名本地保留（新测试断言）
- 删除分类后其目录与其内订阅均不复活（新测试断言）
- 墓碑在远端确认不含后被清除（代码路径 + 断言）
- fmt 与 cargo test 全绿，既有同步契约不回归

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线：cargo test 113/0（含 A-1/A-2/A-5/C-1 转正场景）。行为变更仅限：推送订阅时携带目标分类；分类改名/删除不再被 pull 复活（墓碑）。均为用户确认的缺陷修复方向（FINDINGS-SYNC-GAP.md A-3/A-4，REQ-002/003 范围）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-defect-batch2-20260915
- 保留：既有同步契约测试（feeds/states 两阶段、A-1 墓碑、A-2 编辑推送）；不得回归；验证：rust-test
- 补充：A-3 分类挂载与 A-4 分类防复活回归测试；两缺口此前无覆盖；验证：rust-test

## 执行与恢复

- 首次开始：2026-09-15T15:16:49.113575Z
- 原截止时间：2026-09-15T19:16:49.113575Z
- 当前截止时间：2026-09-15T19:16:49.113575Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 20 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T15:16:49.219430Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T15:37:34.423252Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T15:37:40.787416Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T15:54:09.923358Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T15:54:52.450878Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-037.json)

- [RUN-53ad03d61e1c48328a9d2c8608a0539a](../runs/RUN-53ad03d61e1c48328a9d2c8608a0539a.json)
- [RUN-6afc8885c6754049af6ada84faa3a863](../runs/RUN-6afc8885c6754049af6ada84faa3a863.json)
- [RUN-09052397fdbd4acca220968527eff320](../runs/RUN-09052397fdbd4acca220968527eff320.json)
- [RUN-1534c957dfc14c9eb35242dce7fb8ee1](../runs/RUN-1534c957dfc14c9eb35242dce7fb8ee1.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
