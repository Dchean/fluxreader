<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-100 · 发布前自检遗留项收口：前端 P2×2+P3×26、Rust P3×7（自检三报告 20260928 的未修复项）

**状态**：done

**目标**：收口三路发布前自检（.workflow-kit/docs/selfcheck-20260928/，Rust 13 项/前端 15 项/UI 28 项）中 v0.15.0 未修复的遗留项：前端 P2×2、前端 P3×9、UI P2×2、UI P3×17、Rust P3×7，另含 base.css 历史注释乱码重建（73 行/114 处 U+FFFD，v0.14.0 起已存在）。逐项修法按报告条目执行（报告即规格），实现时可按现场核实调整但每项须有结论：前端——nav.ts reload 在途拦截（P3-1 瞬态错排）、fetchFailed 假 affordance 文案（P3-2）、triggerManualSync 失败可见（P3-3）、toggleAllFolders 失败聚合（P3-4）、ShortcutsTab 补 Space（P3-6）、viewEntriesCache 上限（P3-7）、doDisconnect 误报（P3-8）、SyncReport.removed_feeds 类型补齐并消费（P3-9）、mock addFeed 假成功（P3-10）；J/K 口径按报告建议取轻修（ShortcutsTab 范围改准 + Social/Notif 卡补 role/tabIndex，跨布局 J/K 属产品决策不做并记录）（U-P2-2）；浮层焦点最小实现（ModalOverlay/Lightbox 打开聚焦容器、关闭归还触发元素）（U-P2-7）；UI P3 全部 17 项（Esc 链补 CloseAskDialog、叫法/生成中/By/快捷键格式统一、#e67e22 token 化、星标视觉统一、截断 title、tokens 注释、theme-color、favicon 占位、LAYOUT_NO_AI 单源、播放器图标 SVG 化、侧栏双刷新 busy 统一、占位内联样式入类、stale 注释）；对比度边缘项只记录不改动（--text-tertiary 全局变更需单独决策）；api.ts FTS5 注释与 README 同步措辞修正（R-P3-8，文件在前端侧）。Rust——persist_new_feed/import_feeds 包事务（P3-1）、folder_name Err 上抛（P3-2）、cleanup_cache ai 分支 datetime() 同口径（P3-3）、list_articles limit 负数收敛（P3-4，负→0，不得影响前端 100000 合法大值）、entries.rs 三处 let _ = → warn（P3-5）、config_sync 三处 let _ = → ? 整包回滚（P3-6）、v15 迁移把 SQLite 格式 published_at 归一为 RFC3339 UTC（P3-7，GLOB 定位幂等，配套前端 parseTs 空格格式按 UTC 解析的兼容，成对测试锁定排序与解析）、流式翻译 delta 消毒先评估（R-P3-9：逐 delta 过 ammonia 可能切断跨 delta 标签——若无法在不破坏流式渲染前提下安全实现，允许跳过并写明依据，CSP 兜底已在）。R-P3-10（get_setting 边界）与 R-P3-11（测试覆盖缺口清单）按报告结论记录不改，列 non_goals。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-100-CONSISTENCY.md
**界面检查**：U1.shortcut-hint-format, U2.add-feed-wording, U3.star-visual, U4.player-icons, U5.truncate-title, U6.favicon-fallback, U7.close-ask-esc, U8.theme-color
**修改范围**：src, src-tauri/src, src-tauri/tests, tools/frontend-regression.mjs, index.html, README.md

## 验收标准

- ① 前端 28 项（上列 A/B/C 组）逐项完成且有结论；每项带回归断言或源级断言（沿用 frontend-regression 既有 fix-* 风格），断言总数不低于 445
- ② Rust 7 项逐项完成：事务/上抛/同口径/收敛/吞错清零各有针对性测试（修前红或行为等价锚）；v15 迁移幂等且有 up 测试 + 排序等价断言（SQLite 格式行与 RFC3339 行在 ORDER BY published_at 下按真实时刻排序）+ 前端 parseTs 兼容
- ③ base.css 114 处 U+FFFD 清零（git 历史可考的恢复原文，不可考的重写通顺中文注释；只动注释文本不动规则）；全仓 grep U+FFFD（src、src-tauri、index.html、README）为 0
- ④ UI 取证：tmp/audit-r3/harness（Chrome CDP + 忠实假后端）核对 8 条 ui_checks，浅色/深色各一套截图 + 交互报告（DOM 断言：图标节点为 svg、title 属性存在、提示文案采集）
- ⑤ 门禁全绿且不回退：cargo test ≥231 passed / 0 failed / 9 ignored 不增（新增用例除外）、fmt/clippy 0；lint/build exit 0；frontend ≥445 且全部通过
- ⑥ 独立审查（未参与实现的全新子代理）PASS findings=0 后由主控验收提交

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；v0.15.0 已发布且门禁全绿（cargo 231/0/9、frontend 445/445、lint/build/fmt/clippy exit 0）；本卡修复的是三路自检报告中定性为 P3 的遗留缺陷与一致性项，均为低风险行为修正或视觉/文案统一，基线本身无失败。
- 基线证据：.workflow-kit/docs/selfcheck-20260928/selfcheck-rust.md
- 需求决定：DEC-task100-selfcheck-remaining-20260929
- 补充：tools/frontend-regression.mjs：28 项前端修复的成对/源级断言（fix 系列延续）；新行为与统一口径需要防回退断言；验证：frontend
- 补充：Rust：v15 迁移 up/幂等/排序等价测试、persist_new_feed 事务原子性、folder_name 上抛、limit 收敛、cleanup_cache 口径；行为变化需要成对证据；验证：cargo_test
- 保留：既有 cargo/frontend 断言、fmt/clippy/lint/build；不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-28T22:38:28.988067Z
- 原截止时间：2026-09-29T02:38:28.988067Z
- 当前截止时间：2026-09-29T02:38:28.988067Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 94 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T23:22:06.281365Z：2026-09-28T23:1xZ 实现重派发：上轮两代理 SendMessage 唤醒失败（实例随会话终止，No active local_agent task），改为全新实例后台并行——前端 agent_bfc8797d（30 项，任务书 tmp/task-100/brief-frontend.md）、Rust agent_00523aee（7+1 项，任务书 tmp/task-100/brief-rust.md）。任务书含 9 键 worker-result 契约与文件域互斥（src/** vs src-tauri/**）；现场已核对无半成品；下一步：等两代理交付 → 主控合并 worker-result → diff --run → finish → verify → 全新独立审查子代理（含 U1-U8 真机取证）→ accept → 提交推送 → bump 0.16.0 单独推 → 等 CI 绿 → annotated tag → Release 监控
- 2026-09-29T00:12:28.652638Z：Worker requests manager action; inspect the result；下一步：处理执行者提出的请求，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-29T00:14:49.063774Z：阻塞已处置（action_required）：finish 首次因 unresolved_items 非空记 action_required 阻塞，属契约误用非现场问题，修正后重跑；下一步：begin 重新实现
- 2026-09-29T00:16:21.160336Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-29T00:17:08.016339Z：编码结果已记录，差异范围已核对：README.md, index.html, src-tauri/src/commands/folders.rs, src-tauri/src/commands/opml.rs, src-tauri/src/config_sync.rs, src-tauri/src/db/articles.rs, src-tauri/src/db/folders.rs, src-tauri/src/db/migrations.rs, src-tauri/src/sync/entries.rs, src-tauri/src/sync/subscriptions.rs, src/App.tsx, src/components/ContextMenu.tsx, src/components/Overlays.tsx, src/components/PlayerBar.tsx, src/components/Reader.tsx, src/components/Sidebar.tsx, src/components/Timeline.tsx, src/components/icons.tsx, src/components/primitives.tsx, src/components/settings/AppearanceTab.tsx, src/components/settings/FeedsTab.tsx, src/components/settings/ShortcutsTab.tsx, src/components/settings/SyncTab.tsx, src/lib/api.ts, src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/feeds.ts, src/store/slices/sync.ts, src/styles/base.css, src/styles/tokens.css, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-29T00:18:07.953490Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-29T00:21:36.632505Z：2026-09-28T23:5xZ-00:0xZ：RUN-8a60ab6d 承接 finish 通过（candidate_digest=15bd751d），verify RUN-c9ad8ebf 全绿。全新独立审查子代理 agent_ff2cf8d0 已派发（38 项逐项核对 + U1-U8 真机取证 tmp/audit-r3/harness + base.css 规则级独立 diff + U+FFFD 复查），报告将写 tmp/task-100/review-report.json，UI 证据落 tasks/evidence/TASK-100-ui-*（ui_review 契约要求证据必须在 tasks/evidence/ 下）；下一步：审查子代理交付 → 主控 review --mode independent --file 注册（context=审查代理 id）→ PASS 则 accept → 提交推送 → v0.16.0 发布序列
- 2026-09-29T00:51:25.078734Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-100.json)

- [RUN-d5c798cf62a14a9d95f699ae1a1cd6d1](../runs/RUN-d5c798cf62a14a9d95f699ae1a1cd6d1.json)
- [RUN-8a60ab6d4df046418dba82dd70b510b7](../runs/RUN-8a60ab6d4df046418dba82dd70b510b7.json)
- [RUN-c9ad8ebf914647dcac48fa9823a4b93e](../runs/RUN-c9ad8ebf914647dcac48fa9823a4b93e.json)
- [RUN-2a5445567208452884ebd0024e5f0827](../runs/RUN-2a5445567208452884ebd0024e5f0827.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
