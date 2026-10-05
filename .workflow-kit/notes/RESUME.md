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

已建任务 82 项：已验收 64，待验收 1，阻塞 0。

| 任务 | 状态 | 目标 / 下一步 |
| --- | --- | --- |
| [TASK-110 · 筛选视图真分页——废除 limit:100000 近似全集（二阶段②，审计块②/完成标准「旧文章可达」）](<../tasks/cards/TASK-110.md>) | 待执行 | 废除筛选视图（收藏/未读/今天）的唯一近似全集路径 reloadFilteredEntries（bootstrap.ts:266-306）的 limit:100000，改为真分页，使旧文章在筛选视图可达（审计二阶段完成标准）。现况事实（探查在案）：reloadFilteredEntries 是全库唯一超大 limit 调用点（bootstrap.ts:271-279），articlesExhausted 恒 true（:302），切排序只本地重排全集（nav.ts:200-201）；Rust 端 list_articles 本就支持 limit/offset（commands/articles.rs:54-67 limit unwrap_or(500) 无上限；db/articles.rs:182-188 ORDER/LIMIT/OFFSET）；ARTICLES_PAGE_SIZE=500（bootstrap.ts:34）；假后端 queryRows 已支持分页与筛选（tools/frontend-regression.mjs:365-376）。修法要求：①筛选视图分页拉取：首批 PAGE_SIZE，articlesCursor/scopePageKey 口径复用（游标键已含 layout，TASK-094 R7），加载更多在筛选视图可用（复用或扩展 loadMoreArticles，含其三重竞态守卫的等价物；sentinel 三态 timelineSentinel 与自动续拉 timelineRefill 在筛选视图生效）；articlesExhausted 改为真实判定（next.length < PAGE_SIZE）。②稳定序与偏移：追加按 id 去重（同步插入使 offset 漂移时不得重复入列）；新条目插入已加载页中间的处理策略二选一并写明（追加去重保序 vs 触发重拉），选定的策略要有判别断言。③切排序对齐 all：筛选视图切排序改为重拉当前范围（放弃「全集在内存本地重排」的旧语义；已加载条目的水合正文由 merge 保留——TASK-106 机制），toggleTimelineSort（nav.ts:184-203）相应收口。④计数口径不动（feedCounts 权威对账为 TASK-107 已建立语义）；大库下不再一次拉全集，内存占用随页数线性可控。⑤回归断言 t110-* ≥6 条：分页首屏、加载更多、追加去重（偏移漂移场景）、exhausted 判定、切排序重拉（水合保留）、筛选过滤参数（only_unread/only_starred/only_today）逐一如旧。既有断言零弱化——注意既有断言若依赖「筛选视图拉全集」行为（如 articlesExhausted 恒 true、切排序不重拉），需按行为变化更新并附理由。 |
| [TASK-109 · 查询口径统一收口 + merge bump 按真值来源收窄（二阶段①，承接审计块②）](<../tasks/cards/TASK-109.md>) | 已验证，待验收 | 当前候选的测试与审查通过（independent）；继续已授权任务；所属功能完成后请用户验收 |
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

另有 70 项记录可在任务总览查看。

**本轮暂缓**：SQL 性能与索引加固（审计 route-M，主控已用 EXPLAIN QUERY PLAN 独立复核）：M-5 列表查询因 COALESCE 表达式排序使 idx_articles_published 对 8 个变体全失效（SCAN + USE TEMP B-TREE FOR ORDER BY）；M-7 sync_queue 零索引致每次标读全表扫；M-9 一次全部已读 = 1 SELECT + 1 UPDATE + 2N 往返且全程持锁；M-14 v6→v7 后置回填在事务外且以 user_version 当完成标记（半途中断永不重试）。

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

## 未完成的上下文、决策与待办（Agent 笔记）

- 2026-09-24T05:06:31.639391Z · note/todo · 范围外待办（Batch 1/2 独立审查附注）：src-tauri/src/sync/greader_pull.rs:147-149 增量游标 set_last_sync_ts 取拉取结束时的 Utc::now()，拉取过程中服务端变更的条目下一轮增量会漏掉，只能靠全量对账补回。另有 07:57Z 记录的 mock_greader changed_at 语义全面核对待办。二者同属同步增量窗口，建议合并为一张卡评估。
- 2026-09-28T01:27:17.197991Z · note/decision · TASK-092 · 2026-09-28 owner 指示（本会话原话）：「子代理不成功，后续不重复使用子代理，选择新开」。据此修订 09-23/09-24 的「做过类似问题的子代理继续交付给他」：之后实现/修复一律新开子代理，不续用旧实例（包括失败或停滞的）；新子代理通过交接说明与 tmp/ 下留存产出接手。独立审查仍须新开且未参与实现。本次已停止 impl-frontend-images-2（已恢复但未产出），代码与 m0-backup 一致。
- 2026-09-28T06:45:40.075663Z · note/decision · TASK-093 · 2026-09-28 owner 修订子代理使用规则（本会话原话）：「代码编写……如果之前处理过什么问题，后续如果遇到同类型的问题就再让他进行代码编写，这样节省token和理解，同时一直复用造成长上下文干扰，但是审查子代理每次都要开全新的子代理，避免污染」。据此取代 2026-09-28T01:27Z 的「一律新开」规则：实现/修复类子代理在存在同类型问题处理经验时优先续用（节省 token 与理解成本），但不无限复用，出现长上下文干扰即换新开；审查类子代理一律全新实例，未参与实现，防污染。TASK-093 实现续用首轮 TASK-092 独立审查子代理（agent_ffb8c60a，其产出含 G1-G3 备忘与 cov-*/M22 断言结构核对，同类型经验最直接）。
- 2026-09-29T10:48:03.260746Z · note/decision · 2026-09-29 owner 指示（原话）：「签出一个不含现有工作流的纯项目代码分支作为主分支，现分支作为副分支，后续调整先推送副分支，验证后没有问题再合并进主分支（合并也不添加工作流内容）」。执行：①原 main（含 .workflow-kit 2702 文件 + WORKFLOW-KIT.md + 工作流版 AGENTS/CLAUDE/README 段落）重命名为 dev（0928afd），全部历史与工作流记录保留；②新 main 自 afa7edf（v0.16.0 发布提交）另起一笔 b5499d2——移除 .workflow-kit/ 与 WORKFLOW-KIT.md，AGENTS.md/CLAUDE.md 改写为分支流向约定，README 工作流段落改为指向 dev，.gitignore 增 .workflow-kit/ 防误提交；项目代码（src、src-tauri、tools、public、.github、配置、LICENSE）与 afa7edf 逐字一致（git diff 为空）；未重写历史、无 force push，tag v0.16.0 仍指向 afa7edf 且为 main 祖先。③合并协议（同时写入 main 的 AGENTS.md）：在 main 上 git merge --no-commit --no-ff dev && git rm -r -q .workflow-kit && git commit；AGENTS/CLAUDE/README/.gitignore 冲突保留 main 版。④ci.yml 触发分支补 dev：dev 推送即跑 CI，供合并进 main 前验证。⑤后续发布：bump 与合并顺序——dev 上 bump → 合并进 main（不带工作流）→ 在 main 提交上打 annotated tag（tag 门控要求该提交 CI 绿）。
- 2026-09-29T13:32:50.022310Z · note/decision · 2026-09-29 owner 指示：「按照项目最新情况更新readme（如果不影响工作流的话），并且规范历史提交的commit文案和release文案」。执行：①README 两分支统一为同一份分支无关版本（分支与发布约定、提交与发布文案约定、能力概述刷新、结构表校准——删不存在的 workflow-kit/ 启动包行、.workflow-kit/ 标注仅 dev），未来 dev→main 合并在 README 上零冲突；②release.yml 的 releaseBody 改为自动取 annotated tag 附注（手动触发回退静态文案），今后发布说明只维护 tag 附注一处，模板见 .workflow-kit/docs/RELEASE-NOTES-TEMPLATE.md；③提交信息约定（type(scope): 中文主题；chore(workflow) 仅 dev）写入 README 并即日生效，本日提交即按此执行；④历史提交文案重写经评估不执行：改写祖先提交须重打全部 9 个已发布 tag（v0.8.0..v0.16.0）并强推，重推 v* tag 会逐个重触发 Release 工作流（旧提交无 CI 运行记录，45 分钟门控必超时失败），且 .workflow-kit 证据与日志引用的提交哈希全部失效——收益远低于破坏面；规范自即日起生效，历史以 tag 附注呈现；若 owner 仍要求改写历史需明确批准并接受「临时停用 Release 触发 + filter-repo + 全 tag 重建 + 双分支强推」流程；⑤已发布 Release 的 web 正文（旧静态文案）本机无 gh/token 不能改，owner 可自行 gh release edit --notes-file 修正（命令见模板文档）。
- 2026-10-05T06:55:38.348960Z · note/decision · 核心一致性重构路线（DEC-refactor-roadmap-20261005）：保留 Tauri/Rust/React/SQLite 底座不重写；按审计四块（实体与视图分离/统一查询范围/事务化写入/同文建模）分四阶段推进，第一阶段=TASK-103 正文水合生命周期 + TASK-104 全部已读计数一致性 + TASK-105 Rust 状态写入事务化；编码派子代理、独立审查每轮新开子代理；GReader 优先推荐、Fever 为受限兼容；同文副本读状态传播保持现状并显式记录为政策，布局隔离优先与否留 owner 决定；二阶段及以后卡片在第一阶段验证通过后立项。详见 .workflow-kit/docs/DEC-refactor-roadmap-20261005.md 与 AUDIT-20261005-core-consistency.md
- 2026-10-05T08:20:22.925338Z · note/context · 环境修复（DEC-local-cargo-gate-20261005 补充）：本机 .cargo/bin 全部 shim 为 rustup.exe 符号链接，workflow runtime resolve_program 的 Path.resolve() 追链后以 rustup.exe 身份执行导致 cargo test/fmt 报「invalid value ... for [+toolchain]」。已把 cargo.exe 符号链接替换为等价实体副本（rustup shim 按 argv[0] 文件名分发，副本语义不变，rustup 默认 Windows 形态即实体副本）。cargo --version/rustfmt --version 复验通过。TASK-106 的 cargo_fmt 本地门禁恢复可执行。
- 2026-10-05T08:51:40.666508Z · note/context · TASK-106 · CI 权威证据：dev@76f3256（TASK-106 候选）GitHub Actions CI 全绿（frontend job + rust job，windows-latest cargo test + clippy -D warnings），2026-10-05 完成于约 08:52Z。cargo_test/cargo_clippy 本地可选门禁由本次 CI 运行覆盖（DEC-local-cargo-gate-20261005 补偿控制生效）。
- 2026-10-05T11:40:27.990777Z · note/todo · 二阶段微任务候选（R2 审查裁定记录）：mergeSnapshotEntries 增加 fromBackend 参数把 bump 收窄到后端真值路径（bootstrap.ts 三处传 true，nav.ts 三处缓存恢复不 bump）——审查确认修法技术成立（nav 不 bump 不会重开快照踩踏缺口），当前角例双窄窗口+自愈故不阻塞；宜与二阶段「查询缓存/操作版本模型」一并实施。来源：TASK-107 R2 审查（agent_e92dd332）。
- 2026-10-05T12:37:10.453887Z · note/context · TASK-108 · CI 权威证据：dev@8c1501f（TASK-108 候选）GitHub Actions CI 全绿（rust job windows-latest：cargo test 含新增 4 条故障注入测试 + clippy -D warnings；frontend job 亦绿），编译验证缺口由此闭合。独立审查（agent_1ab56972，PASS findings=0）已落账。
- 2026-10-05T13:18:28.803192Z · note/context · TASK-106 · 主控补记：TASK-107 在 TASK-106 验证后承接其源码继续交付（同链 108 独立路径），begin 时未声明 continuation_of 故 continued_by 未自动落账；现按 begin 对 continuation_of 的既有行为补写 evidence.continued_by=TASK-107，使 106 可与 107/108 整链验收（accept 的 successor 机制）。
- 2026-10-05T13:24:42.946105Z · note/decision · 二阶段任务拆分（DEC-phase2-split-20261005，承接 DEC-refactor-roadmap-20261005 第 3 条）：基于只读探查（口径三把键 scopeQueryArgs/scopePageKey/viewCacheKey 维度不对称；limit:100000 唯一路径=reloadFilteredEntries；mergeSnapshotEntries 六调用点=nav 三处缓存回放+bootstrap 三处后端真值；viewEntriesCache LRU 8 键但单键无上界；GR 对账 read 单向/starred 双向 vs Fever read 双向；sync_queue 无状态列）拆四卡：TASK-109 查询口径统一收口+merge bump 收窄（fromBackend）；TASK-110 筛选视图真分页替代 limit:100000（稳定序+同步偏移去重+切排序重拉对齐 all）；TASK-111 缓存实体预算+后台刷新保位（顶条锚定）；TASK-112 协议对账冲突政策显式化+兼容矩阵文档（Rust+docs，含过时注释修正与副本传播政策文档化）。四态同步展示归三阶段交互一致性。执行沿用一阶段流程：逐任务 prepare→编码子代理→门禁→全新审查子代理→CI→合并。

## 教训

- 2026-09-24T05:06:30.595590Z · note/lesson · 订正 2026-09-23T07:13:57Z 那条 Batch 2 记录：其「新增断言 +7，每条都做过修前/修后成对验证」不准确——独立审查实测新增 9 个 checkNew（frontend-regression.mjs 817/1586/1626/1629/1640/2212/2220/2224/2227 行），其中 1626、2212、2227 三条在修前代码下也通过（不具判别力）；合计 348/348 的总数无误。纪律：声称「成对验证」必须附每条断言对应的变异与变红输出，否则只写「新增 N 条」。
- 2026-09-28T05:03:29.843064Z · note/lesson · TASK-092 · scope 失败已出现 16 次：Interrupted run recovered: 2026-09-28 核对：tasklist+CIM 确认无 fluxreader 相关 python/cargo/chrome 进程（现有 node 进程属 Adobe 与 D:\NextCreator 其他项目，不属于本任务）；写交互报告的子代理 12:56 追。下次准备/实现前先核对这一点。
- 2026-09-28T05:18:30.528502Z · note/lesson · TASK-092 · scope 失败已出现 17 次：Out-of-scope changes: .workflow-kit/binding.json, .workflow-kit/docs/workflow/REFACTOR.md, .workflow-kit/docs/workflow/design-note.md, .workflow-kit/docs/workfl。下次准备/实现前先核对这一点。
- 2026-09-28T05:20:30.786457Z · note/lesson · TASK-092 · protocol 失败已出现 7 次：Worker result must match the complete worker-result contract。下次准备/实现前先核对这一点。
- 2026-09-28T05:53:41.267765Z · note/lesson · TASK-092 · protocol 失败已出现 8 次：Worker result must match the complete worker-result contract。下次准备/实现前先核对这一点。
- 2026-09-28T06:23:16.626668Z · note/lesson · workflow-kit 新引擎（2026-09-28 升级）三处契约坑，下次直接按此准备：①worker-result JSON 键集必须与契约精确相等——finish 成功时工具会自动给文件加 changed_files_source 键并改写 changed_files，从已 finish 的文件复制会带入多余键触发 protocol 阻塞；应始终从 worker 原始 9 键文件重建。②review 报告 ui_review.checked_states 每条必须恰为 ui_check id 字符串（整串相等，不能带结论后缀），evidence_files 只收 tasks/evidence/ 与 tasks/runs/ 下非空文件（截图+报告至少各一）。③PASS 且 findings 为空的报告要求 review_checks 恰好覆盖 5 个规范 area（requirements/regression/failure_paths/maintainability/performance，各一条、无自定义名、无多余条目），每条必带 evidence_files（候选文件或 evidence/runs 附件，tmp/ 无效）；观察性备忘写 summary 与 analysis，不进 findings。M22 变异证据已随 TASK-092 归档可作范例。
- 2026-09-28T17:13:58.507375Z · note/lesson · TASK-099 · protocol 失败已出现 9 次：Invalid worker status/summary。下次准备/实现前先核对这一点。
- 2026-10-05T14:31:46.341244Z · note/lesson · 历史会话证据丢失教训（DEC-evidence-loss-repair-20261005）：审查代理产出的变异测试/门禁日志若只写 tasks/evidence/ 而不随提交入库，摘要校验将永久悬空。本轮起审查报告引用的证据文件必须随任务记录一并提交。存量同型缺陷：TASK-092/093/094/096/097（旧批次，不阻塞操作）的悬空引用仍在，待后续统一处置。

## 最近事件

- 2026-10-05T13:24:42.946105Z · note/decision · 二阶段任务拆分（DEC-phase2-split-20261005，承接 DEC-refactor-roadmap-20261005 第 3 条）：基于只读探查（口径三把键 scopeQueryArgs/scopePageKey/viewCacheKey 维度不对称；limit:100000 唯一路径=reloadFilteredEntries；mergeSnapshotEntries 六调用点=nav 三处缓存回放+bootstrap 三处后端真值；viewEntriesCache LRU 8 键但单键无上界；GR 对账 read 单向/starred 双向 vs Fever read 双向；sync_queue 无状态列）拆四卡：TASK-109 查询口径统一收口+merge bump 收窄（fromBackend）；TASK-110 筛选视图真分页替代 limit:100000（稳定序+同步偏移去重+切排序重拉对齐 all）；TASK-111 缓存实体预算+后台刷新保位（顶条锚定）；TASK-112 协议对账冲突政策显式化+兼容矩阵文档（Rust+docs，含过时注释修正与副本传播政策文档化）。四态同步展示归三阶段交互一致性。执行沿用一阶段流程：逐任务 prepare→编码子代理→门禁→全新审查子代理→CI→合并。
- 2026-10-05T13:29:45.354549Z · prepare · TASK-109 · 任务已冻结：查询口径统一收口 + merge bump 按真值来源收窄（二阶段①，承接审计块②）；范围 src, tools/frontend-regression.mjs
- 2026-10-05T13:30:27.860424Z · checkpoint · TASK-109 · 开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-05T13:59:37.861979Z · checkpoint · TASK-109 · 编码结果已记录，差异范围已核对：src/store/internals.ts, src/store/slices/bootstrap.ts, src/store/slices/nav.ts, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-05T14:00:10.767598Z · checkpoint · TASK-109 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-05T14:11:39.221015Z · checkpoint · TASK-109 · 当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-10-05T14:28:26.639138Z · recompute · TASK-098 · 重算派生摘要 quality_digest；依据：DEC-evidence-loss-repair-20261005（悬空证据引用移除后的摘要重算，非工具变更）
- 2026-10-05T14:28:30.930235Z · recompute · TASK-099 · 重算派生摘要 quality_digest；依据：DEC-evidence-loss-repair-20261005（悬空证据引用移除后的摘要重算，非工具变更）
- 2026-10-05T14:28:35.314405Z · recompute · TASK-101 · 重算派生摘要 quality_digest；依据：DEC-evidence-loss-repair-20261005（悬空证据引用移除后的摘要重算，非工具变更）
- 2026-10-05T14:30:21.992063Z · batch · 关闭 BATCH-d11d22edcd1c4faf8453695d32926081，开启 BATCH-a57865b52aa4449ea24f0f15216d72c5；策略允许自动续批
- 2026-10-05T14:31:23.276760Z · prepare · TASK-110 · 任务已冻结：筛选视图真分页——废除 limit:100000 近似全集（二阶段②，审计块②/完成标准「旧文章可达」）；范围 src, tools/frontend-regression.mjs
- 2026-10-05T14:31:46.341244Z · note/lesson · 历史会话证据丢失教训（DEC-evidence-loss-repair-20261005）：审查代理产出的变异测试/门禁日志若只写 tasks/evidence/ 而不随提交入库，摘要校验将永久悬空。本轮起审查报告引用的证据文件必须随任务记录一并提交。存量同型缺陷：TASK-092/093/094/096/097（旧批次，不阻塞操作）的悬空引用仍在，待后续统一处置。

## 如何继续

1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。
2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。
3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。
4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](../tasks/PROJECT_STATE.md)。
