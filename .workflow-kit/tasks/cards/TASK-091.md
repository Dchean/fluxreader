<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-091 · 失效封面纠正通道：db 清空函数 + tauri 命令 + 回归用例（REQ-106 之④，后端）

**状态**：done

**目标**：修用户报告「文章封面部分获取不到」的后端成因之二——失效封面无纠正通道（审计 round-4 §1-D；基线证据 .workflow-kit/tasks/evidence/TASK-091-baseline-broken-cover.md）。现状：articles_without_cover 只选 image_url IS NULL/''，写入是 COALESCE(NULLIF(image_url,''), ?1) 只填空（scheduler.rs），全仓无「图片加载失败」上报入口，四类图片位也没有 onError ⇒ 源站 404／防盗链失败／文件被删的封面会永久破图（审计实测 ldstatic 30 个真实 URL 中 1 个 404；真实库该域名 1431 篇、约 36 篇永久破图）。本卡只做后端：① 新增 db 函数 clear_article_cover_if_matches(conn, article_id, url)——仅当该行 image_url 与上报 URL 完全一致**且该行 source='direct'** 时清空（返回受影响行数）；非 direct 源不清（否则从「破图」退化为「无图且永不补全」，因为补全队列只收 direct）；② 新增 tauri 命令 report_broken_cover(article_id, url) 返回 bool（是否发生清空），注册进 lib.rs 的 invoke_handler；③ 清空后的行重新进入补全队列，下一次 cover_backfill_round 能写入新封面（有测试）。前端 onError 接线与四类图片位统一代理由后续卡承载。

**依赖**：TASK-090
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/db.rs, src-tauri/src/db/articles.rs, src-tauri/src/commands/articles.rs, src-tauri/src/lib.rs, src-tauri/tests/cover_backfill_e2e.rs

## 验收标准

- ① db 函数语义精确：URL 完全一致且 source='direct' 才清空（返回 1），URL 不一致/本已为空/行不存在返回 0 且数据不变；只写 image_url 一列，不影响其他行与其他列
- ② 非 direct 源（miniflux）不被清空：报告后 image_url 保持原值、返回 false（不得把「破图」变成「无图且永不补全」）
- ③ 清空后可由补全流程填回：清空 → cover_backfill_round 写入新封面（有测试，且断言写入后队列为空）
- ④ 命令层：report_broken_cover 返回布尔，参数为 article_id + url，命令注册进 invoke_handler（编译期即可验证注册缺失会导致构建失败）
- ⑤ 门禁全绿且不回退：cargo test ≥211 passed / 0 failed / 9 ignored 不增；cargo fmt --check exit 0；cargo clippy --all-targets -- -D warnings 干净；lint 0/0；build exit 0；frontend ≥348/348

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；修前门禁全绿（cargo 211/0/9、frontend 348/348、lint 0/0、build exit 0，TASK-090 验证运行 RUN-96824801cdc0469290553580062d3e5a），但既有测试对「失效封面」零覆盖：没有任何入口能清空已写入的封面，也没有用例断言非 direct 源不被清空。已知限制（本卡不改）：前端仍无 onError 上报；miniflux 源仍不补全。
- 基线证据：.workflow-kit/tasks/evidence/TASK-091-baseline-broken-cover.md
- 需求决定：DEC-next-batch-covers-reachability-20260923
- 保留：src-tauri 既有 211 项测试（含 TASK-090 新增的 3 项封面补全用例、sync/dedup 等）；本卡为增量后端能力，不改变既有断言语义；验证：cargo_test
- 补充：src-tauri/tests/cover_backfill_e2e.rs 新增：URL 一致才清空（返回 1）/ URL 不一致或本已为空返回 0 且数据不变 / miniflux 源报告后不被清空 / 清空后补全轮次能写入新封面并清空队列；新能力需要成对断言；纠正通道的「不清 miniflux」是本卡关键边界（避免从破图退化为永久无图）；验证：cargo_test
- 保留：前端回归网 348 项、lint、build；本卡不改前端，作为不回退证据；验证：frontend, lint, build

## 执行与恢复

- 首次开始：2026-09-23T10:27:32.648518Z
- 原截止时间：2026-09-23T14:27:32.648518Z
- 当前截止时间：2026-09-23T14:27:32.648518Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 20 分钟
- 已用修复轮：2
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-23T11:00:10.040271Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-23T11:06:36.002951Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/articles.rs, src-tauri/src/db.rs, src-tauri/src/db/articles.rs, src-tauri/src/lib.rs, src-tauri/tests/cover_backfill_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-23T11:07:22.120046Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-23T11:15:57.687699Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-23T11:16:43.998406Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-23T11:20:07.720729Z：编码结果已记录，差异范围已核对：src-tauri/tests/cover_backfill_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-23T11:21:19.100902Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-23T11:34:50.794625Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-091.json)

- [RUN-509342e304474a3c84b808e59725ec94](../runs/RUN-509342e304474a3c84b808e59725ec94.json)
- [RUN-f5933128d51943658d34da92ee238757](../runs/RUN-f5933128d51943658d34da92ee238757.json)
- [RUN-8ed5c806dee44f2a8cd34637df951a35](../runs/RUN-8ed5c806dee44f2a8cd34637df951a35.json)
- [RUN-2e5aaca128a0419ebdd40e77994ca29f](../runs/RUN-2e5aaca128a0419ebdd40e77994ca29f.json)
- [RUN-3859d9c7502d4e299e4329bc2e5d2ff3](../runs/RUN-3859d9c7502d4e299e4329bc2e5d2ff3.json)
- [RUN-e15c9865ad984bb8badaf5e9d6e8c315](../runs/RUN-e15c9865ad984bb8badaf5e9d6e8c315.json)
- [RUN-adc900a25aad49d984dec5a9189cd807](../runs/RUN-adc900a25aad49d984dec5a9189cd807.json)
- [RUN-9a989731d73b43b99fff9dab8fa6e633](../runs/RUN-9a989731d73b43b99fff9dab8fa6e633.json)
- [RUN-a0be033b1b054b46a02b83ad19fd4e6e](../runs/RUN-a0be033b1b054b46a02b83ad19fd4e6e.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
