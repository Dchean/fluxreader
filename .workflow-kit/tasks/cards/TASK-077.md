<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-077 · 降级可见性与 AI 输入校验收口：全文提取 degraded 标志（P2-10 后半）+ 摘要空正文与 preset 显式提示（P2-11）（REQ-104）

**状态**：verified

**目标**：本任务收口被取消的 TASK-076 的同一成果（历史：TASK-076 已完成全部实现与取证，四门禁实测通过、新增 9 条单测、两项行为变更各有变异取证；但其 allowed_paths 初稿含 4 条不存在路径、订正落盘于 begin 之后，任务基线为订正前快照，diff 恒把任务记录判为 protected，recover/unblock/重新 begin 均无法收敛，故按 TASK-068→TASK-069 / TASK-074→TASK-075 先例取消并以本卡收口；本任务相对 begin 快照为零新增改动，成果经候选快照绑定）。原目标如下。A【P2-10 后半：全文提取 degraded 标志，DEC-req104-p2-10b-fulltext-degraded-20260920】现状：commands/settings.rs 的 extract_fulltext 有「智能防退化」逻辑（提取结果剥标签后不足原文 80% 时保留原文），但降级是静默的——返回裸 String，与「提取成功」无法区分，前端只能靠「返回内容 == 当前正文」的字符串比对去猜。实施：改为结构化 degraded 标志（含原因）+ 准确文案；须补失败路径断言覆盖两种形态（提取失败/空结果、防退化返回原文），并保持成功路径行为不变。B【P2-11：AI 输入校验与配置可观测性，DEC-req104-p2-11-ai-validation-20260920】① ai_summarize 缺少 ai_translate 已有的空正文校验，空正文会白跑一次模型调用；② 未知 preset 静默回落 deepseek-chat / https://api.deepseek.com，用户选的预设不存在时请求被打到其未选择的厂商上。实施：补空正文校验（与 translate 对称）；未知 preset 改为显式提示/报错；须补断言覆盖两条路径。

**依赖**：TASK-075
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/commands/settings.rs, src-tauri/src/commands/ai.rs, src-tauri/src/ai.rs, src-tauri/src/extraction.rs, src-tauri/src/commands/articles.rs, src/lib/api.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs

## 验收标准

- A① 全文提取的降级不再静默：调用方能从返回值/事件中明确区分「提取成功」与「保留原文（降级）」，且降级原因可辨
- A② 失败路径断言覆盖两种形态：提取失败（报错/空结果）与防退化返回原文，两者都有断言；成功路径行为与提取标志语义不变
- B① ai_summarize 补空正文校验，与 ai_translate 对称（同样的错误类型与提示语风格），并有断言
- B② 未知 preset 不再静默回落 deepseek-chat / api.deepseek.com，改为显式提示或报错，并有断言证明不会静默换厂商
- ① 四门禁全绿：cargo test 通过数 ≥184 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 通过数 ≥303
- ② 两项行为变更各有成对证据（修前可复现/修后通过）
- ③ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入
- ④ 台账改动（任务记录/决定）须在 begin 之前完成，避免相对基线越界（TASK-074 教训）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-20 基线（TASK-075 终版候选验证 RUN-4386e97b，提交 c3f9db6）：cargo test 184 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务 behavior=change 且属业务契约变更，依据 owner 2026-09-20 两项裁决：① 全文提取由「静默保留原文」改为「结构化 degraded 标志 + 准确文案」；② AI 侧由「空正文照跑、未知 preset 静默回落」改为「空正文显式报错、未知 preset 显式提示」。两者都改变了用户可见的错误面与文案，属用户已确认的新行为与旧行为冲突，故按 replace 语义处理：替换断言须引用对应 owner 决定。既有无全文提取测试（extract_fulltext 在 tests/ 下无覆盖）；AI 侧既有测试须核对后保持。
- 基线证据：.workflow-kit/tasks/runs/RUN-4386e97bc9044e44b27bc91863c4c3a8.json
- 需求决定：DEC-req104-p2-10b-fulltext-degraded-20260920, DEC-req104-p2-11-ai-validation-20260920
- 替换：全文提取降级时「静默 return Ok(original)」的隐式契约：改为显式 degraded 结果（含原因），并同步前端读取方式；owner 裁决 DEC-req104-p2-10b-fulltext-degraded-20260920 要求降级可见，与静默回落的旧行为冲突；验证：cargo_test
- 替换：未知 preset 静默回落 deepseek-chat / api.deepseek.com 的旧行为：改为显式提示或报错；owner 裁决 DEC-req104-p2-11-ai-validation-20260920 要求显式提示，与静默回落的旧行为冲突；验证：cargo_test
- 补充：失败路径覆盖：全文提取失败（报错/空结果）与防退化返回原文两种形态各一条断言；ai_summarize 空正文断言；未知 preset 断言（证明不会静默换厂商）；本次改动核心就是异常路径的可见性，必须有覆盖；验证：cargo_test
- 保留：全文提取成功路径与提取标志语义、防退化阈值、ai_translate 既有校验、AI 流式协议与 PRESETS 清单，以及其余全部既有断言（184 条 Rust + 303 条前端）逐字不动；除上述契约变更外，其余行为保持；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-21T01:29:19.674330Z
- 原截止时间：2026-09-21T05:29:19.674330Z
- 当前截止时间：2026-09-21T05:29:19.674330Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 0 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-21T01:29:20.043135Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-21T01:29:47.950304Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T01:30:22.860820Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T01:39:45.372870Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-077.json)

- [RUN-5b3e3d8985fb4090a538c5b11616c4c3](../runs/RUN-5b3e3d8985fb4090a538c5b11616c4c3.json)
- [RUN-f5c07ebc890b4519a2f79d843e8d2bcd](../runs/RUN-f5c07ebc890b4519a2f79d843e8d2bcd.json)
- [RUN-fc765484a7fb4c72b48dbccc584bead1](../runs/RUN-fc765484a7fb4c72b48dbccc584bead1.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
