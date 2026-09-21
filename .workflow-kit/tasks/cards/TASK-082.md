<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-082 · 修正 db/articles.rs 的失效分节横幅 + 复核 REQ-105 的 800 行口径

**状态**：ready

**目标**：① 修正 src-tauri/src/db/articles.rs 里三处**失效分节横幅**：`Folders`（行 53）之下实际是 NewArticle/article_row/article_list_item/ArticleQuery/list_articles/search_articles 等**文章域**条目，没有任何 folder 函数；`URL 规范化`（行 473）之下实际是 upsert_article_with_feed/insert_new_article/set_read/set_starred/mark_all_read/feed_counts 等写入与状态条目，没有任何 URL 规范化函数；`Settings`（行 748）是**空横幅**（其后紧接 #[cfg(test)]，内容为零）。这三处是 TASK-023 从 db.rs 拆出 db/ 子模块时的遗留脚手架，会主动误导后续读者，替换为与真实内容一致的分节说明。② 复核 REQ-105 验收①「拆分后无超过 800 行的生产单体」在当前代码库的口径：实测 db/articles.rs 共 998 行 = 生产段 751 行 + 同文件单测 247 行（TASK-081 新增的 P3 单测），**生产代码本身未超限**；评估并记录该文件是否需要按领域拆分（结论与理由写入 AUDIT 报告），供 owner 核对，而不是默认追求行数达标。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/db, .workflow-kit/docs/AUDIT-20260919-v2.md

## 验收标准

- ① db/articles.rs 中不再存在与实际内容不符或为空的分节横幅：`Folders`/`URL 规范化`/`Settings` 三处被替换为准确描述（或删除空横幅），且替换后每处横幅下方确实含有它宣称的条目（以脚本/人工比对名称清单为准）
- ② 行为零变化：四门禁全绿——cargo test 通过数 ≥202 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 313/313 不回退
- ③ 既有断言一行不动（含 TASK-081 新增的 P3 单测），不新增也不删除任何 #[test]
- ④ REQ-105 验收①的复核结论写入 AUDIT-20260919-v2.md：给出实测行数拆分（生产段 vs 测试段）、明确该文件是否需要领域拆分及理由；不得含糊其辞或只写「已达标」

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-081 终版候选验证 RUN-d8ee5c8a，提交 d46eff2）：cargo test 202 passed / 0 failed / 9 ignored、npm run lint 0 warnings / 0 errors、npm run build exit 0、npm run test:frontend 313/313。本任务 behavior=preserve：只改注释横幅与文档，无可观察行为变化，故既有全部断言（202 条 Rust + 313 条前端）必须逐字保留并原样通过；不新增断言。
- 基线证据：.workflow-kit/tasks/runs/RUN-d8ee5c8ac2374b308f60c97e9f80a446.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 202 条 Rust 断言与 313 条前端断言逐字保留并原样通过；纯注释订正与文档补录，无行为改动；既有断言即「零行为变化」的判据。TASK-081 新增的 5 类 P3 单测（purge_remote_data 空目录保留/清空后删除、cleanup_cache 时区归一、search_articles LIMIT、dressed_up 去重）保持原位、一行不动。；验证：cargo_test, frontend
- 保留：lint 与 build 门禁的既有基线（0 warnings / exit 0）；本任务只动注释横幅，不得引入任何新 lint 警告（TASK-081 修复轮曾因在组件文件 export 非组件函数把 lint 从 0 变 1，此处复核同类风险）；build 必须保持 exit 0。；验证：lint, build

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

[唯一状态记录](../items/TASK-082.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
