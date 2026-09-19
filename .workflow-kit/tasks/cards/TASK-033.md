<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-033 · 生产错误回退 mock 修复（P0-2）+ 两处 [object Object] 错误文案（P1-10/P1-11）

**状态**：done

**目标**：三项前端错误路径修复：(1) P0-2——Tauri 环境下 reloadFromBackend/bootstrap 失败时当前回退渲染 mock 演示数据（假订阅/假文章进真实 UI）；改为错误状态 + 重试入口，mock 数据严格限定浏览器 dev 模式。(2) P1-10——store.ts GitHub 登录 WebDAV 冲突检测用 String(e) 恒得 [object Object]，改用 extractError 并按 code==='webdavConflict' 判定，恢复切换确认框与真实错误文案。(3) P1-11——SettingsModal AiTab 连通失败 toast 直接插值 ${e}，改用 extractError。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/**, tools/**

## 验收标准

- tauri 模式下 bootstrap/reload 失败不再渲染 mock 数据，出现错误提示与重试入口（新增回归断言）
- mock 数据仅在非 tauri（浏览器 dev）模式使用
- WebDAV 冲突场景弹确认框且错误文案真实（新增回归断言）
- AI 连通失败 toast 显示真实错误信息；前端回归与 lint 全绿

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-15 基线：lint 0/0；前端回归 13/13（含 TASK-030 新增 S-2）。行为变更仅限三处错误路径：生产不再回退 mock（改为错误态+重试）、WebDAV 冲突检测恢复工作、AI toast 显示真实错误——均为用户确认的缺陷修复（FINDINGS-REQ-007.md P0-2/P1-10/P1-11，REQ-007 范围）。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-15.md
- 需求决定：DEC-defect-batch2-20260915
- 保留：现有 13 项前端逻辑回归；既有行为不回归；验证：frontend
- 保留：oxlint 0 警告门禁；静态检查不放松；验证：lint
- 补充：tauri 模式 bootstrap 失败不回退 mock 的回归断言；P0-2 此前无覆盖；验证：frontend
- 补充：WebDAV 冲突确认路径回归断言；P1-10 此前无覆盖；验证：frontend

## 执行与恢复

- 首次开始：2026-09-15T11:07:13.814991Z
- 原截止时间：2026-09-15T12:37:13.814991Z
- 当前截止时间：2026-09-15T12:37:13.814991Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 11 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-15T11:07:13.867445Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-15T11:18:35.977915Z：编码结果已记录，差异范围已核对；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-15T11:18:37.722778Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T11:24:36.456380Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-15T11:31:02.211625Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-033.json)

- [RUN-bf0d0fad4cc74b9a93f0af5a97a60ff9](../runs/RUN-bf0d0fad4cc74b9a93f0af5a97a60ff9.json)
- [RUN-bfda845a1b994f2ba6969cd54dec8603](../runs/RUN-bfda845a1b994f2ba6969cd54dec8603.json)
- [RUN-a7bc575a7e224638b8f3478eaa9b2580](../runs/RUN-a7bc575a7e224638b8f3478eaa9b2580.json)
- [RUN-f5628531f4354991bdec569d15da11d3](../runs/RUN-f5628531f4354991bdec569d15da11d3.json)
- [RUN-da433ec6d7b04dd49b5b27b471868262](../runs/RUN-da433ec6d7b04dd49b5b27b471868262.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
