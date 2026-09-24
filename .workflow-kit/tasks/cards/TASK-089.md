<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-089 · 封面补全饥饿与死链纠正（补全窗口推进 + 失效封面纠正通道，REQ-106）

**状态**：cancelled

**目标**：修复用户报告「文章封面部分获取不到」的后端两个成因（审计 round-4 §1-B/§1-D，已在真实库与真机量化，基线见 .workflow-kit/tasks/evidence/TASK-089-baseline-cover.md）。① 补全饥饿：src-tauri/src/scheduler.rs 每轮只取 published_at DESC 的前 20 条候选（db/articles.rs:674-687 的 articles_without_cover LIMIT 20），且在**发请求之前**就把 URL 写入进程内 tried 集合（scheduler.rs:374-379）；最新 20 条若全部失败（本次只读探针实测：该 20 条全部属于审计实测 403 的 kirikira.moe），第 21 条及以后在本进程内永不被尝试——真实库 52 条候选中 32 条从未被请求。修法：负缓存只在一次尝试**结束后**记录，并让候选窗口**推进**（已尝试的 URL 不再占住窗口顶部），使后续老候选在同一进程内也能被尝试。② 死链无纠正：articles_without_cover 只选 image_url IS NULL/''，写入是 COALESCE 只填空（scheduler.rs:403-410），且前端无上报通道 ⇒ 一次 404 永久破图（实测 ldstatic 30 个真实 URL 中 1 个 404，真实库约 36 篇）。修法：新增后端纠正通道（db 函数 + tauri 命令），仅当库内 image_url 与上报 URL 完全一致时清空该行 image_url（幂等、只动该行该列），使该行重新进入补全队列并可写入新封面。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/scheduler.rs, src-tauri/src/db/articles.rs, src-tauri/src/commands/articles.rs, src-tauri/src/lib.rs, src-tauri/tests/cover_backfill_e2e.rs

## 验收标准

- ① 饥饿修复有成对证据：新增 Rust 测试（本地假 HTTP 服务，不依赖外网）证明「同一进程内，前 K 条候选全部失败后，第 K+1 条起的候选仍会被请求并补上封面」——修前失败、修后通过
- ② 负缓存语义明确：URL 只在一次尝试结束后进入负缓存；窗口推进不以「丢失失败记录」为代价（失败候选在窗口推进/冷却后可再次尝试，且不会每轮重复轰炸同一批）
- ③ 死链纠正通道：仅当库内 image_url 与上报 URL 完全一致时清空（返回是否变更）；不匹配或本已为空时幂等返回未变更；不改其他列与其他行；清空后补全流程能重新填充（有测试）
- ④ 既有补全语义不回退：非 direct 源仍不进入补全队列（本轮不改该产品口径）；封面写入仍是「只填空」，不覆盖正常封面
- ⑤ 四门禁全绿且不回退：cargo test ≥208 passed / 0 failed / 9 ignored 不增；cargo fmt --check exit 0；lint 0/0；build exit 0；frontend ≥348/348

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；既有四门禁在修前全绿：cargo 208 passed / 0 failed / 9 ignored、frontend 348/348、lint 0/0、build exit 0（本次复核实测，见 JOURNAL 2026-09-23T07:13:57Z 与 TASK-089-baseline-cover.md）。**既有测试完全未覆盖本缺陷**：没有任何用例驱动补全循环的窗口推进，也没有任何用例覆盖「已失效封面」的纠正通路；因此缺陷在 208 项全绿下存活。已知限制：本卡只处理 direct 源口径；miniflux 源无封面仍不补全（属产品口径，owner 未裁决）。
- 基线证据：.workflow-kit/tasks/evidence/TASK-089-baseline-cover.md
- 需求决定：DEC-next-batch-covers-reachability-20260923
- 保留：src-tauri 既有 208 项测试（含 sync_content_e2e 的封面回填用例、dedup、sync_gap 等）；本次为缺陷修复，不改变既有断言语义；既有套件是回归底线；验证：cargo_test
- 补充：新增 src-tauri/tests/cover_backfill_e2e.rs：饥饿复现（同一进程内前 K 条候选全失败后，第 K+1 条必须被请求并补上封面）+ 死链纠正通道（URL 一致才清空、幂等、清空后可再填充、URL 不匹配不动）；缺陷修复需要修前失败/修后通过的成对证据，且现有测试对补全循环与纠正通道零覆盖；验证：cargo_test
- 保留：前端回归网 348 项、lint、build（本次不改前端代码）；用于证明改动没有造成前端与构建回归；验证：frontend, lint, build

## 执行与恢复

- 首次开始：2026-09-23T08:24:04.262635Z
- 原截止时间：2026-09-23T12:24:04.262635Z
- 当前截止时间：2026-09-23T12:24:04.262635Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 3 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 最近检查点

- 2026-09-23T08:24:04.636263Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-23T08:27:56.057648Z：allowed_paths 缺少 src-tauri/src/db.rs（新 db 函数的再导出点），需重新冻结范围；下一步：处理执行者提出的请求，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-23T08:28:21.522821Z：任务已取消：prepare 冻结的 allowed_paths 与实现所需范围不一致（新增 db 层函数必须登记到 src-tauri/src/db.rs 的再导出列表，而该文件不在本卡范围内）；本卡未产生任何代码改动，按工作流「改范围=重新冻结」拆分为后续卡承载同一目标；下一步：如需同一目标，准备新的任务并引用本任务作为历史

## 原始证据

[唯一状态记录](../items/TASK-089.json)

- [RUN-3f8a4862e4044f8499ab912333c046ea](../runs/RUN-3f8a4862e4044f8499ab912333c046ea.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
