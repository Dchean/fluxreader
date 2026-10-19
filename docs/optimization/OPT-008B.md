# OPT-008B：内容实体与各订阅源条目分离

Status: verified-ci
父卡OPT008主体；前置OPT006/007/008A。保留五布局、协议ID和现有离线状态意图，不用仅修URL参数宣称完成。

## 数据所有权

文章条目是订阅源/远端ID的独立实体：每条必须保有自己的feed_id、guid/remote_id、读/藏状态、发布时间与队列引用；同URL不把另一源条目吞掉。内容实体可共享规范URL、HTML/body_text、全文提取与AI产物，条目指向content_id。第一步可对已有表追加content_id+新contents表并保持外部ArticleRow/IPC形状，由JOIN适配，后续迁移避免长期两份正文真值；不把“记录remote_dup_ids CSV”继续当实体模型。

现有历史丢失的副本只有remote_dup_ids的有限信息，没有feed/rawURL时不可凭空构造完整条目；在账号重拉后按真实服务器补齐映射。迁移保留旧id与状态/队列，以真实remoteID优先匹配，跨feed只匹配content而不合并entry。

## 产品语义

默认不改变用户已经接受的读状态跨源传播政策：“标读广播同内容副本、取消标读和收藏只当前绑定”必须明确由策略函数/选择控制，而不是由数据库把条目吞掉来实现。布局/分类中条目可达，画廊/文章同文不能因不同source丢失。用户smartDedup选择控制显示聚合/后台共享内容，不永久删除订阅条目身份；计数、分页、search定位、markAll范围按明确条目集合一致。

内容相同不能仅从危险启发式推断（规范URL只是候选，协议精确entry key优先）；不同t/s等参数已由008A保留。全文/AI实体缓存key应对content_id/版本具有确定映射，不能同文条目一份旧HTML一份新AI。

## 必测迁移与跨源反例

- 两源同URL但不同remoteID：都能在各自布局/分类查到，读/藏当前/广播政策各自可解释，pending精确回传，不覆盖同一个remote_id绑定。
- 一个source删除/断开，不删除仍被另一个条目引用的content或队列；删除最后引用按缓存清理策略处理。
- 同一个feed更新guid但同entryURL仍按现有稳定匹配不复制，跨feed则独立entry。
- old库迁移保存全部article id/状态/队列/正文/AI；触发失败回滚，重开可重试，不改旧迁移。
- 五布局分页/计数/全文搜索/详情/backfill/全文/AI引用全链一致；视图去重不能跳分页或隐藏当前源。

## 预计写集

db/migrations/articles/sync_map/feeds与new contents.rs；sync/entries/push/conflict_policy/pull；frontend api/types/selectors/bodyCache及窗口查询；严格同步/去重/迁移/前端回归；相关核心架构Note原位更新或新模型决定互链。正式执行前主控根据OPT007已稳定接口切成最多两个独立可验证阶段，不开放无界全库重写。

## 退出标准

schema与实际读写、状态策略、UI可达性都使用新所有权，不是只有新表无人消费；所有旧迁移可升级、缺失历史可解释；全门禁与独立review、devCI通过。父卡只有此主体与008A全部验收后才可关闭。

## 分阶段执行（2026-10-12）

基线 c25d367，前置007/011已完成本地验收并推送；CI 38218122226 已成功。该结果只覆盖基线，不代表后续008B实现已经验收。

1. **条目身份与实际消费者**：优先以精确协议 entry ID 匹配，同 URL 兜底只在同订阅源内选行，跨源必须保留独立条目。智能去重从“入库丢弃身份”转为查询呈现策略；列表、计数、搜索、分页和批量操作使用同一可解释的条目集合。按读广播/取消读与收藏只当前条目的既有政策处理队列与版本，不再以 CSV 副本充当唯一身份。旧 CSV 仅作有限兼容线索，远端真实重拉补全，不编造源归属。先用现有 articles 正文列保留全部数据，不建立空 contents 表；本阶段独立审查，不代表正文实体已分离。
2. **正文实体单一所有权**：追加迁移建立 contents 与 articles.content_id，实际详情/列表摘要/搜索/回填/全文/AI/删除消费者统一接入，清除双正文真值；安全共享依据及内容修订失效明确。保留旧条目ID/队列/读藏，迁移失败原子回滚，删除最后引用才清理内容。阶段二实现后方可关闭008B/008。

第一阶段写集：db/articles、sync_map、feeds及必要db.rs导出；sync/entries、push、conflict_policy、两协议pull；必要前端查询/呈现/计数与store派生消费者（不改AI配置协调器）；对应Rust/前端行为回归、既有URL/查询/冲突政策Note。若必须新增持久身份/显示聚合字段，先报告数据契约和迁移理由，不提前堆叠第二阶段空schema。

执行者同一负责人调查、实现、验证与返工。每阶段最后一次验证后停止写入，交初审和主控复审；遇到无法兼容的真实产品语义冲突时提交具体决策点，不能默默关闭smartDedup、取消read广播、禁用布局或删回归。临时交付记录在tmp/optimization-20261008/OPT-008B/RESULT.md，长期取舍见对应新模型Note。

## 执行恢复（2026-10-12）

首轮已产生条目身份/查询集合及相关回归改动，前端聚合计数接线尚在进行，未交付、未初审。16:17 的CLI终态为 error_during_execution，明确429 quota耗尽（不是普通观察超时）；原进程已退出，保留原会话与全部工作区改动，按授权切 custom-local:deepseek-v4.1-flash / max 继续，不把外层退出码0计作成功。原始记录位于 tmp/optimization-20261008/OPT-008B-phase1-20261012，续跑位于 OPT-008B-phase1-fallback-20261012。

第一阶段实际消费者需要新增scope_counts只读计数IPC，允许commands/articles.rs、lib.rs注册及前端api/types/侧栏的必要接线；实现者须说明聚合集合口径、兼容边界和真实消费证据。此扩展不授权提前增加正文schema、修改AI配置协调器或减少原验收范围。当前产品WIP未纳入任何提交级CI；2c7dcf5的CI 38220831888成功仅覆盖之前产品基线与文档。

## 第一阶段交付与复核（2026-10-12）

备用模型续跑终态 success、is_error=false，执行者停止写入；报告前端943/943、组件18/18、Rust全测试/fmt/clippy通过。主控已核对完整Rust测试末尾与格式/lint/build原始命令结果、37篇Note结构/链接检查通过；这些不代替行为验收。独立只读初审正在检查冻结diff（OPT-008B-phase1-review-20261012），未给出结论。

主控定点补测发现真实缺口：隔离生产DB API中3个同URL、不同feed的直连未读条目，smartDedup开启后调用mark_all_read_with_enqueue只改1行、只入队1行，另外2行仍未读。未发现“标读行与队列不一致”的猜测，二者当前都是1；实际问题是代表变已读后另一个副本成为未读代表，离线/纯直连无法依赖远端同步收敛，“全部已读”并未清空该范围。探针源码保存在tmp/optimization-20261008/OPT-008B/opt008b_controller_probe.rs（原临时test target已移回tmp，不进主线），最终断言read=3实际为1，cargo退出101。此问题待初审汇总后交回原实现者，不因其自报全绿或“显示集合一致”文案豁免。

另一项布局切回猜测用真实store试验未复现持久错计：article 7→social 3→article回读7，切回确实重新发scope_counts；仍需检查在途响应与失败边界，不能把这一条通过扩写为所有布局竞态已覆盖。父卡与正文实体第二阶段继续未完成。

## R1返工裁决

独立只读CodeBuddy初审NEEDS_CHANGES（1个P1、4个P2）；主控复核接受markAll漏副本、非线性聚合计数、迟到计数跨布局污染、快照无布局来源及设置持久化/重查顺序缺口。迟到计数已用真实store确定性复现：markAll发出article计数请求→切social并落地3→放行旧article回包，当前social变2（controller-late-counts.mjs退出1）。设置IPC的真实线上乱序未实机复现，不能写成已发生；源码未等待提交及未处理失败已确认，要求受控交错回归。

主控额外发现搜索去重在匹配词之前选全局代表，可能藏掉同URL组中唯一匹配的副本；另要求新协议entry不能继续被同feed URL兜底吞成CSV、read广播目标读取失败不能当空并清队、同轮read/unread顺序政策与未知feed信任边界有正式回归。这些仍在第一阶段身份/实际消费者契约内，不接受“旧路径已有”豁免。

R1已交原实现会话（OPT-008B-phase1-R1-20261012），保持备用模型max；主控不同时修改产品。独立reviewer保留用于复审。范围调整允许必要计数来源类型、动作确认和设置顺序接线，但不开放AI配置协调器或第二阶段schema。当前未提交产品仍未验收，不开始下一阶段。

## R1复审与R2（2026-10-12）

R1在两次max-turns终态后原会话续跑，最终success、is_error=false并停写，交付947/947前端、18/18组件、Rust lib276与全目标测试/格式/clippy通过。原markAll漏副本、迟到跨布局计数、搜索全局代表、同源不同remoteID、push读取失败等已有正式反例闭环；不能把R1历史失败当当前仍未修，也不能把这批全绿当整卡验收。

同一独立reviewer给出4项P2残余，主控核对后返工：默认去重关闭时后端读闭包已改兄弟行、前端条目与计数却只改单行；旧reload与新计数对账的双向覆盖仍不对等；设置确认基线和跨笔reload丢失；Fever轮终对账撤销同轮已接受读广播。

设置两条已由主控真实store+独立持久态复现（原始探针在tmp/optimization-20261008/OPT-008B）：
- controller-settings-r1.mjs：A切去重true成功、B改字号失败，DB去重true/UI却false，退出1。
- controller-settings-success-r1.mjs：A切去重true、B改字号全成功，但reload计数为0（应至少1），脚本退出1。

R2交回原会话（OPT-008B-phase1-R2-20261012）。同轮已被字段守卫接受的远端read信号优先不能被Fever末尾立即撤销；无本轮信号时仍保留Fever双向unread权威，历史本地已读不能永久挡掉后续取消读。新增必要的受影响条目回执/回读与计数确认可以实施，必须覆盖真实消费者与前沿保护；不许为通过旧线性计数mock而关闭后端读闭包。搜索跨布局还需实际组件打开/返回/续页验证，不能以数组存在自证。阶段二与父卡继续未完成。

R2续跑恢复：首段达到max-turns后正常停止，但原会话再次续跑被provider立即以HTTP400“Invalid assistant message: content or tool_calls must be set”拒绝，无产品执行。已确认两个进程均终止，保留全部WIP和有效日志，以干净CodeBuddy会话接管（OPT-008B-phase1-R2-recover-20261012）；不修改CLI历史或全局配置、不重复派两个写入者，模型保持授权备用max。当前新增counts_snapshot/读状态回执接线仍未验收，最近前端回归有3项失败，已作为接管输入，不沿用R1全绿。

## R2交付复核与R3（2026-10-12）

R2最终由OPT-008B-phase1-R2-deliver-20261012交付success并停写，968/968前端、20/20组件、Rust全门禁通过；新增Fever两排列/集合失败恢复/真实网络推送确认及direct刷新身份回归，搜索实际Reader DOM与续页可达性取证。主控已核对最终前端汇总与执行包装、Note37篇格式/链接通过。180轮有界派发用于避免90轮截断后反复触发provider坏历史，不改用户模型/强度或全局配置。

独立R2复审仍NEEDS_CHANGES：读回执用全局序号一刀切跳过未冲突兄弟UI，却无条件扣源计数；reload只检查“当前无在途”而遗漏读取后已完成写入。主控接受这两项并要求计数/条目共同闭环，不能以旧路径已有而豁免视觉回退。

主控另实跑settings两项反例，不能接受初审将配置入口降为P3：
- controller-settings-config-r2.mjs：本地成功写→外部配置应用dedup=true并真实读回→下一次写失败，DB=true/UI=false（退出1）。
- controller-settings-stale-r2.mjs：bootstrap旧false读取挂起→本地true成功→放行旧读，DB=true/UI=false（脚本退出1）。

R3已交刚成功交付的原会话（OPT-008B-phase1-R3-20261012）：逐条目/字段回执确认，读快照与已提交写前沿，app_settings真实配置apply/upload与读回协调，补正式反例。额外允许必要设置协调模块及ConfigSyncSection接线，保留AI协调器内核，不提前做镜像功能。搜索续页证据改为“旧响应代际丢弃后自动重试成功”，需测试区分两次请求；reader-prose外链偶发失败未宣称修复。当前仍rework，不提交产品或开始正文实体第二阶段。

## R3交付待复审

实现者success停写，报告991/991前端、22/22组件与Rust门禁通过；同一独立reviewer正在复审（OPT-008B-phase1-R3-review-20261012）。主控新增真实store探针controller-apply-old-bootstrap-r3.mjs：bootstrap旧false读取挂起→真实runAppSettingsApplyBoundary内模拟配置持久化true并readBackAppliedAppSettings接受→旧bootstrap放行；输出afterApply=true、persistedDedup=true、afterOldBootstrap=false，退出1。应用边界未使旧读取凭据过期，仍有UI/确认态回退风险；未验收，不把执行者“可接受残余”视作主控已同意。待独立结果汇总后定向返工。

R3独立结论为PASS，但主控不采纳最终验收：上述应用后旧读取反例实际失败，review漏审应用代次只观察本地updateSettings写序号。另不接受“IPC默认FIFO”替代读取前pending意图保护的明确契约。R4已交原实现会话（OPT-008B-phase1-R4-20261012），限定设置读取/应用代次与已有字段凭据的最小硬化；补独立提交/回包时序、应用期间多点击及回读失败后的全量载荷覆盖反例。后两类目前是待验证风险，不冒称已实跑。Rust若无改动复用R3相同版本门禁，前端改动须按最终版本验证；身份/Fever已核销路径不重做审计。

## R4复审与R5（2026-10-12）

R4终态success并停写，最终前端1016/1016、组件22/22，lint/build及Rust门禁通过；主控核对前端最终日志与失败修正记录，Note37篇格式/链接检查通过。原应用后旧bootstrap回退已关闭，设置读取前沿与文章在途字段保护已补回归；不能把这些历史失败继续列为当前未修。

独立R4初审NEEDS_CHANGES（2项P2）；主控用真实store与独立持久态确认两项，并推翻另一项“不可能插入”的判断。三条探针均退出1，位于tmp/optimization-20261008/OPT-008B：
- controller-remote-base-r4.mjs：导入行高200、旧本地180；窗口内A字号成功、B行高失败，A用B.prev把远端行高写回180，最终UI/DB都错误地180。相等不等于正确。
- controller-postgate-unconfirmed-r4.mjs：导入去重true但回读失败；门关闭后仅改字号，非门控分支未校准就把未触及的去重覆盖false。
- controller-postgate-click-r4.mjs：窗口内A/B排队，关门后A仍等网络响应；此时C点击去重true，A结束后B构造载荷偷带C，C随后失败但库中已true。初审只考虑单笔门释放的连续微任务，遗漏前一笔网络等待，主控不采纳其不可达结论。

R5交原实现会话（OPT-008B-phase1-R5-20261012），要求在既有设置协调范围内收敛“确认基线+每笔patch+待写意图”，不继续给门内/门外路径叠补丁；同键先后意图、失败回滚、校准中新点击、全量载荷保护须一起保持。初审建议简单优先confirmed也不能直接照做，否则会把A18/B16中的A错误降回基线16。既有身份/Fever/Rust子路径不重审，无改动Rust复用同版本门禁；前端最终版本重新验收。未提交产品、未开始正文实体第二阶段，主卡仍12完成/6未完成。

## R5复审与R6（2026-10-12）

R5统一设置载荷为“确认基线+本笔patch”，待写意图投影UI，已关闭R4三条主控反例；报告1039/1039前端、22/22组件、lint/build与8探针通过，Rust无改动复用R4。Note37篇格式/链接检查通过。独立R5复审NEEDS_CHANGES（1项P2），主控实跑并确认：空基线同键18/20两笔均失败，UI最终18而不是16（controller-empty-base-r5.mjs，退出1），后一笔prev继承前一笔失败乐观值。

主控另发现R5新单飞读取凭据不绑定原请求写前沿：已有确认基线false→旧bootstrap读取挂起→写true成功→第二次bootstrap加入同一旧读取→放行后DBtrue/UIfalse（controller-shared-old-read-r5.mjs，退出1）。这是实际store导出契约反例；生产当前主要是App启动调用，未冒称真实界面操作已复现双bootstrap。初审仅核查base未知时的校准join就概括单飞正确，范围不足；R6须绑定原请求写/应用代次或取消不安全共享，不能为旧测试只有一个release而更改产品语义。

R6已交原会话（OPT-008B-phase1-R6-20261012），保留base+patch方向，最小修复空/partial基线失败下限和共享读取新鲜性。相关反例进入正式回归，原8探针保持，前端最终门禁；Rust若未变不重复执行。尚未提交产品或开始第二阶段。

## 第一阶段R6本地验收（2026-10-12）

R6成功停写，独立只读初审PASS；主控复查失败下限传播、读取凭据与最终原始门禁，接受本阶段交付。新增主控controller-triple-outcomes-r6.mjs真实store探针：空/partial/完整基线各8种三笔同键成功失败组合，共24例全部通过，补足初审仅源码推导的三笔混合边界。原10条探针由实现者最终版本执行均通过，未无意义重跑；前端1047/1047、组件22/22、lint/build通过，Rust未变复用R4全套通过证据。git diff --check与37篇Note树/格式、0篇归档校验通过。

当前第一阶段验收覆盖条目身份、查询显示集合、读闭包回执、计数、搜索实际入口及相关设置协调；不是正文实体已分离。初审登记的双bootstrap读取前沿边界未冒称生产UI复现；应用后的设置回读被前置AI回读长时间阻塞时，门仍关闭且设置尚未确认，未宣称全部异常网络均已原生验收。持续读取失败会可见拒绝设置写而不覆盖未知基线。openExternal偶发未修、真实服务端live未跑继续保留。下一步精确提交并验证CI，成功后才交同负责人第二阶段。

## 第一阶段CI与第二阶段派发

产品提交3fdec3ba5e51881f2bef41f35675cd5230b94e0a已推送dev，[CI 38259218755](https://github.com/Dchean/fluxreader/actions/runs/38259218755) completed/success。第一阶段交付完整通过，不合main、不关闭008B/008。

第二阶段交同一实现负责人（OPT-008B-phase2-20261012，备用模型max），按下列边界实现内容单一所有权，先定稿原proposed身份Note再编码。独立交付记录RESULT-phase2.md留在tmp，不覆盖第一阶段最终证据。实现、定向收口、独立复审与本地门禁已完成；提交级CI通过前不关闭正文分离阶段。

## 第二阶段R3A本地验收（2026-10-13）

R3首轮因达到max-turns未形成交付记录；按任务拆分为R3A定向收尾与最终门禁后，当前WIP完成以下收口：

- `contents` 写入、条目重指、全文提取提交与回填统一使用可嵌套的SAVEPOINT；三条故障注入回归确认失败回滚后无孤儿、旧引用与FTS不变，清障后可重试。
- `contents.id` 使用`AUTOINCREMENT`，删除最大内容后不复用ID；前端额外保存`feedId|title`条目锚，防条目删除重建且内容版本对巧合相等时复用旧正文。
- 全文提取成功回传实际`contentId/contentRevision`并更新bodyCache；Reader在活动文章正文因修订失效且Timeline卡片未挂载时主动重水合，在途旧响应仍被丢弃。
- 历史Readability异常行保持受保护来源并修复同源纯文本；`delete_folder`与`purge_remote_data`各有共享内容引用生命周期回归；五布局真实挂载测试覆盖修订失效、重水合和Reader渲染。

独立只读R3A复审结论为PASS，无P1/P2。修复rustfmt后，最终本地门禁真实退出码全部为0：`cargo fmt --check`、`cargo clippy --all-targets -D warnings`、完整`cargo test`（573 passed / 0 failed / 7 ignored）、Node22前端1059/1059、组件24/24、lint、build、Note校验与`git diff --check`。提交`81567260d49e367f8964cd07ec8ddbc5c2bf112d`的Windows CI [38381181417](https://github.com/Dchean/fluxreader/actions/runs/38381181417) completed/success。live账号测试、真实用户库/账号、main合并仍未执行；阶段状态更新为`verified-ci`，总收口仍未完成。

## 第二阶段接线与验收边界

> **历史边界（2026-10-12 计划现场）**：本节写的「现行版本 21」「FTS 外部内容表」是
> 当时源码状态，不是现状。当前迁移末版为 **v30**（v22 建 `contents`，v29 删
> `articles_fts` 改建 `articles_trigram_idx`）；搜索权威是 **LIKE**，trigram 索引
> 只是**可重建候选索引**。此后的现状与收口口径见文末「OPT-008C 收口（2026-10-19）」。

主控按当前源码确认的入口：migrations.rs现行版本21，FTS外部内容表和触发器依赖articles正文列；articles.rs同时负责列表摘要、LIKE/FTS搜索、入库、AI落盘和缓存清理；sync_map.rs仍有正文/全文/AI输入读取；commands/settings.rs负责异步全文提取；commands/ai.rs在网络生成后按article_id写产物。前端bodyCache以条目ID存实体，正文失效部分依赖snippet变化，不能仅加JOIN就声称修订已打通。

执行者第一阶段验收后继续负责第二阶段，实现前在已有proposed身份Note定稿以下契约，不为这些事实再建重复提案：

1. **迁移与唯一正文**：只追加迁移，保留旧条目ID、feed/guid/remote身份、读藏版本、队列与全部已有正文/AI数据。旧内容不同或AI产物冲突时不得为共享而丢弃其中一份；最终生产读写不得继续依赖旧正文列成为另一真值。是否重建表/删除列由迁移可验证性决定，但兼容列若保留须说明非权威用途，不用双向触发器维持双正文。
2. **候选与等价**：规范URL只是候选；空URL/空正文不能把不相关条目聚成一个内容。相同候选但不同HTML/纯文本/提取来源要有确定的分叉边界；共享必须实际发生且有反例，不把“一条目一contents行”永久化冒充内容去重。条目标题/作者等仍可不同，依赖标题的AI产物不得因正文同一就盲目复用；模型/提示词签名的全面升级由013承接，008B已引入的共享不能先制造误命中。
3. **修订与异步写**：RSS/远端更新不得覆盖已有全文提取的既定保护；正文真正变化后明确content身份/修订和AI失效。网络期间不持DB锁，全文与AI迟到提交不得写到已删除/分叉/替换的内容；清理后旧任务不能复活已清产物。此处交付稳定内容修订与提交前提接口，013再扩展生成签名/分段恢复，不重复造两套版本。
4. **实际消费者**：详情/批量水合、列表snippet、LIKE和FTS、五布局窗口与搜索定位、全文输入输出、AI输入输出、入库回填全部使用新所有权。FTS是可重建索引而非正文权威；共享内容变化须正确更新每个引用条目的索引，不能只有代表可搜到。前端须消费实际content身份/修订，使“snippet未变但正文后半段更新”和共享兄弟内容变更也能失效；请求代次保护和Reader实际渲染不能退化。
5. **引用生命周期**：源删除、分类级联、账号断开、缓存清理均按引用处理，最后引用删除才回收内容与派生记录。AI按年龄清理若只命中部分引用，须明确保持原条目清理范围的分叉策略，不能意外清掉年轻兄弟或留下已承诺清理的旧产物；清理回执数量/UI文案与实际对象一致。

必测：真实旧版本21库含不同正文/不同AI的同URL条目→迁移无损且外键/队列不变；注入迁移中途失败→版本与数据整体回滚、重开可重试；可等价共享/不同内容不共享/单引用修订分叉；删除一个源与删除最后引用；正文后半变更但snippet相同；FTS新词可搜旧词消失、每个源均可达；全文/AI在途修订与清除后的迟到提交拒绝；五布局正文和读藏仍独立。使用隔离库与本地HTTP夹具，不访问用户库或付费模型。改过最终版本的全套本地门禁、独立初审、主控复审与提交级CI均通过后才关闭008B/008。

## OPT-008C 收口（2026-10-19）

- **迁移与身份边界**：当前迁移末版 **v30**。v22 建 `contents` 并同一事务回填
  `content_id`、删除旧正文列；v29 删除零消费的 `articles_fts`，改建
  `articles_trigram_idx`；v30 只追加读闭包部分索引。「现行版本 21」只属于 2026-10-12
  的计划现场，文中相关章节按历史边界保留。
- **搜索口径（正式改写）**：搜索权威 = **LIKE**；`articles_trigram_idx` 是
  FTS5 trigram、contentless 的**可重建候选索引**（只收缩候选，等值验证仍由
  LIKE 完成；索引缺失/损坏回落纯 LIKE）。不得再以「存在 `articles_fts`」作为
  验收条件；共享正文变更后每个引用条目可搜、旧词消失、候选索引可从
  articles×contents 重建，即为满足。
- **identity 不变量（裁决 a）**：`contents.identity` 是（source, HTML, body）的
  内容等价身份，**修复错配 body 时必须重算**。重算命中同 source + 同 HTML +
  同 body 的已有行时按等价合并：全部引用条目重指、目标行 `revision+1`、原行
  删除；哈希碰撞/字节复核失败按 `ensure_content` 同一加盐策略分叉，绝不误并；
  目标行 `fulltext_extracted` 取并集，不弱化提取保护。反例锁定在
  `contents_ownership_e2e.rs::repair_recomputes_identity_and_merges_equivalent_references`
  与 `ensure_content_after_repair_reuses_fixed_row`（修前分别红于「引用未重指」
  与「同字节新建第二行」）。
- **必测补齐**：v21 两源同 URL/不同正文/不同 AI 迁移反例（含外键检查与重开
  幂等）、共享正文下 AI 年龄清理分叉、五布局读藏隔离与回执文案断言均已有
  回归；见 `tmp/optimization-20261008/OPT-008C/RESULT.md` 与对应证据日志。
