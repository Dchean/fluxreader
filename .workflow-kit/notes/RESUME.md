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

已建任务 46 项：已验收 39，待验收 0，阻塞 0。

| 任务 | 状态 | 目标 / 下一步 |
| --- | --- | --- |
| [TASK-074 · 同步语义行为变更：配置同步删除语义（P2-12）+ 远端退订同步删本地（P3-11）（REQ-104）](<../tasks/cards/TASK-074.md>) | 进行中 | 开始执行，保留原任务身份和截止时间；完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify |
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

另有 34 项记录可在任务总览查看。

**已确认但尚未拆分的需求**：ingestion.rs（766 行，最后一个未拆旧单体）拆分为 ingestion/ 领域模块（conditional_get/parse_feed/map_entry/staged 刷新/favicon 发现），沿用先补断言→纯搬运→四门禁配方，crate::ingestion 路径不变，行为零变化

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

## 未完成的上下文、决策与待办（Agent 笔记）

- 2026-09-20T01:22:47.075296Z · note/todo · TASK-069 · REQ-104 待办接收（TASK-069 处置移交）：lib.rs resolve_close（约 57-87 行）的读改写路径未纳入 N-硬2 收口范围，其中 let _ = crate::db::set_setting(&conn, app_settings, ...) 吞掉写失败，属 REQ-104 P3「吞错 warn 化」主题；与既有 TASK-067 记录的三处未接住点（bootstrap.ts:315、reader.ts:386-387、SyncTab.tsx:79）及 settings.ts:56 void setSetting 同批处理。另 N-硬4 实际收敛：根目录 13 个 *.log 为未跟踪文件（HEAD 无 .log 入库）、脚本事故产物（字面文件名，含 db 变量插值痕迹）已被跟踪并删除；.gitignore 的 *.log 与 tmp/ 早在 fee0934 已入库，故相对 HEAD 零差异、无需再补；_rev501-bak/ 为空目录且未跟踪，git 不提交空目录，无动作关闭。
- 2026-09-20T01:31:13.687560Z · note/todo · TASK-069 · 总控只读核查补充（REQ-104 待办，非本候选缺陷，不阻塞 TASK-069）：N-硬1 收口后仍存在同类未接住点——greader_pull.rs:31-59 的 item_ids 分页循环失败时只 push report.errors 后 break，all_item_ids 为空 ⇒ 下游 chunks(100) 循环不执行 ⇒ chunk_failures 仍为 0 ⇒ 第 121-125 行游标照常推进到 now。即『本轮什么都没拉到，窗口却被跳过』，与 N-硬1 修前形态同类（该窗口条目只能等全量同步补回）。已用 git show HEAD:... 逐字节比对该 zone 证明为修前既有、非本候选引入；REQ-103 验收 1 只声明『分块失败』，故本条超出 N-硬1 声明范围，不构成本候选缺陷。建议并入 REQ-104 时与 N-硬1 同批收口（判据：item_ids 分页失败时同样不推进 last_sync_ts）。另注：fever_pull.rs 对称性已核实（三处 fetch_failures 计数 + 时间戳游标守卫；last_sync_entry_id 只计已合并条目，注释所述确实安全）。
- 2026-09-20T01:33:49.782611Z · note/todo · TASK-069 · 总控只读核查（REQ-103 验收 2 的残余面，需 owner/审查裁决是否算本候选缺陷）：卡面验收写『N-硬2 六处调用点收口』，但卡面 objective 实际点名只有 5 处（lib.rs read_close_to_tray/read_close_prompt_shown、commands/mod.rs read_dedup_flag、scheduler.rs read_sync_mode_conn/autoSync），候选实际改动也正是这 5 处。REQ-103 原文验收是『app_settings 读取经收口助手，grep 无散落的 from_str 模板复制』——按该口径未完全达成，同类模板仍在：(a) scheduler.rs:256-264 should_notify 的 notifyOnNewArticles 布尔读取（默认 false，未点名、未收口，可直接用 app_settings_bool 收口且 scheduler.rs 在 allowed_paths 内）；(b) scheduler.rs:24-50 read_refresh_config 与 :203-206 的 raw 绑定（卡面明确『refreshInterval 数值留待后续，raw 绑定保留』，属已知延后，其中 smartDedup/autoRefresh 两个布尔本可一并收口）。另 commands/settings.rs:26 解析的是入参 value 而非库读、config_sync.rs:277 是读改写合并、commands/ai.rs:70 读的是 ai_config 键，三者语义不同，不属同类。附带一处小冗余：scheduler.rs:203 取 raw 后 :208 又调 app_settings_bool，同一函数内 app_settings 被读两次（可由审查判定是否值得合并）。以上均为修前既有，非本候选引入。
- 2026-09-20T02:58:21.973412Z · note/decision · TASK-069 · TASK-069 收口完成（verdict 由独立审查给出后再落库，遵循 TASK-066 教训）：r2 独立审查（RUN-d1c63c73，新上下文 independent-reviewer-r2-20260920-t069-e968ecf8）verdict=PASS、findings=[]，五项 review_checks 全 PASS，F1-F4 逐项经审查者自行变异复现确认修复（F1 还原守卫 ⇒ 新游标测试 FAILED；F2 删除 cover 映射 ⇒ (r) ❌ + exit 1；F3 scheduler get_setting(app_settings) 调用点 4→2；F4 git check-ignore 命中 .gitignore:27）。最终候选 digest=0dfded14…、验证 RUN-5380e8e4（cargo 177/0/9、lint 0/0、build exit 0、frontend 303/303）。本轮共 1 个修复轮（上限 4），任务身份/预算沿用。审查者另报一处环境异常：审查窗口内另有进程改过 src/lib/api.ts 并重生 dist-test（经核对为总控自身 author 常量变异，已还原且 api.ts 与 HEAD 逐字节一致、dist-test 已干净重生 303/303），非候选缺陷，已另记 lesson 约束‘审查期间总控只做只读核查’。REQ-104 待办继续挂账：REQ-103 收口后残余的同类点（不在本候选 allowed_paths 内或属数值型推迟）——greader_pull.rs fetch_stream_ids 仍用旧 and_then(parse) 惯用法（C-1 对账路径，非目标）、scheduler.rs read_refresh_config 的数值型读取、scheduler.rs:203 附近已合并、lib.rs resolve_close 读改写与 let _ = set_setting 吞错、TASK-067 记录的三处前端未接住点与 settings.ts:56。
- 2026-09-20T03:19:46.637284Z · note/decision · owner 设计边界裁决 #1（P2-12 配置同步删除语义，2026-09-20 问答，选项『实施删除语义』）：owner 选择为 config_sync.rs 实施远程配置项的删除语义，而不是把现状注释为保守设计。现行为：config_sync.rs:141-261 只做 upsert，远端删除的配置项在本地不会删除，且 skipped 计数实际是『已更新』口径。实施要求（立项时落进任务卡验收）：① 远端白名单字段在远端消失时本地也删除（或按 owner 确认的语义回落默认值）；② 修正 skipped 计数口径为真实跳过数；③ 保持 autoStart/closePromptShown 等本地专属字段不被远端删除（现白名单已排除，须保持）；④ 属行为变更，须有修前/修后成对证据与独立审查。风险等级：中高（改数据同步语义）。
- 2026-09-20T03:19:47.667130Z · note/decision · owner 设计边界裁决 #2（P3-11 远端退订本地删除，2026-09-20 问答，选项『实施本地删除』）：owner 选择让远端退订时同步删除本地订阅，而不是把『远端退订本地永不删』确认为有意设计。现行为：pull_feeds 无删除分支（与『本地删除不推远端』的保守设计配套）。实施要求：① 远端权威集合中消失的已绑定订阅在本地删除，且要与 TASK-035 的删除墓碑防复活机制协同（避免下一轮 pull 又把它建回来，也不能把本地新订阅误删）；② 必须保留本地直连订阅（origin='local'）与未绑定订阅，只处理『曾绑定且远端已消失』的源；③ 与 pending 未推送队列的交互须明确（本地刚改名/移动未推送时不得被远端快照删除）；④ 属行为变更且有数据丢失面，须有修前/修后成对证据、失败路径测试与独立审查。风险等级：高（可能丢本地阅读状态）。
- 2026-09-20T03:19:52.368982Z · note/decision · owner 设计边界裁决 #3（P2-10 后半 extract_fulltext 降级可见性，2026-09-20 问答，选项『改结构化 degraded 标志 + 准确文案』）：owner 选择修复——全文提取无法提取/防退化原样返回时不再静默回落原文，改为结构化 degraded 标志 + 准确文案。涉及 commands/settings.rs:90-93 与既有 (P2-10) 前端提示口径；须补失败路径断言（提取失败、防退化返回原文两种形态）并保持成功路径不变。风险等级：低。同时记录裁决 #4（P2-11 AI 空校验与 preset，选项『补空校验 + preset 未知显式提示』）：① ai.rs:118 ai_summarize 补空正文校验（与 translate 对称，约 ai.rs:58-82 的 translate 已有）；② 未知 preset 不再静默回退 deepseek-chat，改为显式提示/报错而不是假装成功；③ 须补断言覆盖空正文与未知 preset 两条路径。风险等级：低。以上四项裁决均已作为 owner 真实决定保存，作为 REQ-104 立项与验收依据。
- 2026-09-20T03:23:37.451121Z · note/decision · owner 四项设计边界裁决已落库为正式决定（供 REQ-104 立项与 test_review 引用）：DEC-req104-p2-12-config-delete-20260920（配置同步实施删除语义）、DEC-req104-p3-11-remote-unsub-20260920（远端退订同步删本地订阅）、DEC-req104-p2-10b-fulltext-degraded-20260920（全文提取 degraded 标志+准确文案）、DEC-req104-p2-11-ai-validation-20260920（补空正文校验+preset 未知显式提示）。四者 scope 均为 REQ-104、issuer=owner、status=accepted，source 引用 2026-09-20 本轮问答的真实选项。对应的决策过程笔记同时保留在日志中（P2-12/P3-11/P2-10b/P2-11 四条 decision 笔记），两者一致。check 通过（errors 0 / warnings 0）。
- 2026-09-20T04:01:32.768251Z · note/todo · TASK-070 · 总控只读核查发现的 REQ-104 残留项（不在 TASK-070 声明的删除清单内，未改动，留待后续批次裁决）：删除前端 api.syncNow 之后，Rust 侧 Tauri 命令 commands::sync_now（commands/sync.rs:265，已在 lib.rs:258 注册）在整仓已无任何调用方——前端同步走 api.syncPhase → sync_phase（分步）与 refresh_all_feeds；grep 'sync_now' 只剩命令自身、sync::sync_now 实现、以及测试对 sync::sync_now 的调用。故 commands::sync_now 现属『注册但无人调用』的空壳命令（审计 P3-2 只点名了前端 api.ts:503，未点名该 Rust 命令）。本任务对它只做了『转调 sync::sync_now』以消除双实现漂移（P3-2 的『双实现』面），未删除命令本身：删除会改变 invoke 命令面，超出本卡 allowed_paths 与验收范围。建议后续任务与 REQ-104 剩余批次一并裁决（删命令 + 从 lib.rs 移除注册，或说明保留理由）。
- 2026-09-20T04:36:13.028647Z · note/todo · TASK-070 · REQ-104 待办接收（TASK-070 移交，均不在本卡 allowed_paths 内，未改动）：① 同源『Miniflux 兜底』陈旧注释三处——src/types.ts:44『该源最近一次直连抓取是否失败（失败走 Miniflux 兜底 + 退避重试）』与 :70『source: miniflux=直连失败兜底拉取』、src-tauri/tests/staged_refresh_e2e.rs:195『供 UI 与 Miniflux 兜底查询』、src-tauri/tests/sync_phases_e2e.rs:246 同源表述；TASK-070 已修正 sync/mod.rs 与 ingestion.rs 两处模块头，但上述文件未授权，建议并入后续 REQ-104 批次统一清理，避免同一不实宣称继续散落。② commands::sync_now（commands/sync.rs:265，lib.rs:258 已注册）在前端 api.syncNow 删除后已无任何调用方，属『注册但无人调用』的空壳命令；TASK-070 只把它改成转调 sync::sync_now 消除 P3-2 的双实现漂移，未删命令本身（删会改变 invoke 命令面，超出本卡范围），建议后续裁决删命令+取消注册或说明保留理由。③ 覆盖缺口：sync/entries.rs 两处 same_fetch_trusted 同源判定（状态合并与 merge_pulled_entry 各一处）目前无测试可杀——审查者实测把两处改为恒信任后完整 cargo test 仍 177/0/9 全绿，含 sync_e2e 的端到端跨源守卫；属修前既有缺口，建议按 GATES『把 Bug 变成保护』补一条能杀掉该判定的失败路径断言。④ get_first_folder_id 零生产调用但其唯一引用在 crate 内测试 sync_extraction_tests.rs（不在 TASK-070 allowed_paths），待与该测试一并处置。
- 2026-09-20T04:51:27.382387Z · note/todo · TASK-070 · TASK-070 注释精度改进（审查窗口内发现，已推迟到报告落盘后处理，不属审查发现）：sync/mod.rs 模块头 F1 修正文写『feeds.fetch_failed 只驱动指数退避重试』，核实后发现该列在后端确有唯一消费面（退避重试），但前端还消费它显示侧栏失败标记（src/lib/api.ts:534 fetchFailed → src/components/Sidebar.tsx:236 feed-error-dot）。严格说『只驱动指数退避重试』是后端层内的准确表述，但从全仓角度略欠完整。建议下一次修复轮（或后续 REQ-104 批次）把措辞改为『后端侧只驱动指数退避重试；前端另有失败标记』，使文档诚实性无死角。注意：本条不构成审查发现，审查者对 F1 的判定依据是『不存在回退后端拉取路径』这一核心事实，该事实表述正确。
- 2026-09-20T07:34:27.506222Z · note/context · TASK-074 · TASK-074 只读勘察发现的范围冲突（需 owner 裁决后再实现；本 RUN 未改任何产品文件）： 验收 A② 要求「修正 skipped 计数口径为真实跳过数，并有断言区分『已更新』与『已跳过』」，但该字段是跨前后端契约： ① Rust 侧 apply_payload 现把 skipped += 1 放在「源 URL 已存在并被白名单字段更新」分支（config_sync.rs:184-212），故 skipped 实际等于已更新数（与审计原文「skipped 实为已更新计数」一致）； ② 该值经 commands/sync.rs 以 {imported, skipped} 返回； ③ 前端 src/lib/api.ts:414 声明其类型，src/components/settings/ConfigSyncSection.tsx:98 直接展示「配置已应用：新增 N 个源，跳过 M 个已存在」。 而 TASK-074 的 allowed_paths 不含 src/lib/api.ts 与 src/components/settings/ConfigSyncSection.tsx。因此「区分已更新与已跳过」若按字面实现（新增 updated 字段、或改变 skipped 含义），必然改动 out-of-scope 的前端契约文件。 三条可选路线： (甲) 仅在 Rust 内新增独立 updated 计数，skipped 保持现有含义与前端文案不变——但这不满足「修正 skipped 口径」的字面要求，需 owner 确认验收可如此降级； (乙) 改 skipped 为真实跳过数并新增 updated，同时把 api.ts 与 ConfigSyncSection.tsx 纳入本任务 allowed_paths（属范围订正，需 owner 批准；有 TASK-068 的 DEC-ec075000 先例）； (丙) 把「跳过」重新定义为「因墓碑/冲突等原因未应用者」（当前几乎恒 0），只改注释与补断言，不改字段含义与前端。 另：B 部分（P3-11 远端退订同步删本地）无此冲突，allowed_paths 已覆盖 subscriptions.rs / db/feeds.rs / 相关测试，可独立实施。 处置：等 owner 选定路线后再 begin 实现，不擅自扩大范围，也不改断言给自己放行。

## 教训

- 2026-09-20T00:41:49.368937Z · note/lesson · TASK-068 · scope 失败已出现 6 次：Out-of-scope changes: .workflow-kit/tasks/items/TASK-068.json, ''' + $db + ''' (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, s。下次准备/实现前先核对这一点。
- 2026-09-20T00:43:48.853570Z · note/lesson · TASK-068 · scope 失败已出现 7 次：Out-of-scope changes: ''' + $db + ''', src-tauri/src/db.rs (allowed: src-tauri/src/sync/greader_pull.rs, src-tauri/src/sync/fever_pull.rs, src-tauri/src/db/sett。下次准备/实现前先核对这一点。
- 2026-09-20T00:51:58.628709Z · note/lesson · TASK-069 · protocol 失败已出现 5 次：Worker changed_files does not match the observed diff; declared but unchanged: .gitignore, src-tauri/src/commands/mod.rs, src-tauri/src/db.rs, src-tauri/src/db/。下次准备/实现前先核对这一点。
- 2026-09-20T01:31:27.128668Z · note/lesson · TASK-069 · 环境事实（影响后续所有任务的 build 门禁）：在受限文件沙箱下 npm run build 会以 exit 1 失败，报 vite.config.ts 加载失败 + rollup/rolldown 插件 'Error: spawn EPERM'。根因是 Vite 的 windowsSafeRealPathSync → optimizeSafeRealPathSync 用 child_process.exec 抓管道输出（node:child_process spawn），而 DSH 受限沙箱禁止程序开命名管道 —— 这是沙箱边界，不是产品缺陷。同一命令在放宽权限下 exit 0（vite v8.2.2, 84 modules, built in 779ms）。判据：build 失败若堆栈含 spawn EPERM + optimizeSafeRealPathSync，先按沙箱边界处理，不要记为 test_failure 或改代码。另注：PowerShell 里 npm.ps1 被执行策略禁用，手动跑门禁要用 npm.cmd；workflow 的 verify 走 resolve_program → node npm-cli.js，不受影响。
- 2026-09-20T02:45:17.295823Z · note/lesson · TASK-069 · 总控协作教训（本轮实际踩到）：独立审查者在跑变异复现时，总控不应同时对同一工作区做变异/临时改动——本轮总控在审查进行中自行做了 api.ts 的 author 常量变异（虽 1 分钟内已按备份还原并经 git diff 确认与 HEAD 逐字节一致），而审查者同时在改 greader_pull.rs 做 F1 变异。两者叠加会让 selected_snapshot 与冻结 candidate_digest 短暂不一致，既可能污染审查者观察到的现场，也会掩盖『是谁改的』。正确做法：审查运行期间总控只做只读核查（read/grep/git diff 只读、不写产品文件），把变异取证留给审查者或安排在审查开始前/结束后；确需并行时先约定互斥的文件集合。注：本任务审查者的变异属授权行为且会自行还原，报告落盘前以 sha256 复核候选 156 文件全部复原即可。
- 2026-09-20T04:51:20.430418Z · note/lesson · TASK-070 · 总控协作纪律（本轮第二次踩到，须固化为硬规则）：**独立审查进行期间，总控一律不得写任何候选文件**。本轮 TASK-070 r2 审查在跑时，总控为改进 F1 注释措辞编辑了 src-tauri/src/sync/mod.rs，导致 selected_snapshot 与冻结的 candidate_digest（f4e1629…）不一致——若审查者此刻计算哈希或复跑测试，会看到『候选与报告不符』或读到半途状态，可能得到无效结论。已立即还原并复核 digest 恢复一致（True）。正确做法：审查窗口内只做只读核查（read/grep/git diff/哈希比对），把一切写操作（包括『只是改注释』）推迟到审查报告落盘之后；若确有必须立即修的问题，先记录待办，等报告写入再 begin 修复轮。此前 TASK-069 审查期间已因同类原因记过一条 lesson（当时是变异实验叠加），本次是注释编辑，说明『只读』的边界要按『是否触碰候选文件』来划，而不是按『改动大小/是否产品逻辑』来划。
- 2026-09-20T06:27:42.491022Z · note/lesson · TASK-070 · 流程观察（TASK-070 连续三轮独立审查均 FAIL，值得记录以免重犯）：三轮 findings **全部**落在『我写的解释性注释不准确』，没有一条落在删除面本身——每一轮审查都独立确认：九项删除在 HEAD 上确实零生产调用、四门禁真绿（cargo 177/0/9、lint 0/0、build 0、frontend 303/303）、无有效覆盖丢失、断言未被改动、157/157 候选哈希一致、依赖零改动、真实库未写入。三轮回合的缺陷模式完全一致且都是我自己制造的：**在注释里写下未经验证的新断言**。r1=模块头宣称了一条当时不存在的兜底（但没说我改的那句本身是否成立）；r2=我把 r1 的修正升级成『从未实现』这一历史绝对断言（git 证明它实现过，0ba940f 才删掉）；r2 同轮还把另一处改成『本轮不该建新条目』而套件自己的断言就反驳它、并把退避误挂到 fetch_failed 上；r3=又修三处。教训（对后续所有任务的注释写作）：① 改注释时只写**当场核实过**的事实，逐句核对（grep/git show/读 SQL），不写『从未/只/总是/必然』这类全称或历史绝对断言；② 涉及历史的说法一律先用 git 查证（本例 9828c8f/51dc66e/0ba940f 就能定案）；③ 涉及机制的说法必须落到具体行号与调用链，不靠印象；④ 同一 PR 内多处同类注释要一次改齐并互相一致（r2 的 sync/mod.rs 与 sync_e2e.rs 就自相矛盾）。另一条工具陷阱（r3 审查者提供）：变异实验后还原文件会使 mtime 早于已编译 rlib，cargo 跳过重编 → 干净候选出现『确定性失败』假象；遇到可疑的确定性失败先强制重编（cargo clean -p app 或 touch）再下结论。
- 2026-09-20T07:28:18.140160Z · note/lesson · 流程教训（TASK-070 收口后发现，总控自身操作失误）：我在立项阶段一次性 prepare 了 TASK-070~073 四张卡，违反 TOOLING.md『依赖任务通过后才 prepare 下一张卡』的指导。后果：TASK-071/072/073 的 input 快照取自 TASK-070 开工前的代码状态，而 TASK-070 合法地改动了 snapshot_paths 覆盖的 src / src-tauri/src / src-tauri/tests / tools，于是 check 报三条『invalid input snapshot: candidate changed』，begin 也被拒（工具设计上要求 ready 任务的输入快照与当前树一致，否则开工基线不正确）。另发现两张卡缺少正确依赖边：prepare 时 TASK-070 尚未 verified，故 071/072/073 只依赖 TASK-069，未声明对 TASK-070 的依赖（虽然串行顺序已用笔记约束）。补救（不手工改 digest，避免『rehash 掩盖变化』）：按工具既有先例（TASK-068→TASK-069）cancel 三张过期卡并重新 prepare 新卡，依赖直接写 TASK-070，使快照取当前树、并让依赖继承 TASK-070 的门禁。教训固化：**一张卡 verified/done 之后才 prepare 下一张**，不要为省事批量预建；批量预建在有串行依赖的链上必然作废。

## 最近事件

- 2026-09-20T07:28:18.140160Z · note/lesson · 流程教训（TASK-070 收口后发现，总控自身操作失误）：我在立项阶段一次性 prepare 了 TASK-070~073 四张卡，违反 TOOLING.md『依赖任务通过后才 prepare 下一张卡』的指导。后果：TASK-071/072/073 的 input 快照取自 TASK-070 开工前的代码状态，而 TASK-070 合法地改动了 snapshot_paths 覆盖的 src / src-tauri/src / src-tauri/tests / tools，于是 check 报三条『invalid input snapshot: candidate changed』，begin 也被拒（工具设计上要求 ready 任务的输入快照与当前树一致，否则开工基线不正确）。另发现两张卡缺少正确依赖边：prepare 时 TASK-070 尚未 verified，故 071/072/073 只依赖 TASK-069，未声明对 TASK-070 的依赖（虽然串行顺序已用笔记约束）。补救（不手工改 digest，避免『rehash 掩盖变化』）：按工具既有先例（TASK-068→TASK-069）cancel 三张过期卡并重新 prepare 新卡，依赖直接写 TASK-070，使快照取当前树、并让依赖继承 TASK-070 的门禁。教训固化：**一张卡 verified/done 之后才 prepare 下一张**，不要为省事批量预建；批量预建在有串行依赖的链上必然作废。
- 2026-09-20T07:28:38.039111Z · checkpoint · TASK-071 · 任务已取消：input 快照被 TASK-070 的合法改动作废（同批预建所致）；且依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，重新按『一张卡完成后再 prepare 下一张』立项；下一步：如需同一目标，准备新的任务并引用本任务作为历史
- 2026-09-20T07:28:38.088446Z · cancel · TASK-071 · input 快照被 TASK-070 的合法改动作废（同批预建所致）；且依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，重新按『一张卡完成后再 prepare 下一张』立项；依据：总控处置：批量预建违反 TOOLING『依赖任务通过后才 prepare 下一张卡』，改用逐张 prepare（2026-09-20）
- 2026-09-20T07:28:44.958837Z · checkpoint · TASK-072 · 任务已取消：input 快照被 TASK-070 的合法改动作废（同批预建所致）；依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，等 TASK-071 完成后再逐张立项；下一步：如需同一目标，准备新的任务并引用本任务作为历史
- 2026-09-20T07:28:45.009059Z · cancel · TASK-072 · input 快照被 TASK-070 的合法改动作废（同批预建所致）；依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，等 TASK-071 完成后再逐张立项；依据：总控处置：批量预建违反 TOOLING『依赖任务通过后才 prepare 下一张卡』（2026-09-20）
- 2026-09-20T07:28:46.195081Z · checkpoint · TASK-073 · 任务已取消：input 快照被 TASK-070 的合法改动作废（同批预建所致）；依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，等前序卡完成后再立项；下一步：如需同一目标，准备新的任务并引用本任务作为历史
- 2026-09-20T07:28:46.243075Z · cancel · TASK-073 · input 快照被 TASK-070 的合法改动作废（同批预建所致）；依赖只声明到 TASK-069、缺 TASK-070 边。按工具纪律取消，等前序卡完成后再立项；依据：总控处置：批量预建违反 TOOLING『依赖任务通过后才 prepare 下一张卡』（2026-09-20）
- 2026-09-20T07:29:22.188133Z · batch · 关闭 BATCH-4f9d7067e5b04d5bbedfdcf3a2b021ee，开启 BATCH-ab2d0e5d3f7643379a6de1fd2544b549；策略允许自动续批
- 2026-09-20T07:29:36.110658Z · prepare · TASK-074 · 任务已冻结：同步语义行为变更：配置同步删除语义（P2-12）+ 远端退订同步删本地（P3-11）（REQ-104）；范围 src-tauri/src/config_sync.rs, src-tauri/src/sync/subscriptions.rs, src-tauri/src/sync/mod.rs, src-tauri/src/sync/phases.rs, src-tauri/src/db/feeds.rs, src-tauri/src/commands/sync.rs, src-tauri/tests/config_sync_e2e.rs, src-tauri/tests/account_lifecycle_e2e.rs, src-tauri/tests/mock_greader.rs, src-tauri/tests/sync_e2e.rs, tools/frontend-regression.mjs
- 2026-09-20T07:30:17.018378Z · note/progress · 会话收口状态（2026-09-20）：本轮完成 TASK-069（REQ-103 结构性硬化收口：pull 游标失败守卫 + app_settings 收口 + 行类型 fixture 防漂移 + 仓库卫生；独立审查 r1 FAIL→修复→r2 PASS，验收提交 452e0e2）与 TASK-070（REQ-104 死代码/空壳集群删除 9 项 + SyncReport 收口 + P3-2 双实现合并；独立审查连续三轮 FAIL 均为注释准确性、删除面每轮被独立确认真死，第四轮 PASS findings 为空，验收提交 48e2763）。owner 四项设计边界裁决（P2-12/P3-11/P2-10 后半/P2-11）已作为正式决定入库（DEC-req104-*）。因批量预建导致 TASK-071/072/073 输入快照过期，已按纪律取消并逐张重启：TASK-074（=原 TASK-071，同步语义行为变更，high risk，依赖 TASK-070）已 prepare 就绪。剩余已确认范围：TASK-074（P2-12 配置同步删除语义 + P3-11 远端退订删本地）→ 全文提取 degraded 标志与 AI 校验（原 TASK-072 内容）→ ingestion.rs 拆分（REQ-105，原 TASK-073 内容）。工作区干净（git status 无改动），check 通过（errors/warnings 全空）。
- 2026-09-20T07:31:48.397880Z · checkpoint · TASK-074 · 开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-20T07:34:27.506222Z · note/context · TASK-074 · TASK-074 只读勘察发现的范围冲突（需 owner 裁决后再实现；本 RUN 未改任何产品文件）： 验收 A② 要求「修正 skipped 计数口径为真实跳过数，并有断言区分『已更新』与『已跳过』」，但该字段是跨前后端契约： ① Rust 侧 apply_payload 现把 skipped += 1 放在「源 URL 已存在并被白名单字段更新」分支（config_sync.rs:184-212），故 skipped 实际等于已更新数（与审计原文「skipped 实为已更新计数」一致）； ② 该值经 commands/sync.rs 以 {imported, skipped} 返回； ③ 前端 src/lib/api.ts:414 声明其类型，src/components/settings/ConfigSyncSection.tsx:98 直接展示「配置已应用：新增 N 个源，跳过 M 个已存在」。 而 TASK-074 的 allowed_paths 不含 src/lib/api.ts 与 src/components/settings/ConfigSyncSection.tsx。因此「区分已更新与已跳过」若按字面实现（新增 updated 字段、或改变 skipped 含义），必然改动 out-of-scope 的前端契约文件。 三条可选路线： (甲) 仅在 Rust 内新增独立 updated 计数，skipped 保持现有含义与前端文案不变——但这不满足「修正 skipped 口径」的字面要求，需 owner 确认验收可如此降级； (乙) 改 skipped 为真实跳过数并新增 updated，同时把 api.ts 与 ConfigSyncSection.tsx 纳入本任务 allowed_paths（属范围订正，需 owner 批准；有 TASK-068 的 DEC-ec075000 先例）； (丙) 把「跳过」重新定义为「因墓碑/冲突等原因未应用者」（当前几乎恒 0），只改注释与补断言，不改字段含义与前端。 另：B 部分（P3-11 远端退订同步删本地）无此冲突，allowed_paths 已覆盖 subscriptions.rs / db/feeds.rs / 相关测试，可独立实施。 处置：等 owner 选定路线后再 begin 实现，不擅自扩大范围，也不改断言给自己放行。

## 如何继续

1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。
2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。
3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。
4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](../tasks/PROJECT_STATE.md)。
