<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-079 · REQ-105：ingestion.rs（680 行，最后一个未拆旧单体）拆分为 ingestion/ 领域模块

**状态**：verified

**目标**：按 REQ-105 把 src-tauri/src/ingestion.rs（680 行）拆分为 ingestion/ 领域模块，沿用项目既有拆分配方（TASK-044 拆 commands、TASK-045 拆 sync、TASK-023 拆 db）：**先补/确认断言 → 纯搬运 → 四门禁**。拆分目标（按文件内既有的注释分节天然对应）：① conditional_get（HTTP 条件 GET + build_client + Fetched + read_capped）；② parse_feed（feed-rs 解析 + ParsedFeed + resolve_url + clamp_publish_date + map_entry + mime_from_url）；③ staged 三段式刷新（read_feed_for_refresh + fetch_and_parse + apply_refresh_result + refresh_feed_staged）；④ favicon 发现（discover_favicon + extract_icon_link + rel_is_icon + extract_html_attr + FAVICON_TRIED + BROWSER_UA）；⑤ 常量（USER_AGENT / MAX_BODY_BYTES / NO_STABLE_ID）。硬约束：**crate::ingestion::<item> 公开路径必须逐字不变**——全库有 47 处调用点（产品 commands/folders.rs、commands/articles.rs、scheduler.rs、lib.rs；测试 ingestion_e2e / refresh_dedup_e2e / staged_refresh_e2e / scheduler_e2e / sync_* 等），拆分必须用 pub use 重导出保持路径，或适配全部调用点（二者择一，优先前者以最小化改动面）。行为必须零变化：不得改变任何逻辑、SQL、阈值、超时、并发结构（含 tokio::spawn 与 FAVICON_TRIED 负缓存的语义）与函数签名。验收标准另要求「拆分后无超过 800 行的生产单体」——拆分后 ingestion/ 各模块须均远低于该阈值（现状最大为 db/articles.rs 的 791 行，拆完 ingestion 后需复核该结论仍成立）。

**依赖**：TASK-078
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/src/ingestion.rs, src-tauri/src/ingestion, src-tauri/src/lib.rs, src-tauri/src/commands/folders.rs, src-tauri/src/commands/articles.rs, src-tauri/src/scheduler.rs, src-tauri/tests/ingestion_e2e.rs, src-tauri/tests/refresh_dedup_e2e.rs, src-tauri/tests/staged_refresh_e2e.rs, src-tauri/tests/scheduler_e2e.rs

## 验收标准

- ① 拆分后 crate::ingestion::<item> 的全部既有公开路径仍可用（47 处调用点零语义改动；若机械适配调用点则须逐一列出）
- ② 拆分后无超过 800 行的生产单体（复核 ingestion/ 各模块与全库最大文件）
- ③ 行为零变化：四门禁全绿——cargo test 通过数 193 不减且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend 303/303
- ④ 既有断言一行不动（纯搬运的判据）；公开路径不变或有完整调用点适配清单
- ⑤ 不引入新依赖；package.json/Cargo.toml/Cargo.lock 零改动；文本文件 LF；用户真实数据库不得写入

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-078 终版候选验证 RUN-6aeb3b77，提交 a88d0d1）：cargo test 193 passed / 0 failed / 9 ignored、npm run lint exit 0（0 warnings/0 errors）、npm run build exit 0、npm run test:frontend exit 0 且 303/303。本任务是 behavior=preserve 的纯结构搬运（REQ-105 明确要求『行为零变化、既有断言一行不动』），故全部既有断言按其原样保留；拆分只在模块边界上物理移动代码并用 pub use 保持 crate::ingestion 路径，不改变任何可观察行为。ingestion 相关的既有测验面（ingestion_e2e 的条件 GET/解析/消毒、refresh_dedup_e2e 的去重、staged_refresh_e2e 的三段式与失败写回、scheduler_e2e 的调度抓取）即为本次搬运的回归保护，全部原样运行。
- 基线证据：.workflow-kit/tasks/runs/RUN-6aeb3b77f8984b48ad8c1905007b6c5f.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部 193 条 Rust + 303 条前端断言逐字不动；ingestion_e2e / refresh_dedup_e2e / staged_refresh_e2e / scheduler_e2e 等 ingestion 相关套件作为搬运的回归保护原样保留；REQ-105 验收标准明确『行为零变化：四门禁全绿，既有断言一行不动』，纯搬运不允许改断言；验证：cargo_test, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-21T07:18:29.953573Z
- 原截止时间：2026-09-21T11:18:29.953573Z
- 当前截止时间：2026-09-21T11:18:29.953573Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 8 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-21T07:18:30.338858Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-21T07:26:43.114318Z：编码结果已记录，差异范围已核对：src-tauri/src/ingestion.rs, src-tauri/src/ingestion/favicon.rs, src-tauri/src/ingestion/http.rs, src-tauri/src/ingestion/mod.rs, src-tauri/src/ingestion/parse.rs, src-tauri/src/ingestion/staged.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-21T07:27:13.457784Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-21T07:33:08.448127Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-079.json)

- [RUN-577d7bd16b294693a37cef6db4ed516c](../runs/RUN-577d7bd16b294693a37cef6db4ed516c.json)
- [RUN-88d6970ed646409595b4d8694f579944](../runs/RUN-88d6970ed646409595b4d8694f579944.json)
- [RUN-0db7ba075d914bf4afe1695b5e5d746a](../runs/RUN-0db7ba075d914bf4afe1695b5e5d746a.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
