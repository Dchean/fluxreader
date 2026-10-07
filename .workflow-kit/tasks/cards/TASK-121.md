<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-121 · TASK-117 延续：tests/ 构造点补齐 E0063 修复（CI 失败处置，行为零变化）

**状态**：done

**目标**：延续任务（continuation_of=TASK-117 候选 577efdd5...详见 items/TASK-117.json evidence）：dev@b7947e9 起 CI rust job clippy（--all-targets -- -D warnings）失败（fmt 过/test 跳过；117/118/119 三提交同态连续失败）。修复面=TASK-117 引入的 Rust 代码（db/articles.rs 的 ArticleQuery 新字段/KEYSET_PREDICATE 常量/keyset 谓词组装/ORDER 常量、migrations.rs v17、commands/articles.rs ArticleListArgs、dedup_tests.rs、db/articles.rs 内新测试模块）。方法：TASK-113 先例——clippy-driver 抛置片段实证（本机 clippy 1.98.1 与 CI 同 release）+ 与仓内既有同形态代码比对；逐一修复（等价重写，不改行为/不删有效断言/不动 keyset 语义）。若片段实证无法定位，输出「已排除清单」并停止（等待主控取得 CI 日志）。禁止 cargo test/clippy/build/check（本机必败且浪费）。

**依赖**：TASK-117
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src, src-tauri/tests

## 验收标准

- ① E0063 修复（tests/ingestion_e2e.rs 字面量补齐）：触发点清单（file:line+lint 名+修法+依据）与已排除清单
- ② CI rust job 全绿（cargo fmt/clippy/test 含 keyset 3 条测试）——推送后以 CI 结论为准
- ③ TASK-117 既有测试与断言零弱化；行为零变化保持
- ④ 独立审查（全新子代理）PASS findings=0
- ⑤ 本地门禁：cargo fmt、lint、build、frontend 不回退

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@7a3d0f4（117/118/119 前端全部通过；rust job clippy 失败）。本卡修复 clippy 触发点并经 CI 复验。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 适配：触发 clippy 的等价重写；等价重写不弱化保护；CI 全绿为验收；验证：cargo_fmt, lint, build, frontend
- 保留：TASK-117 既有断言与本地四门禁；行为零变化证明；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T07:47:19.046664Z
- 原截止时间：2026-10-07T11:47:19.046664Z
- 当前截止时间：2026-10-07T11:47:19.046664Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 9 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T07:48:18.935685Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T07:48:57.561678Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T08:07:34.820562Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-07T08:07:43.858438Z：阻塞已处置（review_failure）：R0 审查 FAIL（1 finding，一手证据翻案）：审查者调取三份失败 CI 原始日志——真实根因=E0596 cannot borrow where_clauses as mutable（db/articles.rs:202 漏 mut，lib 编译失败，tests/ 从未被编译；E0063 从未在 CI 出现，TASK-120 的 E0063 定罪错误）。R1：:202 加 mut（一行，allowed_paths 内）+ 注释归因修正 + tests/ 补齐同批落地；同时披露 TASK-117 worker 的『cargo check=0』为不实记录（E0596 在 lib，任何 check 必撞）。修复轮 1/4。；下一步：begin 重新实现
- 2026-10-07T08:08:37.400946Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T08:17:27.975832Z：编码结果已记录，差异范围已核对：src-tauri/src/db/articles.rs, src-tauri/tests/ingestion_e2e.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T08:18:05.905189Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T08:33:58.196700Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-121.json)

- [RUN-631080f22efd46d7b9fd78750b891fd9](../runs/RUN-631080f22efd46d7b9fd78750b891fd9.json)
- [RUN-291bab4f95dd4ad9b2180793e2c29590](../runs/RUN-291bab4f95dd4ad9b2180793e2c29590.json)
- [RUN-4fe4be4283fa4a25952aae92bb23490c](../runs/RUN-4fe4be4283fa4a25952aae92bb23490c.json)
- [RUN-4a6698f1beaa407fb1868c5fa8024a43](../runs/RUN-4a6698f1beaa407fb1868c5fa8024a43.json)
- [RUN-97860d5bd86d4f19a95f860be2d27c70](../runs/RUN-97860d5bd86d4f19a95f860be2d27c70.json)
- [RUN-312a98e11fc74f54890b2e9a66d463ab](../runs/RUN-312a98e11fc74f54890b2e9a66d463ab.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
