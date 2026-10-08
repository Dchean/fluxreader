<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-125 · 协议能力矩阵三列化与冲突应用层错误可见——能力/实现/验证分层（审计 P2-8）

**状态**：cancelled

**目标**：修复第三方审计 P2-8（tmp/audit-20261007/REVIEW.md 第 8 节）：①docs/sync-compat-matrix.md 把 Fever 写成「无全量历史端点」，否定当前 Miniflux/Fever 服务端的 max_id 向更旧翻页能力（handler.go:227-267，2026-10-07 读取），属把「本客户端未实现」说成「协议做不到」；②矩阵把「协议/服务端支持」「本客户端已实现」「指定服务端版本已验证」三类事实混在一处，无法作为验收矩阵；③conflict_policy.rs 的 apply_read_by_policy/apply_star_by_policy 用 unwrap_or(0) 把 DB 写失败吞成「没有变化」，调用方无法区分。修法要求：(A) docs/sync-compat-matrix.md 增加三列表头，逐行把每格事实归入「协议/服务端支持」「本客户端已实现」「指定服务端版本已验证」三列——Fever 行必须如实写明：协议支持 items&max_id（< max_id 向更旧翻页，重复直到空数组），而本客户端当前未实现历史回溯（仅最近 50 条种子 + unread/saved 权威集合 with_ids 补齐），「指定服务端版本已验证」列标注当前仅覆盖 mock 替身与（若确有）已实测版本，Miniflux/FreshRSS 具体版本未逐一验收；GR 行的 conflicts 政策（read-wins）同理标注为「本客户端选择的冲突策略」而非「协议必然要求」。(B) src-tauri/src/sync/conflict_policy.rs：apply_read_by_policy/apply_star_by_policy 由返回 usize 改为返回 AppResult<usize>（DB 写失败向上传播，不再 unwrap_or(0) 吞错）。(C) 两个调用方 src-tauri/src/sync/greader_pull.rs / fever_pull.rs 的 reconcile_reader_state/reconcile_fever_state 相应改为返回 AppResult<()>，循环内用 ? 传播或聚合成 report.errors，reconcile 调用点据此记录错误（失败不再被当成「0 行变化」静默跳过）。(D) src-tauri/src/sync/fever.rs 模块头/方法注释如实记录 max_id 历史回溯能力（协议支持 vs 客户端未实现），供适配器能力报告消费（本卡只需注释与文档，不要求实现历史回溯）。(E) 政策格锁定测试与方向测试保持；新增/更新 Rust 测试覆盖 apply 返回 Err 的传播路径；矩阵与代码常量一致性由现有测试 + 文档对照保证。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：docs/sync-compat-matrix.md, src-tauri/src/sync

## 验收标准

- ① 矩阵三列化：docs/sync-compat-matrix.md 的协议×操作差异矩阵含「协议/服务端支持」「本客户端已实现」「指定服务端版本已验证」三列，逐格标注；Fever 行明确「协议支持 items&max_id 向更旧翻页；本客户端未实现历史回溯」
- ② 冲突政策标注为客户端选择：GR read-wins 等格在「协议/服务端支持」列不写成协议必然要求，而标注为本客户端冲突策略
- ③ 版本已验证列如实：说明当前仅 mock 替身（及确有实测者）覆盖，Miniflux/FreshRSS 具体版本未逐一验收
- ④ conflict_policy apply_read_by_policy/apply_star_by_policy 返回 AppResult<usize>，DB 写失败不再被 unwrap_or(0) 吞成 0
- ⑤ 调用方 reconcile_reader_state/reconcile_fever_state 返回 AppResult<()> 并在调用点记录失败（report.errors 或传播），不再静默
- ⑥ fever.rs 注释如实记录 max_id 能力（协议支持 vs 客户端未实现）
- ⑦ Rust 测试覆盖 apply 失败传播路径（CI 执行）；现有政策格锁定测试保持绿
- ⑧ 门禁全绿：cargo_fmt + lint/build/frontend 不回退（前端零改动）；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑨ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@eba4df5（第五阶段 P1/P2 任务 117/118/119/121/122/123/124 全部验收后）：frontend 759/759、CI 全绿。本卡为文档矩阵分层 + 冲突应用层错误可见（审计 P2-8），Rust 改动限定 conflict_policy 的错误传播。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：apply_read_by_policy/apply_star_by_policy 返回 Err 的传播路径 Rust 测试（CI 执行）+ 政策格锁定保持；把静默 unwrap_or(0) 改为向上传播，需断言失败不再被当成 0 行变化（审计 P2-8 ③）；Rust 测试由 CI rust job 执行（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build
- 保留：既有政策格锁定测试（conflict_policy.rs / greader_pull.rs / fever_pull.rs 的 #[cfg(test)]）与既有 frontend/cargo fmt/lint/build 门禁；政策方向未变（方向锁定与判别力保持）；本卡前端零改动，矩阵文档无自动化门禁（由独立审查人工核对）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-08T00:47:30.734985Z
- 原截止时间：2026-10-08T04:47:30.734985Z
- 当前截止时间：2026-10-08T04:47:30.734985Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 21 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-10-08T00:47:31.660924Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-08T01:08:57.514981Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-08T01:10:20.262301Z：任务已取消：范围定义疏漏：allowed_paths 漏 src-tauri/src/fever.rs（objective 的 D 项明确点名该文件记录 Fever max_id 能力），diff 判越界；且 finish 在文件被还原后记录了空候选（记录事故）。编码成果已存 tmp/task125.patch，重建任务后应用。；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-125.json)

- [RUN-7898f81cf64c41b6a62feac346049abf](../runs/RUN-7898f81cf64c41b6a62feac346049abf.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
