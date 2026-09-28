<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-093 · Batch 1/2 独立审查 P3 收口：竞态守卫含排序、回滚恢复原值、阅读器平行路径、reload 异常与断言补齐（REQ-102）

**状态**：verified

**目标**：收口 Batch 1/2 独立审查（.workflow-kit/tasks/evidence/BATCH12-independent-review-20260924.md）的 5 条 P3：F1 bootstrap.ts:172 loadMore 竞态守卫只比 scopeKey 与 articlesLimit===offset、不含排序方向 ⇒ 旧排序在途页可被接到新排序列表（探针实测 duplicates 100 / missing 100）；F2 reader.ts:396-404 卡片失败回滚是「再翻一次当前值」⇒ 连点两次、首次失败时 UI 与 DB 不一致（探针实测 UI isRead=true / DB is_read=false）；F3 阅读器平行路径未收口：reader.ts:321-333 toggleCurrentReadStatus（及同文件同形的收藏入口）先弹乐观成功 toast、失败不回滚，ai.ts:231-252 toggleReaderTranslation 失败路径不按「无半截未消毒产物即清 rawTranslatedIds」规则处理；F4 frontend-regression.mjs 覆盖缺口：收藏失败回滚、onError 路径清标记、viewEntriesCache.clear、SocialCard 删除假成功 toast 四处变异后全绿，且 1626/2212/2227 三条在修前代码下也通过；F5 nav.ts:159-160 toggleTimelineSort 的 reloadFromBackend 无 .catch（unhandled rejection）、缺 dataMode 判断、筛选视图下重拉多余。修法：F1 给分页请求加代际（发起时记录 sort 或 reload 递增的 generation，返回时不一致丢弃）；F2/F3 卡片与阅读器共用一个「乐观写 + 仅当当前值仍等于乐观值才恢复原值 + 失败 toast」的 helper，成功提示只在成功后出现（与 P1-5 去假成功一致）；翻译失败路径共用 ai.ts 已有的清标记规则；F5 调用点 .catch + dataMode 判断，筛选视图（本地排序的全集）只做本地重排不重拉；F4 为以上每一处补断言，并加强 1626/2212/2227 使其具判别力。；另收口 TASK-092 两轮独立审查的 P3 备忘（tmp/review-092.json P2 外条目与 tmp/review-092-round2.json summary）：G1 coverImage.ts ready 缓存 LRU(300) 驱逐成功 URL 后可重新请求（注释已声明权衡，评估是否可接受或收紧）；G2 readyOrder 驱逐后重成功会重复 push 导致上限提前触顶；G3 Number(articleId) 对非数字 id 静默拒绝（当前全为纯数字串无损，评估 mock 前缀 id 语义）；G4 证据引用名与归档名差一个 ui- 前缀、ui-result.json 键名不统一（流程文档层，非代码）；G5 CoverImage.tsx 行尾 LF→CRLF 规范化与代理解码失败 reason=direct-error 不参与决策（确认无需改动即可关闭）。G1–G3 为代码项，G4–G5 核对后可直接关闭。

**依赖**：TASK-092
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src/**, tools/frontend-regression.mjs

## 验收标准

- ① F1：旧排序在途的 loadMore 晚于新排序重拉到达时被丢弃——有断言复现审查探针场景（修前 duplicates>0，修后 entries 唯一且无缺失）
- ② F2：连点两次且第一次失败时，最终 UI 与后端一致（已读与收藏两条路径各一条断言，修前失败修后通过）
- ③ F3：阅读器标读/收藏失败时回滚且只出失败提示、成功提示在成功后出现；阅读器翻译失败且无半截产物时 rawTranslatedIds 被清、有半截产物时保留（成对断言）
- ④ F5：toggleTimelineSort 不产生 unhandled rejection；非 tauri 模式不调后端；筛选视图不重拉（IPC 计数断言）
- ⑤ F4：审查列出的 M2c/M4b/M6/M7 四个变异在修后回归网下全部变红；1626/2212/2227 三条改为修前代码下会失败；每条新增/加强断言附变异与变红输出
- ⑥ 门禁全绿且不回退：cargo test ≥213 passed / 0 failed / 9 ignored 不增；fmt/clippy/lint/build exit 0；frontend 通过数不少于 TASK-092 验证后的数目且全部通过

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；Batch 1/2 独立审查复跑门禁全绿（cargo 213/0/9、frontend 348/348），但审查以变异证明 M2c/M4b/M6/M7 四处无断言保护、1626/2212/2227 三条无判别力，并以探针复现 F1/F2。
- 基线证据：.workflow-kit/tasks/evidence/BATCH12-independent-review-20260924.md
- 需求决定：DEC-task093-p3-batch-20260928
- 保留：前端回归既有断言（含 Batch 1/2 与 TASK-092 新增）；修复不改变已确认语义；验证：frontend
- 补充：tools/frontend-regression.mjs：F1 排序代际、F2 连点回滚、F3 阅读器回滚/翻译清标记、F5 无 unhandled rejection 与 IPC 计数、M2c/M4b/M6/M7 覆盖；加强 1626/2212/2227；审查指出的覆盖缺口需要成对断言；验证：frontend
- 保留：src-tauri 既有测试、fmt、clippy、lint、build；本卡不改 Rust；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build

## 执行与恢复

- 首次开始：2026-09-28T06:28:35.529898Z
- 原截止时间：2026-09-28T10:28:35.529898Z
- 当前截止时间：2026-09-28T10:28:35.529898Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 81 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T06:28:35.748558Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T06:58:58.545151Z：Interrupted run recovered: 2026-09-28 主控核实：续用的实现子代理因长上下文干扰停止（其指控的环境损坏与磁盘真相不符——git status 一致、无伪造文件、无 worker-result）；实际留有部分连贯工作：F1 排序代际守卫完整（bootstrap.ts）、F2/F3 共用 helper 完整（internals.ts optimisticEntryFlagToggle）、reader.ts 仅 import 接线；按 owner 规则（复用出现干扰即换新）改派全新实例接手；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-28T06:59:09.727273Z：阻塞已处置（interrupted）：中断原因已核实为续用子代理的长上下文读取污染，非环境损坏；工作区部分改动（internals.ts/bootstrap.ts/reader.ts）经主控逐行审阅为合理中间态，保留供新实例接手；无 worker-result、无越界写入。恢复安全。；下一步：begin 重新实现
- 2026-09-28T06:59:19.753718Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T07:50:15.289825Z：编码结果已记录，差异范围已核对：src/lib/coverImage.ts, src/store/internals.ts, src/store/slices/ai.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/slices/reader.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T07:51:14.500505Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T08:03:26.355138Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-093.json)

- [RUN-44185d04397b4707a659182dfab50f4a](../runs/RUN-44185d04397b4707a659182dfab50f4a.json)
- [RUN-51d9865f2f544e01b815eec2ec0aa20c](../runs/RUN-51d9865f2f544e01b815eec2ec0aa20c.json)
- [RUN-193cf241b3774e198b31c83cf2eacdec](../runs/RUN-193cf241b3774e198b31c83cf2eacdec.json)
- [RUN-bb3cea5349834fc7a33650bb0c3ea9f4](../runs/RUN-bb3cea5349834fc7a33650bb0c3ea9f4.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
