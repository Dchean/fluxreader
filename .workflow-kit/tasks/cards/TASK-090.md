<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-090 · 封面补全饥饿修复：负缓存语义 + 候选窗口推进（REQ-106 之①）

**状态**：done

**目标**：修复用户报告「文章封面部分获取不到」的后端成因之一——封面补全饥饿（审计 round-4 §1-B；基线证据 .workflow-kit/tasks/evidence/TASK-089-baseline-cover.md 含本次只读实测：真实库 52 条 direct 候选中最新 20 条全部属于审计实测 403 的 kirikira.moe，而第 27-46 位可用候选从未被请求）。现状（src-tauri/src/scheduler.rs:313-327 / :374-379 与 src-tauri/src/db/articles.rs:674-687）：每轮只取 published_at DESC 的前 20 条候选，且在**发请求之前**就把 URL 写入进程内 tried 集合；前 20 条一旦全部失败，targets 变空后循环继续 sleep 并重新取同一批最新 20 条 ⇒ 第 21 条及以后在本进程内永不被尝试。修法：① 负缓存只在**一次尝试结束后**记录（成功 / 无 og:image / 网络失败都算已尝试）；② 候选窗口**推进**——每轮按分页扫描候选（跳过已尝试项），使后续批次的老候选在同一进程内也能被尝试，且单轮扫描页数有上限以约束开销；③ 单轮内按 URL 去重，保证同一 URL 每轮最多一次请求。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/scheduler.rs, src-tauri/src/db/articles.rs, src-tauri/tests/cover_backfill_e2e.rs

## 验收标准

- ① 有成对证据：新增 Rust 测试（本地假 HTTP 服务，不依赖外网）证明「同一进程内，最新 K 条候选全部失败后，第 K+1 条起的候选仍会被请求并补上封面」——修前失败、修后通过
- ② 负缓存语义：URL 只在一次尝试**结束后**进入负缓存（不再出现「请求还没发就被标记为已尝试」）；单轮内同一 URL 最多请求一次
- ③ 窗口推进有界：候选为空时不再空转重复请求；单轮扫描页数有上限且常量带注释说明取舍
- ④ 既有补全语义不回退：仍只处理 source='direct'；封面写入仍是「只填空」（不覆盖已有封面）；成功补到的条目不再留在队列里
- ⑤ 四门禁全绿且不回退：cargo test ≥208 passed / 0 failed / 9 ignored 不增；cargo fmt --check exit 0；lint 0/0；build exit 0；frontend ≥348/348

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；修前四门禁全绿（cargo 208 passed / 0 failed / 9 ignored、frontend 348/348、lint 0/0、build exit 0，见 JOURNAL 2026-09-23T07:13:57Z），但**既有测试对补全循环零覆盖**：没有任何用例驱动窗口推进或负缓存语义，故缺陷在全绿下存活。已知限制（本卡不改）：miniflux 源仍不进入补全队列；失效（404）封面仍无纠正通道，属后续卡。
- 基线证据：.workflow-kit/tasks/evidence/TASK-089-baseline-cover.md
- 需求决定：DEC-next-batch-covers-reachability-20260923
- 保留：src-tauri 既有 208 项测试（含 sync_content_e2e 的封面回填、dedup、sync_gap 等）；缺陷修复不改变既有断言语义；既有套件是回归底线；验证：cargo_test
- 补充：新增 src-tauri/tests/cover_backfill_e2e.rs：饥饿复现（最新 K 条候选全失败后第 K+1 条起必须被请求并补上封面）+ 负缓存语义（尝试后才记、单轮 URL 去重）；修复需要修前失败/修后通过的成对证据，且该行为当前零覆盖；验证：cargo_test
- 保留：前端回归网 348 项、lint、build（本次不改前端代码）；证明改动没有造成前端与构建回归；验证：frontend, lint, build

## 执行与恢复

- 首次开始：2026-09-23T08:29:42.345186Z
- 原截止时间：2026-09-23T12:29:42.345186Z
- 当前截止时间：2026-09-23T12:29:42.345186Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 38 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-23T08:49:59.240111Z：编码结果已记录，差异范围已核对：src-tauri/src/db/articles.rs, src-tauri/src/scheduler.rs, src-tauri/tests/cover_backfill_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-23T08:52:32.723923Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-23T09:21:35.259954Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-23T09:22:29.059246Z：阻塞已处置（review_failure）：独立审查 PASS 但列出 5 条 finding，工具要求 find非空即需修复。已核对：可执行的是 #1（去重断言平凡成立——同 feed 同 URL 会被 upsert 的 by_url 兜底折叠成一行，断言未真正覆盖 picked_urls 去重）与 #4（COALESCE 无法覆盖 image_url='' 的行，导致空串行永远留在队列且被记入 filled 计数）；#2 为等价变异、#3 为验收③明确允许的有界取舍、#5 为归属留痕，均不需要改代码，将登记为后续待办。修复范围仍在原 allowed_paths 内（tests/cover_backfill_e2e.rs 与 src-tauri/src/scheduler.rs），预算充足（repair 0/4 轮，截止 2026-09-23T12:29Z）。；下一步：begin 重新实现
- 2026-09-23T09:22:55.940936Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-23T09:41:18.862730Z：编码结果已记录，差异范围已核对：src-tauri/src/db/articles.rs, src-tauri/src/scheduler.rs, src-tauri/tests/cover_backfill_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-23T09:41:58.152795Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-23T10:08:32.704801Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-090.json)

- [RUN-b407ba6d56b1437d95dc270a59835c72](../runs/RUN-b407ba6d56b1437d95dc270a59835c72.json)
- [RUN-4360bb8d33874b5c92876ab13cb19053](../runs/RUN-4360bb8d33874b5c92876ab13cb19053.json)
- [RUN-4d7a532a82f1435d92dcc643b3881a76](../runs/RUN-4d7a532a82f1435d92dcc643b3881a76.json)
- [RUN-58df9eb3fe274c3b979eb6d7fcea36b4](../runs/RUN-58df9eb3fe274c3b979eb6d7fcea36b4.json)
- [RUN-96824801cdc0469290553580062d3e5a](../runs/RUN-96824801cdc0469290553580062d3e5a.json)
- [RUN-0a165e9b0ec34a3eba583fd38853631a](../runs/RUN-0a165e9b0ec34a3eba583fd38853631a.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
