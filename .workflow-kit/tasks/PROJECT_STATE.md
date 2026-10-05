<!-- project-workflow: generated view; edit task JSON instead -->
# 项目状态

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

已建任务 77 项：已验收 61，待验收 0，阻塞 0。

| 任务 | 状态 | 目标 / 下一步 |
| --- | --- | --- |
| [TASK-103 · 文章快照与正文水合生命周期统一——刷新不丢正文、水合终态完备（REQ-001）](<cards/TASK-103.md>) | 待执行 | 修复 REQ-001（社交布局正文一直加载）的根因并补齐水合终态机，依据外部审计探针复现的链路（AUDIT-20261005-core-consistency.md）：①src/store/slices/bootstrap.ts reloadFromBackend 快照替换 entries 时新行不含正文且清空 hydratedIds，而虚拟列表按文章 id 保持组件身份、useLazyHydrate（src/components/Timeline.tsx 一带）effect 依赖仅 [id]，同 id 不再触发水合请求，卡片停留「加载正文…」且实际无请求在途。修法要求：快照替换时按 id 保留既有条目的正文字段（content/rawContent/translatedContent/aiSummary/fulltextExtracted/hydrated），新行自带正文（with_content 场景）时以新行为准；hydratedIds 不再无条件清空；hydrated 保留与新行合并逻辑收口到单一函数（避免 bootstrap/bootstrapFromBackend/其他快照路径各自为政）。②水合触发修正：useLazyHydrate 不再只依赖 [id]——卡片挂载期间观察 store 的「无正文 && 未水合 && 无终态 && 无在途」状态，条件重新成立时重新入队（或 reloadFromBackend 完成后对未水合条目统一重新入队，coder 二选一并断言锁定）。③终态机完备（reader.ts hydrateArticleContent / enqueueHydration / retryHydration）：成功含空正文 → hydrated=true 终态「无正文」（不再显示加载占位、不无限重试）；请求 rows 中缺失的 id → 「文章不存在」终态（hydrationErrors 明确错误或从 entries 清理，不得留加载占位）；请求失败 → hydrationErrors 保留内联重试；空 ids/空 rows 不再静默 return 留占位；乱序/过期响应防护（reload 已有 reloadGeneration 手法，水合补同类保护），旧响应不得覆盖新状态。④既有 enqueueHydration 在途去重语义保留。回归断言：tools/frontend-regression.mjs 新增 t103-* ≥5 条（快照替换保留正文、同 id 刷新后重新水合、空正文/缺行/失败终态、乱序防护、在途去重），与既有断言冲突项同步更新。coder 开工前先实证 with_content 在各布局的实际取值（layoutNeedsBody），据实修正注释与逻辑，不以猜测为准。 |
| [TASK-104 · 全部已读与标读计数一致性——按后端实际影响数对账、失败不假成功（REQ-003）](<cards/TASK-104.md>) | 待执行 | 修复外部审计探针复现的计数缺口（AUDIT-20261005-core-consistency.md）：①src/store/slices/nav.ts markCurrentViewAllRead 乐观调用 markEntriesRead(ids) 只对已加载条目扣计数，而后端 apply_mark_all_read 处理整个范围（探针：范围 600 条未读、前端加载 1 条，后端成功后界面仍显示 599 条未读）。api.markAllRead 已返回实际标读条数（usize）但前端忽略返回值。修法要求：成功路径以返回条数校验并以 api.feedCounts() 重取全量计数（与 reloadFromBackend 同一计数来源），替代「只按已加载条目推算」；本地已加载条目的已读态允许乐观先行但失败必须回滚（保存原 read 态，catch 恢复并 toast），不允许「计数已扣、状态已改」的假成功；范围语义（feed/folder/starredOnly/sinceMs/layout）不动。②单条标读与计数一致性核查（审计「标读后数字、显示不一致」缺口）：核查 setRead/markEntriesRead 路径对 feedCounts.unread 的扣减与后端口径是否一致——含同文副本去重场景（本地主条目标读、副本计数归属）与跨布局不可见条目；发现不一致即修复并以断言锁定，确认一致的路径在 progress 写明核查结论，不得静默跳过。③t104-* 断言 ≥4 条：全部已读成功后计数=后端口径（600/1 探针场景转断言）、失败回滚不假成功、单条标读计数一致、范围外布局计数不受影响。coder 开工前先用探针/测试实证单条路径现状，据实决定修复面。 |
| [TASK-105 · Rust 状态写入事务化——文章状态与待同步队列同生共死（REQ-002）](<cards/TASK-105.md>) | 待执行 | 统一单条状态写入的事务与同步语义（外部审计 AUDIT-20261005-core-consistency.md 点名的一致性缺口）：src-tauri/src/commands/articles.rs record_read_state / record_star_state 先 db::set_read/set_starred 再 db::enqueue_sync，无外层事务——第二步失败时「本地状态已改但没有待同步记录」，离线期间该变更将永不补推且可能被远端对账覆盖；set_read_bulk 批量路径逐条执行同样可能部分成功。修法要求：①record_read_state / record_star_state 的「状态写入+入队」复合操作包进单一 rusqlite 事务，与 db::mark_all_read_with_enqueue 既有事务口径一致（建议下沉为 db 层函数复用，commands 层只调度）；②set_read_bulk 改为整体单事务（任一 id 失败全部回滚，消除部分成功），语义与单条路径一致并在契约注释载明；③故障注入测试：入队步失败（返回 Err 的测试注入手法）→ 状态不落库，断言「状态变更与队列项同生共死」≥2 条（单条读/藏各一）；④不改命令签名、IPC 形状、push/对账行为；既有 cargo 测试不回退。coder 开工前先盘点 db::set_read/set_starred/enqueue_sync 的全部调用点，确认事务下沉不破坏其他调用方（如对账/导入路径）。 |
| [TASK-029 · 全局排查空壳功能与隐藏 Bug，产出可确认清单（REQ-007）](<cards/TASK-029.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-030 · 修复社交布局正文无限加载（REQ-001）](<cards/TASK-030.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-031 · 定位双向同步缺口：订阅与文章状态回传（REQ-002/003）](<cards/TASK-031.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-032 · 同步状态链路修复：对账防误判（C-1）+ 离线变更一律入队（A-5）](<cards/TASK-032.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-033 · 生产错误回退 mock 修复（P0-2）+ 两处 [object Object] 错误文案（P1-10/P1-11）](<cards/TASK-033.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-034 · 社交/通知卡片翻译按钮接线（P1-7）](<cards/TASK-034.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-035 · 删除订阅接线：远端退订 + 删除墓碑防复活（A-1）](<cards/TASK-035.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-036 · 订阅改名/移动目录接线：edit_subscription 推送远端（A-2）](<cards/TASK-036.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |
| [TASK-037 · 同步接线收尾：push 挂分类（A-3）+ 分类改名/删除防复活（A-4）](<cards/TASK-037.md>) | 已验收 | 当前候选的测试与审查通过；继续已授权任务；所属功能完成后请用户验收 |

另有 65 项记录可在任务总览查看。

**本轮暂缓**：SQL 性能与索引加固（审计 route-M，主控已用 EXPLAIN QUERY PLAN 独立复核）：M-5 列表查询因 COALESCE 表达式排序使 idx_articles_published 对 8 个变体全失效（SCAN + USE TEMP B-TREE FOR ORDER BY）；M-7 sync_queue 零索引致每次标读全表扫；M-9 一次全部已读 = 1 SELECT + 1 UPDATE + 2N 往返且全程持锁；M-14 v6→v7 后置回填在事务外且以 user_version 当完成标记（半途中断永不重试）。

**阻塞**：无已记录阻塞

**下一步**：结合当前任务、验收与实际文件确定下一步

任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。

运行 `python .workflow-kit/scripts/project_workflow.py progress --root .` 生成对话用进度；start 给出实际下一步。
本文件由需求、PROJECT、任务和检查点生成；实际证据与进程仍须核对。
