# FluxReader v0.17.0 重构独立审计

日期：2026-10-07。范围：dev@d9448ee，对照前次审计基线 07a0db9，重点核查报告所述 TASK-106～116 和性能结论。

本审计按用户要求直接开展，没有执行 workflow-kit、修改任务状态或沿用旧授权。没有修改业务源码、用户数据库或同步账户。开始时已有 tools/phase4_measure.mjs 和 tools/phase4_seed.py 两处未提交改动，均保留。审计材料单独保存在 git 忽略的 tmp/audit-20261007/。

## 结论

这是一次有价值的缺陷修复和局部重构，但尚不能认定为“核心一致性重构完成”。建议保留成果，修复本报告前两项高优先级问题，并完成查询、实体缓存与写操作协调的边界重构；不需要更换技术栈，也不需要撤销整个版本。

较扎实的部分是 Rust 状态写入与入队事务化，以及正文水合缺失行、失败态和刷新重触发处理。较薄弱的部分是前端跨动作一致性、筛选分页、缓存失效、回位与同步状态的完整链路。性能报告的测量条件不足以支持“五布局大库无需优化”的广泛结论。

下文 P1 表示优先修复的明显正确性问题，P2 表示应修复的功能或证据缺口。probes.mjs 输出中的 P1～P8 是探针编号，并非严重度。

## 1. [P1] 未读筛选使用 OFFSET，阅读本身会让下一页跳过大量未读文章

位置：src/store/slices/bootstrap.ts 的 loadMoreArticles（约 250～315 行）；src-tauri/src/db/articles.rs 的 list_articles_sql；TASK-110 新增筛选分页路径。

在 WHERE is_read=0 的可变集合上，offset 不能等价于用户已经看过的条目数。1200 篇未读，加载前 500 篇，把这 500 篇标读，数据库查询集合只剩 700 篇。下一页仍 OFFSET 500，于是跳过剩余集合的前 500 篇，只取最后 200 篇，并宣告 exhausted=true。

探针结果：loaded=700、missingUnread=500、exhausted=true。探针驱动实际 store，假后端按 only_unread + offset 的真实查询语义筛选。默认“打开即标读”即可逐步触发同一问题；批量操作使它更明显。收藏视图取消收藏亦同理。

报告把删除型漂移描述为“可能跳个位数行”，低估了主操作造成的集合收缩。追加去重只能防重复，不能找回跳过的行。

建议：以稳定排序键做 keyset 分页，例如规范化发布时间 + 唯一 ID，列表、定位与续拉共用排序规则；或者固定查询快照的 ID 序列。不要仅以“offset 按拉取行数推进”作为正确性证明。验收必须包含阅读/取消收藏过程中连续翻页，而不是只有静态分页与插入去重。

## 2. [P1] 文章级共享版本把收藏操作当成读状态接管，导致失败回滚失效

位置：src/store/internals.ts 的 entryMutationVersion / flipEntryFlag；src/store/slices/nav.ts 的 markCurrentViewAllRead 失败分支（约 357～379 行）。TASK-107 新增版本守卫。

复现：未读文章 → 全部已读请求在途 → 用户收藏该文章 → 全部已读失败。收藏会 bump 同一个 article 版本，读状态回滚因此被跳过。收藏并没有接管 isRead。

探针结果：应为 isRead=false、unread=1，实际 isRead=true、isStarred=true、unread=0。

建议：至少使用 articleId × field 的操作版本；更完整的方案是已确认状态 + 每字段待确认意图，由操作 ID 确认或撤销。后端快照也不能只凭“来自后端”就无条件覆盖所有在途意图。

相邻缺口：optimisticEntryFlagToggle 仍使用值比较，而 markEntriesReadBulk 和打开即标读失败仍主要只提示、不回滚。这些是留存问题，不应与新增回归混淆，但说明“所有乐观写入都有统一版本回滚”尚未成立。

## 3. [P2] 快照继承没有失效机制，清理 AI 缓存后旧产物会被重新保留

位置：src/store/internals.ts:302 mergeSnapshotEntries，尤其 translatedContent / aiSummary 的 || 回退；src/components/settings/CacheCleanupSection.tsx:19；src-tauri/src/db/articles.rs:512。

清理 AI 缓存真实路径会把数据库 ai_summary / translated_content 设为 NULL，再 reloadFromBackend。列表中的空摘要被转换为 ''，merge 又通过 a.aiSummary || prev.aiSummary 恢复旧值，译文也会继承。当前界面显示的缓存与数据库已经分离。

探针结果：后端已返回空产物，UI 仍为 old summary / old translation。另一个探针显示列表 snippet 已更新时，正文仍为 old body，entryNeedsHydration=false，没有正文重取。

正文无条件保留能修复加载死区，但没有内容版本或显式失效事件，会造成内容长期陈旧。NULL/未请求字段与“已明确清空”的字段也没有区分。

建议：单独的正文/AI 实体缓存，增加内容版本、失效状态；清理操作主动失效相关 ID。对于列表真实返回的 ai_summary，尊重 NULL 的删除语义；对未返回字段使用明确的 omitted 标识或分离 DTO，避免空字符串同时承担缺省和删除两种意义。

## 4. [P2] 请求来源正确不代表响应足够新：查询与计数仍可被迟到结果覆盖

位置：src/store/slices/bootstrap.ts 的 reloadFilteredEntries（约 348～398 行）；src/store/slices/nav.ts:323～338 reconcileCounts。

两个独立探针：

- 同 scope/view/sort 发起两次筛选请求，新请求先返回 ID 2，旧请求后返回 ID 1，最终列表回到 ID 1。当前守卫只比较查询参数，不能识别同一查询的旧版本，也不能处理 A→B→A。
- 全部已读成功后请求计数，计数快照为 0；随后用户把文章改回未读；旧计数响应返回并整体替换，最终文章未读但未读数为 0。

第一个窗口原本就存在，QueryScope 本次只是将谓词收口，没有消除它；第二个窗口属于新计数对账链路。

建议：每个查询实例维护 requestId/generation，所有首屏、续页、刷新、导航共享同一套过期判断；计数使用数据库 revision 或请求版本与待确认操作合并。对旧响应不能只比较“查询参数相同”。

## 5. [P2] 后台刷新和切换返回的保位只对保留窗口内的文章有效

位置：src/store/slices/bootstrap.ts:139 刷新固定 offset=0 / limit=500；src/components/Timeline.tsx:273；src/components/timelineAnchor.ts:129。

加载 1000 篇、读到第 750 篇后后台刷新，实际列表缩回前 500 篇，anchorRestoreIndex 返回 null。程序此时没有原文章可定位，无法兑现“后台刷新原位保持”。这不是文章被删除，而是刷新策略主动丢掉了阅读窗口。

另一个组合问题是缓存返回先定位，然后导航仍无条件后台重拉第一页；若返回锚在第二页，重拉会再次删除锚。只订阅 switchRestoreNonce 的 effect 不能在这次重拉后解决丢失。

现有锚只保存 ID，不保存卡片内像素偏移，长社交正文也只能恢复卡片顶部。250ms 节流没有尾沿补记，停滚后立即切换还可能记到较早的位置。

建议：刷新实体变化并保留当前已加载窗口；确需重建时围绕 anchor ID 查询窗口，保存 intra-item offset。缓存若截断，应记录窗口边界，不能只保留原 loadedCount 并假定已移除的条目仍可达。

## 6. [P2] 同步四态缺少队列变化通知，日常操作时仍可能显示“后端已同步”

位置：src/store/slices/bootstrap.ts:206；src/lib/syncPill.ts:21；src-tauri/src/sync/push.rs:177。

统计刷新集中在 reloadFromBackend，设置页另存一份局部统计。普通标读/收藏入队和即时推送出队，没有相应刷新 store 的事件。探针：本地写入后模拟队列有 1 项，sync_queue_stats 请求数=0，store waiting=0，pill 仍为“后端已同步”。

此外，认证/端点构建失败在 build_client 阶段就提前返回，根本未进入 exec_push，新增 attempts/last_error 不会记录这类失败。四态文案的真值表测试不能证明真实状态会到达该文案。

建议：本地事务提交、推送确认/失败后发轻量 SyncStatusChanged 事件；前端按同一状态源更新，必要时在同步活跃期短轮询。区分“未配置”“认证失败”“网络失败”，保留 lastPushSuccess/lastSyncSuccess 的真实语义，不能用队列空或拉取游标代替远端确认。

## 7. [P2] 性能证据不足，已提交工具与报告使用的修正版本也不一致

位置：tools/phase4_measure.mjs:59～88；tmp/phase4/measure-50k.json；.workflow-kit/docs/PHASE4-MEASUREMENT-20261007.md。

具体问题：

1. 切换完成条件是 document.querySelectorAll('[data-card-index]').length>0。旧卡片尚未换掉时就成立，63～78ms 可能主要是 50ms 轮询与自动化开销，不能称为新查询完成的端到端耗时。
2. 搜索在打开面板并等待 500ms 后才计时，报告说包含打开浮层并不准确；完成判据搜索整个 document.body，可能命中背景列表已有文本，也没有断言结果属于新查询；超时仍输出一个普通耗时数字。
3. 50k 原始数据中社交/画廊/播客/通知 cards 都为 0。20k social 数据是短文本段落，不包含真实图片、音频和解码压力。
4. JS heap 不等于 Rust + WebView2 + 图片解码的进程内存。数据库总量 20k→50k、前端仍只加载少量页面的对比，也不能证明长会话缓存有界。当前 entries 持续追加，entryMutationVersion 也无淘汰。
5. 程序化滚动仅 3 秒，不能外推为整库滚动、所有布局或真实输入延迟“零卡顿”。独立 SQL 查询没有制造后台同步持锁竞争，不能排除锁等待。
6. HEAD 提交的工具把 findPageTarget 返回的 target 对象直接传给 connect，并对 Runtime.evaluate 响应多解了一层 result，和 t059_cdp.mjs 契约不符。工作区未提交改动修复了这两处，也修了种子时间戳。这些改动先于本审计，不能算本审计修改，但报告按已提交工具复跑会遇到问题。

合理结论应为：受测条件下的短时文章/社交文本场景没有观察到明显瓶颈；尚无证据要求立刻替换 SQLite 单连接。不能进一步推导为“全阶段性能目标已满足、无需优化”。

建议：以 query/request revision + 新结果 ID + 渲染稳定作为计时终点；记录实际加载实体数，测首屏/深页/8 次视图往返/长会话；分别测 RSS/JS 堆/解码内存/锁等待 p95/p99，加入真实媒体与同步并发负载。先保证测量正确，再决定是否优化。

## 8. [P2] 协议限制与客户端未实现能力混淆，兼容矩阵还不是验收矩阵

位置：docs/sync-compat-matrix.md:30 附近；src-tauri/src/sync/conflict_policy.rs；src-tauri/src/fever.rs。

矩阵把 Fever 写成“无全量历史端点”，只列最近 50 篇 + 未读/收藏补齐。此次直接核查 Miniflux 当前服务端源码，items&max_id 支持向更旧条目翻页，并说明重复直到返回空数组。不能将当前客户端没有实现历史回溯说成协议做不到。历史是否仍被服务端保留另当别论。

来源：https://github.com/miniflux/v2/blob/main/internal/fever/handler.go ，约 227～267 行（2026-10-07 读取）。这证明当前 Miniflux 的能力，不等于所有部署版本均已验收。

同样，GR read-wins 是客户端选择的冲突策略，不是 Google Reader 协议必然要求。将原行为抽成常量使其可审计，但没有解决真实服务端游标差异、远端重新标未读的收敛和同文条目身份问题。文档保留这些限制是诚实的；将其算作相关核心重构已完成则不准确。

建议：将矩阵分成“协议/服务端支持”“本客户端已实现”“指定服务端版本已验证”三列；协议适配器报告能力，同步引擎消费能力。冲突政策返回可测试决策，数据库应用层返回 Result，避免 apply_*_by_policy 中 unwrap_or(0) 继续把写失败当作没有变化。

## 架构与代码质量判断

| 工作 | 判断 |
| --- | --- |
| Rust 单条/批量状态与入队事务化 | 方向正确，封装适度。触发器在真实入队 INSERT 注入失败、批量末条失败验证全回滚，比复制实现的测试有效。 |
| 水合缺行/失败/刷新重触发 | 修复了前次审计的明确死区；应保留。 |
| 文章实体与视图分离 | 尚未完成。mergeSnapshotEntries 仍是复制/继承视图行，正文、AI、读状态仍随多份快照传播。 |
| 显式水合状态机 | 部分完成。状态仍分布在 content、hydrated、hydratedIds、hydrationErrors、模块级 inFlight，且不存在状态与失败共用字符串错误。能工作，但不是单一判别联合状态机。 |
| QueryScope | 参数工厂与守卫命名的集中化，不是完整查询值对象；pageKey、viewKey、filterKey 的维度仍不一致。模块内内存键没有必须逐字兼容的外部契约，机械锁定字符串会限制必要改进。 |
| 协议政策模块 | 显式化有价值，但一个枚举只有一个变体、政策函数直接写 DB 并吞错，抽象收益有限。应该分开决策与持久化。 |
| 同文内容/各源条目建模 | 未实现，报告已披露。保留旧传播策略是阶段性兼容决定，不是这一重构块完成。 |
| 注释和测试 | 长篇历史注释、任务号与当前规则混杂，部分注释仍保留“筛选全集”等过期描述；约 6000 行集中测试混有源码字符串检查。应保留规则与不变量，历史说明移到设计文档，测试按领域拆分。 |

更好的目标结构，无需引入另一套框架：

- articlesById：轻量文章实体，读状态有 confirmed value 与待确认操作。
- bodyById：正文/AI 缓存，使用显式状态和 contentRevision；可独立失效及按内存预算淘汰。
- queries[完整 QueryKey]：ID 页、keyset cursor、request generation、exhausted、窗口边界；不复制正文。
- readingContext[QueryKey]：anchor ID + 卡片内像素偏移；刷新围绕当前窗口进行。
- mutations[(articleId, field)]：操作序列与确认/失败处理；单条和批量共用一个协调器。
- SyncEngine：协议能力与游标、冲突决策、事务应用、同步状态通知分层；同文关联与条目身份后续独立建模。

采用 TanStack Query 可减少查询生命周期的手工工作，但不是必要条件，也不会自动解决 mutable OFFSET、冲突策略和实体身份。继续使用 Zustand 完全可行，前提是移除同类状态的多个所有者。

## 验证结果与限制

- TypeScript 项目检查通过。
- 现有前端回归 655/655 通过，日志见 regression.log。
- oxlint 退出码 0；工作区原有未提交性能脚本有一处未使用 send 警告。
- 8 个补充状态探针均完成，输出见 probe-results.txt。它们运行实际 store/辅助函数，以可控 IPC 替身验证时序，不冒充真实桌面 E2E 或真实同步服务验收。
- Rust 定向事务测试尝试被缺少 MSVC link.exe 阻断；没有独立确认远端 CI 结果，不把报告中的 CI 全绿当成本次实测。
- 没有运行会操作真实应用数据库的性能种子脚本，也没有改动现有两份未提交工具文件。

复跑命令（项目根目录，Node 需支持现有测试使用的 TypeScript 直接导入；本次 Node 24.19.0）：

```powershell
node node_modules/typescript/bin/tsc -p tsconfig.test.json
node --loader ./tools/test-loader.mjs ./tools/frontend-regression.mjs
node --loader ./tools/test-loader.mjs ./tmp/audit-20261007/probes.mjs
```

补充探针是诊断输出脚本，执行成功表示场景跑完，不表示被测行为正确；正确性差异在 expected 字段与本报告说明中。探针不访问真实同步后端或磁盘数据库。

## 建议的收口顺序

1. 先修复可变筛选分页、跨字段回滚和请求/计数过期覆盖，并把复现转成真实行为回归。
2. 完成实体/正文/查询缓存分离与显式失效，再统一所有读状态写入口。
3. 在真实挂载的组件测试中验证深页刷新、切换返回、清理 AI 缓存、同步统计更新；纯函数和 SSR 无法覆盖 effect 生命周期。
4. 校正协议能力矩阵，补指定 FreshRSS/Miniflux 版本的双端同步验证。
5. 修正并提交测量工具，补深页/媒体/同步并发数据，然后决定数据库与内存优化。

总体评价：成果应保留，完成度需要下调。最重要的下一步是减少状态所有者并统一请求与操作的生命周期，而不是继续围绕同一个全局快照模型叠加更多守卫和补偿分支。
