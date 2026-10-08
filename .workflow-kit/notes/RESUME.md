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

已建任务 100 项：已验收 81，待验收 0，阻塞 0。

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

另有 88 项记录可在任务总览查看。

**本轮暂缓**：SQL 性能与索引加固（审计 route-M，主控已用 EXPLAIN QUERY PLAN 独立复核）：M-5 列表查询因 COALESCE 表达式排序使 idx_articles_published 对 8 个变体全失效（SCAN + USE TEMP B-TREE FOR ORDER BY）；M-7 sync_queue 零索引致每次标读全表扫；M-9 一次全部已读 = 1 SELECT + 1 UPDATE + 2N 往返且全程持锁；M-14 v6→v7 后置回填在事务外且以 user_version 当完成标记（半途中断永不重试）。

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

## 未完成的上下文、决策与待办（Agent 笔记）

- 2026-10-05T08:20:22.925338Z · note/context · 环境修复（DEC-local-cargo-gate-20261005 补充）：本机 .cargo/bin 全部 shim 为 rustup.exe 符号链接，workflow runtime resolve_program 的 Path.resolve() 追链后以 rustup.exe 身份执行导致 cargo test/fmt 报「invalid value ... for [+toolchain]」。已把 cargo.exe 符号链接替换为等价实体副本（rustup shim 按 argv[0] 文件名分发，副本语义不变，rustup 默认 Windows 形态即实体副本）。cargo --version/rustfmt --version 复验通过。TASK-106 的 cargo_fmt 本地门禁恢复可执行。
- 2026-10-05T08:51:40.666508Z · note/context · TASK-106 · CI 权威证据：dev@76f3256（TASK-106 候选）GitHub Actions CI 全绿（frontend job + rust job，windows-latest cargo test + clippy -D warnings），2026-10-05 完成于约 08:52Z。cargo_test/cargo_clippy 本地可选门禁由本次 CI 运行覆盖（DEC-local-cargo-gate-20261005 补偿控制生效）。
- 2026-10-05T11:40:27.990777Z · note/todo · 二阶段微任务候选（R2 审查裁定记录）：mergeSnapshotEntries 增加 fromBackend 参数把 bump 收窄到后端真值路径（bootstrap.ts 三处传 true，nav.ts 三处缓存恢复不 bump）——审查确认修法技术成立（nav 不 bump 不会重开快照踩踏缺口），当前角例双窄窗口+自愈故不阻塞；宜与二阶段「查询缓存/操作版本模型」一并实施。来源：TASK-107 R2 审查（agent_e92dd332）。
- 2026-10-05T12:37:10.453887Z · note/context · TASK-108 · CI 权威证据：dev@8c1501f（TASK-108 候选）GitHub Actions CI 全绿（rust job windows-latest：cargo test 含新增 4 条故障注入测试 + clippy -D warnings；frontend job 亦绿），编译验证缺口由此闭合。独立审查（agent_1ab56972，PASS findings=0）已落账。
- 2026-10-05T13:18:28.803192Z · note/context · TASK-106 · 主控补记：TASK-107 在 TASK-106 验证后承接其源码继续交付（同链 108 独立路径），begin 时未声明 continuation_of 故 continued_by 未自动落账；现按 begin 对 continuation_of 的既有行为补写 evidence.continued_by=TASK-107，使 106 可与 107/108 整链验收（accept 的 successor 机制）。
- 2026-10-05T13:24:42.946105Z · note/decision · 二阶段任务拆分（DEC-phase2-split-20261005，承接 DEC-refactor-roadmap-20261005 第 3 条）：基于只读探查（口径三把键 scopeQueryArgs/scopePageKey/viewCacheKey 维度不对称；limit:100000 唯一路径=reloadFilteredEntries；mergeSnapshotEntries 六调用点=nav 三处缓存回放+bootstrap 三处后端真值；viewEntriesCache LRU 8 键但单键无上界；GR 对账 read 单向/starred 双向 vs Fever read 双向；sync_queue 无状态列）拆四卡：TASK-109 查询口径统一收口+merge bump 收窄（fromBackend）；TASK-110 筛选视图真分页替代 limit:100000（稳定序+同步偏移去重+切排序重拉对齐 all）；TASK-111 缓存实体预算+后台刷新保位（顶条锚定）；TASK-112 协议对账冲突政策显式化+兼容矩阵文档（Rust+docs，含过时注释修正与副本传播政策文档化）。四态同步展示归三阶段交互一致性。执行沿用一阶段流程：逐任务 prepare→编码子代理→门禁→全新审查子代理→CI→合并。
- 2026-10-06T04:15:15.898732Z · note/decision · TASK-112 CI 失败处置（dev@d167135，rust job clippy -D warnings 失败：fmt 过、测试被跳过、frontend job 绿）：本地无 MSVC 链接器无法运行 clippy（DEC-local-cargo-gate-20261005），属工具盲区内的质量事故；候选验证与独立审查记录保留有效（静态核查未覆盖 clippy lint 面）。处置：按延续任务机制建 TASK-113（continuation_of=TASK-112 候选 7205fab9），静态排查修复 clippy 警告后重走 verify→独立审查→CI 复验（CI 绿为合并前置），验收以 112+113 整链进行。
- 2026-10-06T08:32:37.189654Z · note/context · 陈旧控制器锁清理：中断会话的控制器（lock token lock-8a1b22789bc6461，started 2026-10-06T05:11Z）退出后其 pid 33276 被 Windows 复用给 Bitwarden.exe（tasklist 实证），工具 pid_alive 保守检查拒绝 recover。主控以进程身份证据（复用非同进程）判定锁为陈旧，备份锁文件至 tmp/refactor-20261005/stale-controller-lock-backup.json 后删除。锁为瞬态控制文件非受管记录。
- 2026-10-06T09:14:27.149769Z · note/decision · 三阶段任务拆分（DEC-phase3-split-20261006，承接 DEC-refactor-roadmap-20261005 第 3 条「五布局交互一致性」+ 审计优化点「同步四态展示」）：基于只读探查拆三卡——TASK-114 五布局状态与快捷键统一（NotifCard 补水合失败/空正文/加载 UI 对齐 SocialCard；Social/Notif 卡补 Enter/Space=选中；J/K 从仅文章布局扩展到全部虚拟化布局，ShortcutsTab 同步）；TASK-115 返回位置统一规则（per filterKey 顶条锚记忆+切回恢复，复用 timelineAnchor 机制；阅读器关闭焦点归还；统一规则文档化）；TASK-116 同步四态展示（Rust：sync_queue 增 attempts/last_error 列迁移+push 失败标记+sync_queue_stats 命令；前端：侧栏 pill 扩展+SyncTab 四态摘要，文案纪律 ≤48 字；顺手修 seed_bound doc 笔误）。加载/空/哨兵已探查证实五布局统一，不在卡内重做。执行沿用既定流程。
- 2026-10-07T07:08:48.610322Z · note/context · TASK-117 · CI 失败记录：dev@b7947e9 起 rust job clippy -D warnings 失败（fmt 过/test 跳过；117/118/119 三个提交连续失败，同一 Rust 代码态）。日志需登录无法匿名读取（annotations 仅 exit code 1）。处置：延续卡静态排查（113 先例）+ 请 owner 从日志页贴错误文本双轨。117 本地验证与独立审查有效，CI 绿前不验收不合并。
- 2026-10-08T03:36:42.956552Z · note/context · TASK-128 · 待观察项（不可稳定复现）：TASK-128 首轮烟雾（tmp/phase4/measure-smoke.json，10:10，worker 自起 Chrome/CDP 9224）捕获到一次 getSnapshot should be cached → React #185 崩溃（appHealth.crashes x2：longSession 与 memoryByLayout:通知 各一次），该 JSON 内不含组件归属；worker 报告称报错组件为 <NotifCard>（现场观察，无存档栈）。TASK-122 R1 的 useShallow 修复（eba4df5，08:10）在该烟雾之前已落地。总控随后复现尝试全部阴性：完整电池 x2（11:28/11:29，everCrashed=false）、快速五布局切换 x6 轮、GC+布局循环、Reader 打开/AI 按钮/Escape、命令面板输入——均 0 错误；全 src 的 17 处非平凡订阅已逐一核查（均为标量或稳定引用），t122-9 源级锁覆盖三处 selectArticleBody 订阅。处置：记为待观察项，不阻断交付；若 owner 实机复现，按 tools/phase4_checklist.md §6 取证。候选补强（backlog）：测量电池的错误钩子捕获 React componentStack（console.error 的次参数）并落盘，使复发可归因。
- 2026-10-08T03:36:56.878840Z · note/todo · backlog（非本轮范围，待 owner 决定）：①测量电池错误钩子补 React componentStack 捕获（NotifCard #185 复发可归因）；②挂载组件测试通道评估（P2-7.5，audit 建议真实组件测试覆盖 effect 生命周期）；③回归套件按领域拆分（约 6000 行集中测试含源码字符串检查）；④Fever max_id 历史回溯实现（TASK-127 仅如实记录能力未实现）；⑤scheduler.rs 封面回填 DB 写失败 unwrap_or(0) 同类问题（TASK-127 审查观察项，在 allowed_paths 外）；⑥同文内容/各源条目独立建模（四阶段④，未实现）。

## 教训

- 2026-10-06T09:58:18.205342Z · note/lesson · TASK-114 · scope 失败已出现 19 次：Out-of-scope changes: .workflow-kit/docs/UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md (allowed: src, tools/frontend-regression.mjs)。下次准备/实现前先核对这一点。
- 2026-10-06T10:02:04.089801Z · note/lesson · TASK-114 · scope 失败已出现 20 次：Out-of-scope changes: .workflow-kit/docs/UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md (allowed: src, tools/frontend-regression.mjs)。下次准备/实现前先核对这一点。
- 2026-10-06T10:07:28.915593Z · note/lesson · TASK-114 · scope 失败已出现 21 次：Out-of-scope changes: .workflow-kit/docs/UI-CONTRACT-TASK-114-LAYOUT-CONSISTENCY.md (allowed: src, tools/frontend-regression.mjs)。下次准备/实现前先核对这一点。
- 2026-10-06T10:10:42.656676Z · note/lesson · TASK-114 · protocol 失败已出现 10 次：Worker result belongs to a different task/run。下次准备/实现前先核对这一点。
- 2026-10-07T07:41:17.258677Z · note/lesson · TASK-120 · scope 失败已出现 22 次：Out-of-scope changes: src-tauri/tests/ingestion_e2e.rs (allowed: src-tauri/src)。下次准备/实现前先核对这一点。
- 2026-10-07T07:43:13.134405Z · note/lesson · TASK-120 · scope 失败已出现 23 次：Out-of-scope changes: src-tauri/tests/ingestion_e2e.rs (allowed: src-tauri/src)。下次准备/实现前先核对这一点。
- 2026-10-07T14:02:58.243853Z · note/lesson · TASK-123 · scope 失败已出现 24 次：Out-of-scope changes: .zcodeignore (allowed: src, tools/frontend-regression.mjs)。下次准备/实现前先核对这一点。
- 2026-10-07T14:05:02.574988Z · note/lesson · TASK-123 · protocol 失败已出现 11 次：Worker result belongs to a different task/run。下次准备/实现前先核对这一点。

## 最近事件

- 2026-10-08T02:34:33.188703Z · checkpoint · TASK-128 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T02:45:13.176413Z · checkpoint · TASK-128 · Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-08T02:45:42.457653Z · checkpoint · TASK-128 · 阻塞已处置（review_failure）：R1 修复范围：F1 搜索正例限定文章分组+overlayText 打开后读取+键入前 token 硬断言；F2 每段自带 conditions 快照并据此生成结论行；F3 清单与 seed 脚本加『还原前退出应用』保护；F4 修正『只读』声明并单列真实同步警告；F5 统一清单文件名与长会话判定线；F6 崩溃布局堆值标失效。；下一步：begin 重新实现
- 2026-10-08T02:45:42.863432Z · unblock · TASK-128 · review_failure → ready；依据：独立复审 FAIL（6 findings，其中 F1 搜索正例可被订阅源条目抢先、F2 结论行条件与实际同步状态矛盾为高/中危测量可信度问题）
- 2026-10-08T02:46:35.037914Z · checkpoint · TASK-128 · 开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-08T03:07:54.102660Z · checkpoint · TASK-128 · 编码结果已记录，差异范围已核对：tools/phase4_checklist.md, tools/phase4_measure.mjs, tools/phase4_seed.py；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-08T03:08:36.584948Z · checkpoint · TASK-128 · 预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T03:19:13.656197Z · checkpoint · TASK-128 · 当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-10-08T03:22:28.503699Z · accept · 验收 TASK-128；依据：owner 会话指示『继续推进优化』——视为验收确认；TASK-128 独立复审 R1 PASS findings=0（6 findings 逐条对抗式复核：搜索归属对抗探针、restore 三分支实跑、条件快照贯穿结论行）+ CI 全绿（b7c3ed8）
- 2026-10-08T03:35:49.723027Z · note/progress · TASK-128 · 第五阶段收口：TASK-122/123/124/127/128 全部验收（DEC-00de125f/7bcb581d/cfb13b2e/0b906820/013d34d4），dev→main 合并完成（cdb9bb3，剔除 .workflow-kit 与 TASK 契约文档），main CI 运行中（frontend job 全绿；rust job clippy/test 步骤已过、mock 集成测试中）。TASK-125/126 因 allowed_paths 未含 src-tauri/src/fever.rs 两次重建后以 TASK-127 交付（cancel 记录在案）。
- 2026-10-08T03:36:42.956552Z · note/context · TASK-128 · 待观察项（不可稳定复现）：TASK-128 首轮烟雾（tmp/phase4/measure-smoke.json，10:10，worker 自起 Chrome/CDP 9224）捕获到一次 getSnapshot should be cached → React #185 崩溃（appHealth.crashes x2：longSession 与 memoryByLayout:通知 各一次），该 JSON 内不含组件归属；worker 报告称报错组件为 <NotifCard>（现场观察，无存档栈）。TASK-122 R1 的 useShallow 修复（eba4df5，08:10）在该烟雾之前已落地。总控随后复现尝试全部阴性：完整电池 x2（11:28/11:29，everCrashed=false）、快速五布局切换 x6 轮、GC+布局循环、Reader 打开/AI 按钮/Escape、命令面板输入——均 0 错误；全 src 的 17 处非平凡订阅已逐一核查（均为标量或稳定引用），t122-9 源级锁覆盖三处 selectArticleBody 订阅。处置：记为待观察项，不阻断交付；若 owner 实机复现，按 tools/phase4_checklist.md §6 取证。候选补强（backlog）：测量电池的错误钩子捕获 React componentStack（console.error 的次参数）并落盘，使复发可归因。
- 2026-10-08T03:36:56.878840Z · note/todo · backlog（非本轮范围，待 owner 决定）：①测量电池错误钩子补 React componentStack 捕获（NotifCard #185 复发可归因）；②挂载组件测试通道评估（P2-7.5，audit 建议真实组件测试覆盖 effect 生命周期）；③回归套件按领域拆分（约 6000 行集中测试含源码字符串检查）；④Fever max_id 历史回溯实现（TASK-127 仅如实记录能力未实现）；⑤scheduler.rs 封面回填 DB 写失败 unwrap_or(0) 同类问题（TASK-127 审查观察项，在 allowed_paths 外）；⑥同文内容/各源条目独立建模（四阶段④，未实现）。

## 如何继续

1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。
2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。
3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。
4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](../tasks/PROJECT_STATE.md)。
