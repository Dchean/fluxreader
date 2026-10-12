# OPT-008B：内容实体与各订阅源条目分离

Status: rework
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
