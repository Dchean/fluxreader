<!-- project-workflow: generated view; edit task JSON instead -->
# 接手与恢复笔记

任何 Agent 接手前先读本文件，再运行 `python .workflow-kit/scripts/project_workflow.py resume --root .`。本文件由任务记录、检查点和日志生成；事实以 JSON 记录和原始证据为准。

## 当前状态

**项目进度 · fluxreader**

目标：以 workflow-kit 2026-09-18.3 新周期对 fluxreader 持续改进：修复全项目体检发现的缺陷（P1/P2），处置功能空壳与死代码确保无未落实的宣称能力，完成 ingestion.rs 拆分与结构性硬化（pull 游标守卫、app_settings 收口），使项目稳定规范、代码与技术路线优雅高效

当前阶段：**验收与交付**

阶段目标：核对完整范围，交付可运行成果、使用说明及适用的恢复办法

| 阶段 | 目标 | 状态 |
| --- | --- | --- |
| 需求与目标 | 明确目标、已有 Bug、新功能、其他要求、质量目标和执行边界 | 已完成 |
| 分析与方案 | 记录参考、原始基线状态与限制，说明维护、稳定和性能取舍，并确认路线 | 已完成 |
| 界面预览 | 验证关键流程、整体设计和控件完整状态，确认后沿用前端实现 | 不适用 |
| 分步实施 | 落实已确认的完整范围，逐步交付并保持已验收行为：社交布局下正文经常一直加载，切换一下布局又能秒加载；双向同步未真正做到：订阅操作与文章状态变更未回传同步后端；本地抓取模式下，本地文章数量与状态和同步后端不一致 | 分批推进 |
| 回归与审查 | 以需求、失败路径、适用界面检查、维护性和性能证据核对当前组合候选 | 分批推进 |
| 验收与交付 | 核对完整范围，交付可运行成果、使用说明及适用的恢复办法 | 当前 |

**完整验收目标**：体检报告 AUDIT-20260919-v2.md 的 P1/P2 缺陷经确认后全部修复并有修前复现/修后验证的成对证据；结构性硬化落地：pull 分块失败不推进增量游标；app_settings 读取收口为类型化助手；ingestion.rs 拆分为领域模块，行为零变化，四门禁不回归；死代码/空壳集群处置完毕（删除或裁决保留），无未落实的宣称能力；设计边界项（P2-12/P3-11 等）经 owner 逐项裁决：实施或注释明示保留；既有质量底线延续：cargo 161/0/9、lint 0/0、build exit 0、frontend 283/283 不回退（通过数可增不可减）

**质量目标**：维护性—单体模块拆分为领域模块（延续 db.rs 试点模式），前端补行为测试，lint/typecheck/test 作为门禁；稳定性—现有 Rust e2e 与前端回归不回归；同步、抓取、播放主流程稳定

**性能安排**：社交布局正文加载不再无限等待，与切换布局后的秒开对齐

已建任务 41 项：已验收 37，待验收 1，阻塞 0。

| 任务 | 状态 | 目标 / 下一步 |
| --- | --- | --- |
| [TASK-069 · 结构性硬化收口：pull 游标失败守卫、app_settings 读取收口、行类型 fixture 防漂移、仓库卫生（REQ-103）](<../tasks/cards/TASK-069.md>) | 已验证，待验收 | 当前候选的测试与审查通过（independent）；继续已授权任务；所属功能完成后请用户验收 |
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

另有 29 项记录可在任务总览查看。

**已确认但尚未拆分的需求**：死代码/空壳集群处置与 P3 卫生项：删除零生产调用代码（db/feeds.rs:245,257,269 Miniflux 兜底三查询、ingestion.rs:330-407 旧版 refresh_feed、greader.rs:501-518 未用方法、db/sync_map.rs 被取代的逐条查询、sync/mod.rs:29 fallback_entries 恒 0 与模块注释修订、lib/api.ts:503 api.syncNow、AiEvent::Error 删或接通）；P3 项逐条处置（吞错 warn 化、sync_save 留空只复用 password、purge_remote_data 范围、cleanup_cache 时区、LIMIT 绑定、normalize 去重一致化、gist 孤儿、init unwrap 加固、快捷键浮层让路、播放中同集切换、批量标读合并等）；设计边界项 P2-12（配置同步删除语义）/P3-11（远端退订本地删除）与 P2-10 后半/P2-11 修复立项前逐项请 owner 裁决；ingestion.rs（766 行，最后一个未拆旧单体）拆分为 ingestion/ 领域模块（conditional_get/parse_feed/map_entry/staged 刷新/favicon 发现），沿用先补断言→纯搬运→四门禁配方，crate::ingestion 路径不变，行为零变化

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

## 未完成的上下文、决策与待办（Agent 笔记）

- 2026-09-19T07:45:48.458674Z · note/decision · TASK-060 · owner 确认 TASK-060 报告 §4 的可达性结论成立：upsert_remote_entry 的 pending 守卫分支（entries.rs:224）为死代码，其保护场景由 merge_remote_status（entries.rs:76）承担；owner 选择「确认死代码，立项删除」——将立项新任务删除该死分支并走完整测试与审查流程（来源：验收问答 2026-09-19，同场验收 TASK-060，DEC-0126898ab7284c73881f86220cfb5827）
- 2026-09-19T09:25:32.113666Z · note/decision · REQ-007 清单遗留项归档为已知限制（owner 2026-09-19 选择记录项目级验收，遗留项随项目归档、后续想做再另立任务）：需产品决策的设计边界 P1-6（本地直连新文章不回写远端，协议限制）/P2-12（配置同步无删除语义）/P3-11（远端退订本地永不删，疑有意保守设计）；P2 残留 P2-9（Miniflux 兜底路径未实现）/P2-10 后半/P2-11/P1-7 残留（卡片挂载自动生成译文 effect）；P3 卫生 P3-2（sync_now 两份实现漂移）/P3-4/P3-5（吞错点）/P3-6/P3-7/P3-8（错误处理边角）/P3-9（init 期 panic 风险点三处）/P3-10（AiEvent::Error 从不发送）；证据缺口 P2-2/P2-5/P2-6（代码已修，测试 harness 无 DOM 无渲染级证据）。来源：TASK-051 报告逐项处置清单（DEC-fix-all-findings-20260917 框架）
- 2026-09-19T09:40:09.510616Z · note/decision · 新周期 legacy_review（owner 2026-09-19 指令：以更新后的 workflow-kit 从零启动新一轮分析）。已读来源：.workflow-kit/tasks/ 全部记录（PROJECT.json stage=complete、DECISIONS 79 条、33 任务、194 运行）、PROJECT_STATE.md、JOURNAL.md、docs/FINDINGS-REQ-007.md、docs/FINDINGS-IGNORED-TESTS.md。保留约束：BRIEF.compatibility 确认继续有效（SQLite 与迁移、Fever/GReader/本地抓取协议行为、现有 UI 布局与交互习惯）；回归底线=161 条 Rust 测试 + 283 条前端断言 + lint/build 门禁不弱化。旧任务处置：33 任务全部验收（TASK-046 按流程取消），stage=complete；归档的约 15 项已知限制（见 2026-09-19 note）仅作为本轮排查线索重新验证，不自动纳入实施。流程冲突结论：旧授权（含 DEC-fix-all-findings「把发现的问题都修复」）仅限旧周期，不自动延续；新周期运行边界经用户确认沿用（DEC 本日新周期启动条目），重构路线待体检证据呈现后由 owner 重新选择。角色结论：当前会话是总控；Worker 身份只来自明确 task_id/run_id 执行包
- 2026-09-19T23:37:23.137875Z · note/todo · TASK-067 · 审查清查补充（TASK-067 r1 PASS，非本候选缺陷）：N10 枚举外仍有 3 处同类未接住点为修前既有——bootstrap.ts:315（anchorToArticle 的 void setRead）、reader.ts:386-387（toggleEntryFlag 的 void setRead/setStarred，时间流卡片按钮路径）、SyncTab.tsx:79（doSave 后 syncStatus().then）；另 settings.ts:56 void setSetting（N9 最终落库点）。建议并入 REQ-104 P3 卫生批次收口
- 2026-09-20T01:11:57.419524Z · note/context · TASK-069 · TASK-069 阻塞核查（manager 只读）：failure_kind=action_required，唯一触发点是 worker-result 的 unresolved_items 非空（status=ready_for_verification、requested_actions=[]、blocked_reason=null）。RUN-93cf2b 的 changes.json 为 {changed_files:[]} 且相对基线无改动，TASK-069 范围检查已通过，本阻塞不是 scope 类，无需撤销任何改动。工作区核对四项成果均在盘：greader_pull.rs:79/84/121 与 fever_pull.rs:65/74/99/118/164 失败计数守卫；db/settings.rs:44 app_settings_bool、:54 app_settings_str；调用点 lib.rs:41/50、commands/mod.rs:20、scheduler.rs:64/208；新增 tests/pull_cursor_e2e.rs、tests/row_fixture_e2e.rs、tests/fixtures/row_fixture.json，tools/frontend-regression.mjs 已改。未决项1 _rev501-bak/：空目录（0 项）、未跟踪、git 无法提交空目录，且 .gitignore 与 HEAD 零差异（*.log 与 tmp/ 早在 fee0934 已入库），无需忽略规则。未决项2 lib.rs:57-87 resolve_close 为读改写路径，不在卡面 N-硬2 声明的六处调用点内；其中 let _ set_setting 吞错正属 REQ-104 P3「吞错 warn 化」主题。另两点：本任务 run 的 checks 为空，四门禁数值目前仅来自 Worker 自述、尚未实测留证；RUN-93cf2b-worker-result.json 的 summary 为双重编码乱码，下次写 worker-result 须用 UTF-8 文件工具而非 shell heredoc。处置待 owner 确认后再 unblock。
- 2026-09-20T01:22:47.075296Z · note/todo · TASK-069 · REQ-104 待办接收（TASK-069 处置移交）：lib.rs resolve_close（约 57-87 行）的读改写路径未纳入 N-硬2 收口范围，其中 let _ = crate::db::set_setting(&conn, app_settings, ...) 吞掉写失败，属 REQ-104 P3「吞错 warn 化」主题；与既有 TASK-067 记录的三处未接住点（bootstrap.ts:315、reader.ts:386-387、SyncTab.tsx:79）及 settings.ts:56 void setSetting 同批处理。另 N-硬4 实际收敛：根目录 13 个 *.log 为未跟踪文件（HEAD 无 .log 入库）、脚本事故产物（字面文件名，含 db 变量插值痕迹）已被跟踪并删除；.gitignore 的 *.log 与 tmp/ 早在 fee0934 已入库，故相对 HEAD 零差异、无需再补；_rev501-bak/ 为空目录且未跟踪，git 不提交空目录，无动作关闭。
- 2026-09-20T01:31:13.687560Z · note/todo · TASK-069 · 总控只读核查补充（REQ-104 待办，非本候选缺陷，不阻塞 TASK-069）：N-硬1 收口后仍存在同类未接住点——greader_pull.rs:31-59 的 item_ids 分页循环失败时只 push report.errors 后 break，all_item_ids 为空 ⇒ 下游 chunks(100) 循环不执行 ⇒ chunk_failures 仍为 0 ⇒ 第 121-125 行游标照常推进到 now。即『本轮什么都没拉到，窗口却被跳过』，与 N-硬1 修前形态同类（该窗口条目只能等全量同步补回）。已用 git show HEAD:... 逐字节比对该 zone 证明为修前既有、非本候选引入；REQ-103 验收 1 只声明『分块失败』，故本条超出 N-硬1 声明范围，不构成本候选缺陷。建议并入 REQ-104 时与 N-硬1 同批收口（判据：item_ids 分页失败时同样不推进 last_sync_ts）。另注：fever_pull.rs 对称性已核实（三处 fetch_failures 计数 + 时间戳游标守卫；last_sync_entry_id 只计已合并条目，注释所述确实安全）。
- 2026-09-20T01:33:49.782611Z · note/todo · TASK-069 · 总控只读核查（REQ-103 验收 2 的残余面，需 owner/审查裁决是否算本候选缺陷）：卡面验收写『N-硬2 六处调用点收口』，但卡面 objective 实际点名只有 5 处（lib.rs read_close_to_tray/read_close_prompt_shown、commands/mod.rs read_dedup_flag、scheduler.rs read_sync_mode_conn/autoSync），候选实际改动也正是这 5 处。REQ-103 原文验收是『app_settings 读取经收口助手，grep 无散落的 from_str 模板复制』——按该口径未完全达成，同类模板仍在：(a) scheduler.rs:256-264 should_notify 的 notifyOnNewArticles 布尔读取（默认 false，未点名、未收口，可直接用 app_settings_bool 收口且 scheduler.rs 在 allowed_paths 内）；(b) scheduler.rs:24-50 read_refresh_config 与 :203-206 的 raw 绑定（卡面明确『refreshInterval 数值留待后续，raw 绑定保留』，属已知延后，其中 smartDedup/autoRefresh 两个布尔本可一并收口）。另 commands/settings.rs:26 解析的是入参 value 而非库读、config_sync.rs:277 是读改写合并、commands/ai.rs:70 读的是 ai_config 键，三者语义不同，不属同类。附带一处小冗余：scheduler.rs:203 取 raw 后 :208 又调 app_settings_bool，同一函数内 app_settings 被读两次（可由审查判定是否值得合并）。以上均为修前既有，非本候选引入。
- 2026-09-20T02:58:21.973412Z · note/decision · TASK-069 · TASK-069 收口完成（verdict 由独立审查给出后再落库，遵循 TASK-066 教训）：r2 独立审查（RUN-d1c63c73，新上下文 independent-reviewer-r2-20260920-t069-e968ecf8）verdict=PASS、findings=[]，五项 review_checks 全 PASS，F1-F4 逐项经审查者自行变异复现确认修复（F1 还原守卫 ⇒ 新游标测试 FAILED；F2 删除 cover 映射 ⇒ (r) ❌ + exit 1；F3 scheduler get_setting(app_settings) 调用点 4→2；F4 git check-ignore 命中 .gitignore:27）。最终候选 digest=0dfded14…、验证 RUN-5380e8e4（cargo 177/0/9、lint 0/0、build exit 0、frontend 303/303）。本轮共 1 个修复轮（上限 4），任务身份/预算沿用。审查者另报一处环境异常：审查窗口内另有进程改过 src/lib/api.ts 并重生 dist-test（经核对为总控自身 author 常量变异，已还原且 api.ts 与 HEAD 逐字节一致、dist-test 已干净重生 303/303），非候选缺陷，已另记 lesson 约束‘审查期间总控只做只读核查’。REQ-104 待办继续挂账：REQ-103 收口后残余的同类点（不在本候选 allowed_paths 内或属数值型推迟）——greader_pull.rs fetch_stream_ids 仍用旧 and_then(parse) 惯用法（C-1 对账路径，非目标）、scheduler.rs read_refresh_config 的数值型读取、scheduler.rs:203 附近已合并、lib.rs resolve_close 读改写与 let _ = set_setting 吞错、TASK-067 记录的三处前端未接住点与 settings.ts:56。

## 教训

- 2026-09-19T16:08:24.966804Z · note/lesson · TASK-064 · protocol 失败已出现 3 次：Worker changed_files does not match the observed diff; declared but unchanged: src-tauri/src/db/articles_tests.rs; declare either the task's cumulative changes 。下次准备/实现前先核对这一点。
- 2026-09-19T22:45:21.591111Z · note/lesson · TASK-066 · protocol 失败已出现 4 次：Worker changed_files does not match the observed diff; declared but unchanged: src/components/Reader.tsx, src/components/Timeline.tsx, src/store/slices/ai.ts, s。下次准备/实现前先核对这一点。
- 2026-09-19T22:51:40.263382Z · note/lesson · TASK-066 · 两个流程教训：① 审查 FAIL 报告的回录要求工作区与冻结候选逐字一致——修复轮改动后无法回录（selected_snapshot 按工作区实时重算），本轮因先改后录被迫取消 TASK-065 并以 TASK-066 收口；正确顺序是先回录 FAIL 触发 review_failure，再在修复轮 begin 内整改。② 自研门禁的新增断言必须放在汇总统计之前（checkNew 只 push 不抛错，汇总后的断言永远无法影响退出码），且以退出码而非日志行核对变异检出
- 2026-09-20T00:41:49.368937Z · note/lesson · TASK-068 · scope 失败已出现 6 次：Out-of-scope changes: .workflow-kit/tasks/items/TASK-068.json, ''' + $db + ''' (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, s。下次准备/实现前先核对这一点。
- 2026-09-20T00:43:48.853570Z · note/lesson · TASK-068 · scope 失败已出现 7 次：Out-of-scope changes: ''' + $db + ''', src-tauri/src/db.rs (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/sett。下次准备/实现前先核对这一点。
- 2026-09-20T00:51:58.628709Z · note/lesson · TASK-069 · protocol 失败已出现 5 次：Worker changed_files does not match the observed diff; declared but unchanged: .gitignore, src-tauri/src/commands/mod.rs, src-tauri/src/db.rs, src-tauri/src/db/。下次准备/实现前先核对这一点。
- 2026-09-20T01:31:27.128668Z · note/lesson · TASK-069 · 环境事实（影响后续所有任务的 build 门禁）：在受限文件沙箱下 npm run build 会以 exit 1 失败，报 vite.config.ts 加载失败 + rollup/rolldown 插件 'Error: spawn EPERM'。根因是 Vite 的 windowsSafeRealPathSync → optimizeSafeRealPathSync 用 child_process.exec 抓管道输出（node:child_process spawn），而 DSH 受限沙箱禁止程序开命名管道 —— 这是沙箱边界，不是产品缺陷。同一命令在放宽权限下 exit 0（vite v8.2.2, 84 modules, built in 779ms）。判据：build 失败若堆栈含 spawn EPERM + optimizeSafeRealPathSync，先按沙箱边界处理，不要记为 test_failure 或改代码。另注：PowerShell 里 npm.ps1 被执行策略禁用，手动跑门禁要用 npm.cmd；workflow 的 verify 走 resolve_program → node npm-cli.js，不受影响。
- 2026-09-20T02:45:17.295823Z · note/lesson · TASK-069 · 总控协作教训（本轮实际踩到）：独立审查者在跑变异复现时，总控不应同时对同一工作区做变异/临时改动——本轮总控在审查进行中自行做了 api.ts 的 author 常量变异（虽 1 分钟内已按备份还原并经 git diff 确认与 HEAD 逐字节一致），而审查者同时在改 greader_pull.rs 做 F1 变异。两者叠加会让 selected_snapshot 与冻结 candidate_digest 短暂不一致，既可能污染审查者观察到的现场，也会掩盖『是谁改的』。正确做法：审查运行期间总控只做只读核查（read/grep/git diff 只读、不写产品文件），把变异取证留给审查者或安排在审查开始前/结束后；确需并行时先约定互斥的文件集合。注：本任务审查者的变异属授权行为且会自行还原，报告落盘前以 sha256 复核候选 156 文件全部复原即可。

## 最近事件

- 2026-09-20T01:27:10.069147Z · checkpoint · TASK-069 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T01:31:13.687560Z · note/todo · TASK-069 · 总控只读核查补充（REQ-104 待办，非本候选缺陷，不阻塞 TASK-069）：N-硬1 收口后仍存在同类未接住点——greader_pull.rs:31-59 的 item_ids 分页循环失败时只 push report.errors 后 break，all_item_ids 为空 ⇒ 下游 chunks(100) 循环不执行 ⇒ chunk_failures 仍为 0 ⇒ 第 121-125 行游标照常推进到 now。即『本轮什么都没拉到，窗口却被跳过』，与 N-硬1 修前形态同类（该窗口条目只能等全量同步补回）。已用 git show HEAD:... 逐字节比对该 zone 证明为修前既有、非本候选引入；REQ-103 验收 1 只声明『分块失败』，故本条超出 N-硬1 声明范围，不构成本候选缺陷。建议并入 REQ-104 时与 N-硬1 同批收口（判据：item_ids 分页失败时同样不推进 last_sync_ts）。另注：fever_pull.rs 对称性已核实（三处 fetch_failures 计数 + 时间戳游标守卫；last_sync_entry_id 只计已合并条目，注释所述确实安全）。
- 2026-09-20T01:31:27.128668Z · note/lesson · TASK-069 · 环境事实（影响后续所有任务的 build 门禁）：在受限文件沙箱下 npm run build 会以 exit 1 失败，报 vite.config.ts 加载失败 + rollup/rolldown 插件 'Error: spawn EPERM'。根因是 Vite 的 windowsSafeRealPathSync → optimizeSafeRealPathSync 用 child_process.exec 抓管道输出（node:child_process spawn），而 DSH 受限沙箱禁止程序开命名管道 —— 这是沙箱边界，不是产品缺陷。同一命令在放宽权限下 exit 0（vite v8.2.2, 84 modules, built in 779ms）。判据：build 失败若堆栈含 spawn EPERM + optimizeSafeRealPathSync，先按沙箱边界处理，不要记为 test_failure 或改代码。另注：PowerShell 里 npm.ps1 被执行策略禁用，手动跑门禁要用 npm.cmd；workflow 的 verify 走 resolve_program → node npm-cli.js，不受影响。
- 2026-09-20T01:33:49.782611Z · note/todo · TASK-069 · 总控只读核查（REQ-103 验收 2 的残余面，需 owner/审查裁决是否算本候选缺陷）：卡面验收写『N-硬2 六处调用点收口』，但卡面 objective 实际点名只有 5 处（lib.rs read_close_to_tray/read_close_prompt_shown、commands/mod.rs read_dedup_flag、scheduler.rs read_sync_mode_conn/autoSync），候选实际改动也正是这 5 处。REQ-103 原文验收是『app_settings 读取经收口助手，grep 无散落的 from_str 模板复制』——按该口径未完全达成，同类模板仍在：(a) scheduler.rs:256-264 should_notify 的 notifyOnNewArticles 布尔读取（默认 false，未点名、未收口，可直接用 app_settings_bool 收口且 scheduler.rs 在 allowed_paths 内）；(b) scheduler.rs:24-50 read_refresh_config 与 :203-206 的 raw 绑定（卡面明确『refreshInterval 数值留待后续，raw 绑定保留』，属已知延后，其中 smartDedup/autoRefresh 两个布尔本可一并收口）。另 commands/settings.rs:26 解析的是入参 value 而非库读、config_sync.rs:277 是读改写合并、commands/ai.rs:70 读的是 ai_config 键，三者语义不同，不属同类。附带一处小冗余：scheduler.rs:203 取 raw 后 :208 又调 app_settings_bool，同一函数内 app_settings 被读两次（可由审查判定是否值得合并）。以上均为修前既有，非本候选引入。
- 2026-09-20T02:09:42.233780Z · checkpoint · TASK-069 · Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-20T02:11:53.045156Z · checkpoint · TASK-069 · 开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T02:28:14.528064Z · checkpoint · TASK-069 · 编码结果已记录，差异范围已核对：.gitignore, src-tauri/src/scheduler.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/sync/greader_pull.rs, src-tauri/tests/fixtures/row_fixture.json, src-tauri/tests/mock_greader.rs, src-tauri/tests/pull_cursor_e2e.rs, src-tauri/tests/row_fixture_e2e.rs, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-20T02:28:41.953318Z · checkpoint · TASK-069 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-20T02:29:35.681205Z · note/progress · TASK-069 · TASK-069 修复轮 r1（RUN-64e391b4，第 1/4 轮）已完成并重新验证：独立审查四项发现全部处置。F1=真实缺陷（greader 的 reading-list id 列举失败不计入守卫 ⇒ 游标照常推进、失败窗口被跳过）：新增 id_failures 计数（列举失败 + continuation 无法解析/本页无 id 均计入），守卫改为 id_failures+chunk_failures==0；fever 对称补 collection_failures；新增回归测试 pull_id_listing_failure_keeps_last_sync_ts 与 mock 注入 fail_reading_list_ids（只让主列举失败、对账仍成功，隔离被测路径）；变异取证：守卫还原为只计 chunk_failures ⇒ 新测试 FAILED，还原后通过。F2=(r) 断言被证伪（整行删除 cover 映射仍 301/301）：改为键集合+逐键取值全量核对（missing/extra/wrong，数组按项比较），加 2 条比较器自检防断言失效，fixture 可空字段改非空使接错源可检出，Rust 侧期望值同步；变异取证：删除 api.ts 的 cover 映射行 ⇒ (r) ❌ + exit 1，还原后 303/303。F3=scheduler.rs notifyOnNewArticles 收口为 app_settings_bool（原为逐字相同布尔模板）；顺带合并 auto_sync_backend 内 app_settings 二次读取（行为不变）并注释数值型读取按 N-硬2 推迟。F4=补 _rev501-bak/ 忽略规则（git check-ignore 现命中 exit 0，原与 HEAD 逐字节相同）。新候选 digest 0dfded14…，验证 RUN-5380e8e4：cargo 177/0/9、lint 0/0、build exit 0、frontend 303/303 全绿；依赖零改动、全部 LF、真实库未写入。
- 2026-09-20T02:45:17.295823Z · note/lesson · TASK-069 · 总控协作教训（本轮实际踩到）：独立审查者在跑变异复现时，总控不应同时对同一工作区做变异/临时改动——本轮总控在审查进行中自行做了 api.ts 的 author 常量变异（虽 1 分钟内已按备份还原并经 git diff 确认与 HEAD 逐字节一致），而审查者同时在改 greader_pull.rs 做 F1 变异。两者叠加会让 selected_snapshot 与冻结 candidate_digest 短暂不一致，既可能污染审查者观察到的现场，也会掩盖『是谁改的』。正确做法：审查运行期间总控只做只读核查（read/grep/git diff 只读、不写产品文件），把变异取证留给审查者或安排在审查开始前/结束后；确需并行时先约定互斥的文件集合。注：本任务审查者的变异属授权行为且会自行还原，报告落盘前以 sha256 复核候选 156 文件全部复原即可。
- 2026-09-20T02:58:05.042023Z · checkpoint · TASK-069 · 当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-09-20T02:58:21.973412Z · note/decision · TASK-069 · TASK-069 收口完成（verdict 由独立审查给出后再落库，遵循 TASK-066 教训）：r2 独立审查（RUN-d1c63c73，新上下文 independent-reviewer-r2-20260920-t069-e968ecf8）verdict=PASS、findings=[]，五项 review_checks 全 PASS，F1-F4 逐项经审查者自行变异复现确认修复（F1 还原守卫 ⇒ 新游标测试 FAILED；F2 删除 cover 映射 ⇒ (r) ❌ + exit 1；F3 scheduler get_setting(app_settings) 调用点 4→2；F4 git check-ignore 命中 .gitignore:27）。最终候选 digest=0dfded14…、验证 RUN-5380e8e4（cargo 177/0/9、lint 0/0、build exit 0、frontend 303/303）。本轮共 1 个修复轮（上限 4），任务身份/预算沿用。审查者另报一处环境异常：审查窗口内另有进程改过 src/lib/api.ts 并重生 dist-test（经核对为总控自身 author 常量变异，已还原且 api.ts 与 HEAD 逐字节一致、dist-test 已干净重生 303/303），非候选缺陷，已另记 lesson 约束‘审查期间总控只做只读核查’。REQ-104 待办继续挂账：REQ-103 收口后残余的同类点（不在本候选 allowed_paths 内或属数值型推迟）——greader_pull.rs fetch_stream_ids 仍用旧 and_then(parse) 惯用法（C-1 对账路径，非目标）、scheduler.rs read_refresh_config 的数值型读取、scheduler.rs:203 附近已合并、lib.rs resolve_close 读改写与 let _ = set_setting 吞错、TASK-067 记录的三处前端未接住点与 settings.ts:56。

## 如何继续

1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。
2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。
3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。
4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](../tasks/PROJECT_STATE.md)。
