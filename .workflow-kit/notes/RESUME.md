<!-- project-workflow: generated view; edit task JSON instead -->
# 接手与恢复笔记

任何 Agent 接手前先读本文件，再运行 `python .workflow-kit/scripts/project_workflow.py resume --root .`。本文件由任务记录、检查点和日志生成；事实以 JSON 记录和原始证据为准。

## 当前状态

**项目进度 · fluxreader**

目标：以 workflow-kit 2026-09-18.3 新周期对 fluxreader 持续改进：修复全项目体检发现的缺陷（P1/P2），处置功能空壳与死代码确保无未落实的宣称能力，完成 ingestion.rs 拆分与结构性硬化（pull 游标守卫、app_settings 收口），使项目稳定规范、代码与技术路线优雅高效

当前阶段：**分步实施**

阶段目标：落实已确认的完整范围，逐步交付并保持已验收行为：社交布局下正文经常一直加载，切换一下布局又能秒加载；双向同步未真正做到：订阅操作与文章状态变更未回传同步后端；本地抓取模式下，本地文章数量与状态和同步后端不一致

| 阶段 | 目标 | 状态 |
| --- | --- | --- |
| 需求与目标 | 明确目标、已有 Bug、新功能、其他要求、质量目标和执行边界 | 已完成 |
| 分析与方案 | 记录参考、原始基线状态与限制，说明维护、稳定和性能取舍，并确认路线 | 已完成 |
| 界面预览 | 验证关键流程、整体设计和控件完整状态，确认后沿用前端实现 | 不适用 |
| 分步实施 | 落实已确认的完整范围，逐步交付并保持已验收行为：社交布局下正文经常一直加载，切换一下布局又能秒加载；双向同步未真正做到：订阅操作与文章状态变更未回传同步后端；本地抓取模式下，本地文章数量与状态和同步后端不一致 | 当前 |
| 回归与审查 | 以需求、失败路径、适用界面检查、维护性和性能证据核对当前组合候选 | 分批推进 |
| 验收与交付 | 核对完整范围，交付可运行成果、使用说明及适用的恢复办法 | 待推进 |

**完整验收目标**：体检报告 AUDIT-20260919-v2.md 的 P1/P2 缺陷经确认后全部修复并有修前复现/修后验证的成对证据；结构性硬化落地：pull 分块失败不推进增量游标；app_settings 读取收口为类型化助手；ingestion.rs 拆分为领域模块，行为零变化，四门禁不回归；死代码/空壳集群处置完毕（删除或裁决保留），无未落实的宣称能力；设计边界项（P2-12/P3-11 等）经 owner 逐项裁决：实施或注释明示保留；既有质量底线延续：cargo 161/0/9、lint 0/0、build exit 0、frontend 283/283 不回退（通过数可增不可减）

**质量目标**：维护性—单体模块拆分为领域模块（延续 db.rs 试点模式），前端补行为测试，lint/typecheck/test 作为门禁；稳定性—现有 Rust e2e 与前端回归不回归；同步、抓取、播放主流程稳定

**性能安排**：社交布局正文加载不再无限等待，与切换布局后的秒开对齐

已建任务 35 项：已验收 34，待验收 0，阻塞 0。

| 任务 | 状态 | 目标 / 下一步 |
| --- | --- | --- |
| [TASK-029 · 全局排查空壳功能与隐藏 Bug，产出可确认清单（REQ-007）](<../tasks/cards/TASK-029.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-030 · 修复社交布局正文无限加载（REQ-001）](<../tasks/cards/TASK-030.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-031 · 定位双向同步缺口：订阅与文章状态回传（REQ-002/003）](<../tasks/cards/TASK-031.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-032 · 同步状态链路修复：对账防误判（C-1）+ 离线变更一律入队（A-5）](<../tasks/cards/TASK-032.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-033 · 生产错误回退 mock 修复（P0-2）+ 两处 [object Object] 错误文案（P1-10/P1-11）](<../tasks/cards/TASK-033.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-034 · 社交/通知卡片翻译按钮接线（P1-7）](<../tasks/cards/TASK-034.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-035 · 删除订阅接线：远端退订 + 删除墓碑防复活（A-1）](<../tasks/cards/TASK-035.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-036 · 订阅改名/移动目录接线：edit_subscription 推送远端（A-2）](<../tasks/cards/TASK-036.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-037 · 同步接线收尾：push 挂分类（A-3）+ 分类改名/删除防复活（A-4）](<../tasks/cards/TASK-037.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-038 · 同步队列卫生：老化清理（A-8）+ 吞错日志（C-2）](<../tasks/cards/TASK-038.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-039 · REQ-004 播客页 toast 位置 + REQ-008 设置页控件一致性](<../tasks/cards/TASK-039.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-040 · 前端缺陷批一：按 id 摘要态（F4）+ 搜索打开标读（F7）+ 全部已读视图口径（F8）+ 搜索竞态（F20）](<../tasks/cards/TASK-040.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |

另有 23 项记录可在任务总览查看。

**已确认但尚未拆分的需求**：体检发现的 P2 缺陷逐项修复（N3~N11）：手动刷新全部忽略 smartDedup（scheduler.rs:104）；已删订阅重新添加墓碑不清→pull 永久跳过+队列老化物理删除（folders.rs:141-186/opml.rs:52）；「今天」视图时区错位（db/articles.rs:195,692）；config_sync_apply 无事务中途失败留半套（config_sync.rs:141-261）；anchorToArticle 不复位译文/全文/原始渲染标志（bootstrap.ts:264-275）；卡片译文按纯文本渲染 HTML 契约不一致（Timeline.tsx:452-455,739-742）；列宽拖拽逐像素落库（App.tsx:256-271）；前端 async action 无 catch 静默脱节（bootstrap/nav/reader/feeds/设置页多处）；流式翻译未消毒 HTML 进 DOM 且回读失败残留（Reader.tsx:294-298、slices/ai.ts:70-79,156-167）；结构性硬化：pull 分块失败时不推进增量游标（greader_pull.rs:77-90,116-119 与 fever_pull 对称）；app_settings JSON blob 逐字段解析收口为类型化助手（lib.rs:38-96、commands/mod.rs:18、scheduler.rs:62,201-222 共 6+ 处）；lib/api.ts 行类型契约加 serde fixture 往返断言防漂移；仓库根目录垃圾文件清理与 .gitignore 补全；死代码/空壳集群处置与 P3 卫生项：删除零生产调用代码（db/feeds.rs:245,257,269 Miniflux 兜底三查询、ingestion.rs:330-407 旧版 refresh_feed、greader.rs:501-518 未用方法、db/sync_map.rs 被取代的逐条查询、sync/mod.rs:29 fallback_entries 恒 0 与模块注释修订、lib/api.ts:503 api.syncNow、AiEvent::Error 删或接通）；P3 项逐条处置（吞错 warn 化、sync_save 留空只复用 password、purge_remote_data 范围、cleanup_cache 时区、LIMIT 绑定、normalize 去重一致化、gist 孤儿、init unwrap 加固、快捷键浮层让路、播放中同集切换、批量标读合并等）；设计边界项 P2-12（配置同步删除语义）/P3-11（远端退订本地删除）与 P2-10 后半/P2-11 修复立项前逐项请 owner 裁决；ingestion.rs（766 行，最后一个未拆旧单体）拆分为 ingestion/ 领域模块（conditional_get/parse_feed/map_entry/staged 刷新/favicon 发现），沿用先补断言→纯搬运→四门禁配方，crate::ingestion 路径不变，行为零变化

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

## 未完成的上下文、决策与待办（Agent 笔记）

- 2026-09-19T07:45:48.458674Z · note/decision · TASK-060 · owner 确认 TASK-060 报告 §4 的可达性结论成立：upsert_remote_entry 的 pending 守卫分支（entries.rs:224）为死代码，其保护场景由 merge_remote_status（entries.rs:76）承担；owner 选择「确认死代码，立项删除」——将立项新任务删除该死分支并走完整测试与审查流程（来源：验收问答 2026-09-19，同场验收 TASK-060，DEC-0126898ab7284c73881f86220cfb5827）
- 2026-09-19T09:25:32.113666Z · note/decision · REQ-007 清单遗留项归档为已知限制（owner 2026-09-19 选择记录项目级验收，遗留项随项目归档、后续想做再另立任务）：需产品决策的设计边界 P1-6（本地直连新文章不回写远端，协议限制）/P2-12（配置同步无删除语义）/P3-11（远端退订本地永不删，疑有意保守设计）；P2 残留 P2-9（Miniflux 兜底路径未实现）/P2-10 后半/P2-11/P1-7 残留（卡片挂载自动生成译文 effect）；P3 卫生 P3-2（sync_now 两份实现漂移）/P3-4/P3-5（吞错点）/P3-6/P3-7/P3-8（错误处理边角）/P3-9（init 期 panic 风险点三处）/P3-10（AiEvent::Error 从不发送）；证据缺口 P2-2/P2-5/P2-6（代码已修，测试 harness 无 DOM 无渲染级证据）。来源：TASK-051 报告逐项处置清单（DEC-fix-all-findings-20260917 框架）
- 2026-09-19T09:40:09.510616Z · note/decision · 新周期 legacy_review（owner 2026-09-19 指令：以更新后的 workflow-kit 从零启动新一轮分析）。已读来源：.workflow-kit/tasks/ 全部记录（PROJECT.json stage=complete、DECISIONS 79 条、33 任务、194 运行）、PROJECT_STATE.md、JOURNAL.md、docs/FINDINGS-REQ-007.md、docs/FINDINGS-IGNORED-TESTS.md。保留约束：BRIEF.compatibility 确认继续有效（SQLite 与迁移、Fever/GReader/本地抓取协议行为、现有 UI 布局与交互习惯）；回归底线=161 条 Rust 测试 + 283 条前端断言 + lint/build 门禁不弱化。旧任务处置：33 任务全部验收（TASK-046 按流程取消），stage=complete；归档的约 15 项已知限制（见 2026-09-19 note）仅作为本轮排查线索重新验证，不自动纳入实施。流程冲突结论：旧授权（含 DEC-fix-all-findings「把发现的问题都修复」）仅限旧周期，不自动延续；新周期运行边界经用户确认沿用（DEC 本日新周期启动条目），重构路线待体检证据呈现后由 owner 重新选择。角色结论：当前会话是总控；Worker 身份只来自明确 task_id/run_id 执行包

## 教训

- 2026-09-19T08:27:39.212281Z · note/lesson · TASK-061 · 审查登记规则：本项目工具要求 verdict=PASS 且 findings 为空才接受审查报告；即使审查者自己标注「非阻塞、不要求返工」的备注，只要列进 findings 就会触发 review_failure。处置方式是如实落地备注所指的小修（本轮：注释措辞从「完全一致」改为精确子集论证表述并补维护警示），同一独立审查上下文复核后 findings 清空重报，而不是让审查者撤回发现

## 最近事件

- 2026-09-19T13:53:52.470325Z · prepare · TASK-062 · 任务已冻结：修复 OPML 导入每条根级订阅重复新建「导入」文件夹（REQ-101/N1）；范围 src-tauri/src/commands/opml.rs
- 2026-09-19T13:53:56.408691Z · checkpoint · TASK-062 · 开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T14:05:00.062895Z · checkpoint · TASK-062 · 编码结果已记录，差异范围已核对：src-tauri/src/commands/opml.rs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T14:05:20.654022Z · checkpoint · TASK-062 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T14:16:21.928772Z · checkpoint · TASK-062 · 当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-09-19T14:21:48.945612Z · accept · 验收 TASK-062；依据：用户选择：验收 TASK-062，继续 N2（验收问答 2026-09-19）
- 2026-09-19T14:32:09.572300Z · prepare · TASK-063 · 任务已冻结：接线 selectFeed 范围切换拉取：缓存命中同步恢复 + 未命中重拉，修复跨范围污染与重复卡片（REQ-101/N2）；范围 src/store/slices/nav.ts, tools/frontend-regression.mjs
- 2026-09-19T14:32:13.050314Z · checkpoint · TASK-063 · 开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-19T14:48:58.576662Z · checkpoint · TASK-063 · 编码结果已记录，差异范围已核对：src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-19T14:49:18.751695Z · checkpoint · TASK-063 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T15:05:13.355906Z · checkpoint · TASK-063 · 当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-09-19T15:11:56.007300Z · accept · 验收 TASK-063；依据：用户选择：验收 TASK-063，继续下一批（验收问答 2026-09-19）

## 如何继续

1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。
2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。
3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。
4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](../tasks/PROJECT_STATE.md)。
