<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-117 · 可变集合分页改 keyset 游标——阅读中连续翻页不丢文章（审计 P1-1）

**状态**：done

**目标**：修复第三方审计 P1-1（tmp/audit-20261007/REVIEW.md 第 1 节，探针 probes.mjs P1）：可变筛选集合（WHERE is_read=0）上 OFFSET 分页不等价于已看条数——1200 未读读 500 标读后集合剩 700，下一页仍 OFFSET 500 → 跳过 500 篇并假 exhausted。审计要求：以稳定排序键做 keyset 分页（规范化发布时间+唯一 ID），列表、定位与续拉共用排序规则；验收必须包含阅读/取消收藏过程中连续翻页，不能只有静态分页与插入去重。修法要求：①Rust：list_articles 增可选 keyset 参数（last_published TEXT + last_id i64），DESC 谓词 `(a.published_at < ?last_published OR (a.published_at = ?last_published AND a.id < ?last_id))`（ASC 反向）；ORDER 常量补 id 决胜（`a.published_at DESC, a.id DESC`/ASC 同理）——**article_index 的 ROW_NUMBER 窗口必须同步补 id 决胜**（锚定位置与列表顺序同口径，锚定不受本卡破坏）；OFFSET 参数保留（其他调用方兼容）但前端列表路径停用。②前端：articlesCursor 从「已加载条数」改为「每 scopeKey 的 {lastPublished, lastId, loaded}」，loadMoreArticles 传 keyset 参数；exhausted 判定不变（返回 < PAGE_SIZE）；selectVisibleEntries 本地排序必须补 id 决胜（与服务器顺序一致，续拉追加才不乱序）。③「今天/未读/收藏」三种筛选与「全部」全部走 keyset。④回归断言 t117-* ≥6 条，**必须包含操作序列场景**（DEC-gate-adjust 纪律）：阅读中连续翻页（审计探针场景：1200 未读、读 500、续拉不得跳过）、取消收藏场景、插入漂移（keyset 天然免疫，断言锁定）、排序决胜并列 published_at（同秒文章不重不漏）。既有断言依赖 offset 语义的按行为变化更新附理由。审计探针可参考 tmp/audit-20261007/probes.mjs 的 P1 场景构造。⑤Rust 测试 ≥2 条（keyset 谓词正确性：DESC/ASC 边界、并列 published_at 决胜；CI 执行）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src, tools/frontend-regression.mjs, src-tauri/src

## 验收标准

- ① 阅读中连续翻页不丢：1200 未读场景，读 500 后续拉返回剩余集合前 500 篇（审计探针场景转回归，t117-*）
- ② 取消收藏场景同性质通过；并列 published_at 由 id 决胜不重不漏
- ③ article_index 锚定与列表顺序保持同口径（既有锚定断言不回退）
- ④ Rust keyset 谓词测试 ≥2 条（DESC/ASC/并列决胜）；cargo test/clippy 由 CI 承担（DEC-local-cargo-gate-20261005）
- ⑤ 门禁全绿：frontend（t117-* 新增）、lint/build/cargo_fmt 不回退
- ⑥ 独立审查（全新子代理）PASS findings=0；keyset 谓词的 SQL 注入面（last_published 为外部字符串）需审查确认参数化

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@02a82f1（门禁调整后）：frontend 655/655、CI rust job 全绿（v0.17.0 tag）。本卡修复审计 P1-1 正确性缺陷（offset 分页在可变集合跳过条目），属行为修复；审计探针场景必须转回归（DEC-gate-adjust 纪律）。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：t117-* 操作序列回归（阅读中翻页/取消收藏/插入漂移/并列决胜）+ Rust keyset 谓词测试；审计 P1-1 正确性修复需按操作序列场景锁定（DEC-gate-adjust ①）；验证：cargo_fmt, frontend
- 适配：依赖 offset 游标语义的既有断言（游标镜像/applyArticlesCursor 契约）；offset 语义正是本卡废除的缺陷手法，更新附理由；验证：frontend
- 保留：其余既有 frontend/cargo fmt/lint/build 断言；不回退证据；cargo 由 CI 承担（DEC-local-cargo-gate-20261005）；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-07T02:10:39.215028Z
- 原截止时间：2026-10-07T06:10:39.215028Z
- 当前截止时间：2026-10-07T06:10:39.215028Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 83 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-07T02:10:39.955043Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-07T03:34:13.529795Z：编码结果已记录，差异范围已核对：src-tauri/src/commands/articles.rs, src-tauri/src/db/articles.rs, src-tauri/src/db/dedup_tests.rs, src-tauri/src/db/migrations.rs, src/lib/api.ts, src/store/internals.ts, src/store/selectors.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, src/store/types.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-07T03:34:59.903113Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-07T04:11:24.216205Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-117.json)

- [RUN-e1cb59a9c6174f8baf1ee25fb446acfc](../runs/RUN-e1cb59a9c6174f8baf1ee25fb446acfc.json)
- [RUN-ac201789cc76419e87ee62468a7fedf8](../runs/RUN-ac201789cc76419e87ee62468a7fedf8.json)
- [RUN-f72b939388e44a01a06521a93d56e1e3](../runs/RUN-f72b939388e44a01a06521a93d56e1e3.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
