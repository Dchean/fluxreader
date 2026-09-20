<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-072 · 降级可见性与 AI 输入校验：全文提取 degraded 标志（P2-10 后半）+ 摘要空正文与 preset 显式提示（P2-11）（REQ-104）

**状态**：ready

**目标**：按 owner 2026-09-20 两项裁决修复降级可见性与输入校验。A【P2-10 后半，DEC-req104-p2-10b-fulltext-degraded-20260920】现状：commands/settings.rs 的 extract_fulltext 在无法提取或防退化原样返回时静默回落原文，用户看不出发生了降级。实施：① 提取失败/防退化时给出结构化 degraded 标志（而非只改文案），前端据此显示准确文案；② 保持成功路径行为与文案不变；③ 补失败路径断言（提取失败、防退化返回原文两种形态）。B【P2-11，DEC-req104-p2-11-ai-validation-20260920】现状：commands/ai.rs 的 ai_summarize 无空正文校验（translate 已有，不对称）；未知 preset 静默回退 deepseek-chat。实施：④ ai_summarize 补空正文校验，与 translate 对称（给出可理解错误而不是把空文本送模型）；⑤ 未知 preset 不再静默回退，改为显式提示/报错；⑥ 补断言覆盖空正文与未知 preset 两条路径。

**依赖**：TASK-069
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/settings.rs, src-tauri/src/commands/ai.rs, src-tauri/src/ai.rs, src/store.ts, src/store/slices/reader.ts, src/lib/api.ts, src/components/Reader.tsx, src-tauri/tests/ai_e2e.rs, tools/frontend-regression.mjs

## 验收标准

- A① 提取失败与防退化两条路径都有结构化 degraded 标志（不再静默回落），成功路径行为与文案不变
- A② 失败路径断言覆盖『提取失败』与『防退化返回原文』两种形态，修前复现/修后通过成对留证
- B① ai_summarize 空正文被拦下并给出可理解错误（与 translate 对称），有断言覆盖
- B② 未知 preset 显式提示/报错，不再静默回退 deepseek-chat，有断言覆盖
- ③ 四门禁全绿：cargo test 通过数 ≥177 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ④ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-20 基线（TASK-069 终版候选验证 RUN-5380e8e4，提交 452e0e2）：cargo test 177 passed / 0 failed / 9 ignored、lint 0/0、build exit 0、frontend 303/303。本任务 behavior=change：① 全文提取由『静默回落原文』改为『结构化 degraded 标志 + 准确文案』——审计记录的现状缺陷，owner 裁决修复；② ai_summarize 由『空正文照送模型』改为『先校验并报错』，与 translate 既有行为对齐；③ 未知 preset 由『静默回退 deepseek-chat』改为『显式提示』。既有断言中与旧行为绑定的部分随新行为改造，其余保持；既有 (P2-10) 前端断言口径须同步核对，不得弱化仍有效的保护。
- 基线证据：.workflow-kit/tasks/runs/RUN-5380e8e45603487db6019495fdb61fb7.json
- 需求决定：DEC-req104-p2-10b-fulltext-degraded-20260920, DEC-req104-p2-11-ai-validation-20260920
- 替换：全文提取『失败仍按成功回落原文并报成功』相关断言：改为断言 degraded 标志与非成功文案；owner 裁决 DEC-req104-p2-10b-fulltext-degraded-20260920 要求降级可见，与旧断言冲突；验证：cargo_test, frontend
- 补充：ai_summarize 空正文校验、未知 preset 显式提示两条路径的断言；本次新行为需要覆盖，且两条都是此前无保护的路径；验证：cargo_test
- 保留：其余全部既有断言与测试（177 条 Rust + 303 条前端）逐字不动，含既有 (P2-10) 防退化断言与 AI 流式断言；除上述契约变更外行为保持，既有防退化保护继续有效；验证：cargo_test, lint, build, frontend

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

[唯一状态记录](../items/TASK-072.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
