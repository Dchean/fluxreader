# 同步协议与服务端兼容矩阵

本页分开记录外部协议/服务端能力、客户端实际政策、验证覆盖。政策单点为 [conflict_policy.rs](../src-tauri/src/sync/conflict_policy.rs)；改变政策必须同步实现、本文和锁定测试。选择理由见[冲突政策决定](../.agents/notes/implemented/architecture/2026-10-07-同步冲突政策单点.md)。

外部参照固定为 [Miniflux faecfa2](https://github.com/miniflux/v2/tree/faecfa2fac61a91f4f69550bb1afd506c4b51749) 与 [FreshRSS 4e8a258](https://github.com/FreshRSS/FreshRSS/tree/4e8a258b011aad190597d7953b7122fc43f4658f)。2026-11-03 的来源记录在 `tmp/audit-sync-core/upstream/provenance.json`，2026-11-06 对订阅更新/URL 匹配另有固定源码复核。旧鉴权材料的 FreshRSS `219eaf58` 属 2026-10-08 时点。夹具模拟这些形态，不等于某个真实部署版本已经验收。

**验证上限**：生产客户端的受控 HTTP/SQLite 测试已覆盖下列有限路径；本轮同步缺陷与重认证报告修复已在固定范围通过独立复审，版本与原始失败链见[验收入口](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md#本轮修复的限定验收2026-11-07)。FreshRSS 1.30.4、Miniflux 2.3.5 的隔离真实服务端已完成下方[限定 live 实测](#live-deployments)；这些结果不代表全部部署、版本或异常路径均已验收。

## 状态操作与冲突政策

| 操作 | 外部能力 | 客户端行为与限制 |
| --- | --- | --- |
| 标读推送 | GR `edit-tag` 可聚合 entry ID；Fever `mark=item&as=read` 只接受单 ID | 绑定条目及已记账的同文副本广播；GR 合并请求、Fever 逐 ID；写后读回主目标确认 |
| 取消标读推送 | GR remove read；固定 FreshRSS/Miniflux 的 Fever 支持扩展 `as=unread` | 仅绑定条目，不广播。原 Fever 规范只定义 read/saved/unsaved，不能把此扩展当所有 Fever 服务端支持 |
| 收藏/取消收藏推送 | GR starred add/remove 为赋值；固定 FreshRSS api_version4 与 Miniflux api_version3 都是绝对赋值 | 仅绑定条目。Fever ≥4 直接写，3/未知先查 is_saved，仅不同才写以兼容历史 toggle；api_version3 不能等同 toggle |
| GR light 读状态 | 完整 reading-list 可用 `xt=read` 排除已读 | 单向 read-wins：远端明确未读不批量复活本地已读。当前未读全集与本地绑定未读差分，再查候选存在性；完整性失败不作差分 |
| Fever light 读状态 | `unread_item_ids` 全集，没有已读集合 | 双向权威：命中可恢复未读，未命中置已读；依赖集合完整和字段保护 |
| 两协议 light 收藏 | GR starred 流、Fever saved 集合 | 双向权威；完整集合内命中收藏、不命中取消。不是协议强制的冲突政策 |
| full 合并 | 协议提供条目及状态，不规定本地合并方向 | 共同使用 merge_pulled_entry；read-anywhere-wins，同源绑定允许明确 unread，跨源副本 unread 不落地 |

四格方向常量为 `GR_READ_DIRECTION`、`GR_STAR_DIRECTION`、`FEVER_READ_DIRECTION`、`FEVER_STAR_DIRECTION`。read-wins 是客户端为了避免历史文章成批复活未读而作的取舍，不能解释为 GR 协议不支持未读。read 广播、unread/star/unstar 仅绑定的跨源政策目前保持既有行为；是否改为布局隔离需要产品选择，内容实体分离并未自动改变它。

GR 的 `kept-unread` 标签（`user/-/state/com.google/kept-unread`）目前没有专门的识别或消费逻辑。收到该标签时仍按上述普通 read/unread 规则处理，不将它解释为持续“保持未读”的单独意图；客户端尚未实现 sticky-unread 语义。

共享保护：本地读/藏写入在同一事务递增各自 `read_version`/`star_version`，pull 在任何 HTTP 前捕获字段版本；落地只接受版本未变且对应字段无 pending 的结果。清队不清版本，read 的冲突不阻止无冲突 star，反之亦然；失败后重建映射不能刷新原版本快照。GR 权威集合失败按 read/star 字段隔离，成功侧仍可对账；Fever unread/saved 任一失败跳过该段对账。

## 条目身份、状态与确认

| 契约 | 当前实现及边界 |
| --- | --- |
| ID 形态 | Fever JSON 数字或十进制数字字符串都按整数处理，拒绝负值/溢出/浮点，不经 f64；GR 无前缀数字按十进制，带 Google item tag 的尾段按 hex，不凭长度猜进制 |
| 状态三态 | GR 缺失/null categories 是 Missing，显式 `[]` 是 false；Fever 缺字段 Missing、0/1 为明确值，其他数字拒绝。Missing 只更新可接受内容，不覆盖已有读藏 |
| ACK 不等于已应用 | 请求成功外观之后按目标 ID 读回；重复/无关 ID 不补足缺项。主目标存在且目标态一致才确认；空/短/缺状态/非法回包保留 retry_wait/verification，不猜目标删除 |
| 广播确认 | 主目标决定出队，副本缺失记 partial。相同 states 轮已尝试过的 queue ID 不补推第二次 |
| 入库归属 | 精确远端条目身份优先，URL 只能辅助匹配。有效远端 ID 且 feed 已绑定时，缺/空/空白 URL 仍可独立入库，空白链接不产生匹配键；新 GR 条目缺有效 origin 明确失败并保留游标，已绑定 ID 的稀疏回包继续兼容 |
| 订阅身份 | 远端 feed ID 优先，仅未绑定本地行参与 URL 兜底。远端来源的同 ID 改 URL 保留本地 ID、文章、布局、分类与自定义标题；本地来源的已绑定源保留本地 URL。多重绑定、规范 URL 多候选或唯一 URL 冲突明确报错并保留原值，退订对账按远端 ID 判断 |

有远端身份的本地意图进入推送；无身份的 direct 条目按所属源区分 local_only 与 bindable_retry，不计作可推 waiting。超过 30 天转 expired_unconfirmed，保留队项和本地读藏。显式恢复用摘要 account_generation 复核当前账号、按文章当前目标建立新操作 ID；缺文章仍过期，恢复不等于确认。断开/换号放弃旧状态队列，本地直连读藏保留但不会自动迁入新账号。详见[队列生命周期](../.agents/notes/implemented/architecture/2026-11-03-同步队列生命周期与本地意图.md)及[回补与恢复](../.agents/notes/implemented/bug-fix/2026-11-05-GR权威集合回补与过期意图恢复.md)。

## 分页、回补与持久边界

| 路径 | 获取预算 | 提交与恢复 |
| --- | --- | --- |
| GR 主列举 | reading-list + ot，单页 n=10000，最多200页；continuation 必须合法、未重复且严格前进 | 在列举开始前取起点墙钟候选。普通正文100 ID/块；逐语句落库，失败保留已提交内容并保留旧主游标，不能称整页回滚 |
| GR 权威集合 | unread/starred 用完整集合，不依赖 ot；分页不完整、空itemRefs却有continuation都报协议错误 | Missing/损坏集合不是空集；成功字段可独立对账。格式正常但语义漏报无法结构检测 |
| GR 缺身份/正文回补 | 每次 pull 每集合最多2000个唯一ID，100/请求、批内并发至多3；按数值ID与账号代际持久轮转 | 预算内请求和合并全部成功才存进度，缺feed/空正文也占预算防头部饥饿；进度或预过滤DB错终止。states发现订阅后最多重拉一次，同一states轮最多两份预算 |
| Fever 增量 | since_id 向新，每页50；非空短页继续到空页 | 连续成功的增量页或从顶部连续历史页才推进 since；数据和该页游标同短事务，失败整页及内存统计/maps/seen回滚，先前成功页保留 |
| Fever 历史 | max_id 向旧，页内最小ID为下页游标且严格减小；每轮最多200页 | Unknown/Pending/Complete，缺失Unknown、损坏明确报错；预算或失败保留Pending，自动light续取，full从顶部幂等重放。先存Pending才允许更大since；只有空页完成 |
| Fever 点查 | with_ids 50 ID/批；i64::MAX 顶边界另点查 | 点查与Pending续取页不推进连续since，防跳过区间；初始或回滚后map读取失败即停止，不退空映射继续 |

GR/Fever 协议响应使用16 MiB流式读取硬界，含解压后的大小。Fever完成时间写入只在无失败且历史完成后尝试，写入失败进入报告；GR游标在主列举、合并、绑定和权威回补全部成功后推进。绑定不完整或新条目缺归属时保留旧游标，下一轮可重试。

认证重试累计首轮已提交变化与统计，只过滤已恢复的认证故障。首轮非认证 SQL/协议错误继续作为注明“当时状态”的历史事实保留；重试成功不把旧“待重试”文案冒充当前队项状态，重认证登录失败也不替换原报告。远端已接受但本地确认出队失败时明确报告，队项留待重试。

Fever 的每次实际探测/写请求前复核账号会话代次，断开后阻止新请求；已经发出的请求无法撤回。并发 discovery 的已发请求也属于这一边界。2026-11-05/06 的原始反例与后继复审均保留在验收入口所列本地证据中，不改写原失败。

## 固定服务端差异

- **Miniflux GR**：ot 为 `published_at > ot`，严格大于发布时间；不是状态修改时间。固定 [handler](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/googlereader/handler.go) 与 [query builder](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/storage/entry_query_builder.go)支持此结论。未读/收藏完整集合回补绕开ot，已读未收藏晚到旧文仍依赖full；起点墙钟不能消除同秒或旧发布时间边界。
- **FreshRSS GR**：固定 [greader.php](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/p/api/greader.php) 将 MinDate/MinModifiedDate 用 OR 组合；[EntryDAO](https://github.com/FreshRSS/FreshRSS/blob/4e8a258b011aad190597d7953b7122fc43f4658f/app/Models/EntryDAO.php)为 `id >= ot×1000000 OR lastModified >= ot`。读藏改的是lastUserModified，所以不能把ot当状态变更流。认证形态夹具共享Miniflux时间模型，不证明FreshRSS时间列实测。
- **Fever端点与短页**：Miniflux常用 `{base}/fever/`；FreshRSS `/api/fever.php` 或 `/p/api/fever.php`，api_key在POST表单而非query。两者max_id页为id DESC。FreshRSS在LIMIT50之后由EntryBeforeDisplay过滤，hook也作用with_ids；非空短页不等于结束，被hook排除的项可能整体不可见。
- **Fever收藏版本差异**：固定Miniflux虽报告api_version3，saved/unsaved已是条件绝对赋值。历史toggle夹具是兼容反例，不能当所有v3事实；保守先探测仍有多设备并发窗口。
- **订阅修改**：固定Miniflux [Update Feed API](https://miniflux.app/docs/api.html#endpoint-update-feed)和[storage/feed.go](https://github.com/miniflux/v2/blob/faecfa2fac61a91f4f69550bb1afd506c4b51749/internal/storage/feed.go)允许同ID更改feed_url，并按原URL精确去重。因此客户端不能用折叠HTTP/HTTPS的URL覆盖两个已知ID。若部署总重定向到同HTTPS且服务端去重，不满足该碰撞前提。
- **Bazqux前瞻**：未接入。既有官方材料警告不要依赖ot主列举、推荐xt未读流；它保留退订源历史，未读集合中消失但contents仍可取的条目会击穿“未读差分＋存在性”的假设。接入前需定义退订行排除/清理域，不直接承诺兼容。

固定Fever集合端点没有continuation或已验证的状态增量参数，当前只能整段重拉。GR long-poll/ContinueWhatNewStream、Fever last_id_on_sync、订阅清单ETag/304在历史核对中缺少可采纳的固定版本依据，未实现为通用优化；这不等于未来版本永不存在。选择理由见[会话复用与有界并发](../.agents/notes/implemented/architecture/2026-11-01-同步会话复用与有界并发与聚焦同步.md)。

## 鉴权、分类与订阅写回

| 维度 | 当前契约 |
| --- | --- |
| GR认证 | 请求统一带 `Authorization: GoogleLogin auth=<auth>`；POST用action token作为T。Miniflux token可等于auth，FreshRSS可为独立字符串；首写OnceCell缓存 |
| token失败 | 仅404允许回退登录auth；401/403/5xx/空体/网络错明确失败，不靠URL猜服务端 |
| 分类读取 | category_name单点识别type folder或缺type且label前缀ID；label优先，缺省取`user/.../label/`后缀，中文/斜杠保留；state tag和type tag不成目录 |
| 分类写入 | 裸目标名始终前置一次`user/-/label/`后用form编码，名字本身含前缀字面量不误判；移动需处理旧成员关系，不把追加label当移动 |
| GR普通写响应 | HTTP成功且trim后为OK才视作成功外观；FAIL/空体/其他正文脱敏报协议错，不回显凭据或服务端正文。状态写还需目标读回 |
| 订阅能力 | GR支持客户端订阅/分类写回；Fever相应操作仅本地。空分类没有可移植远端操作；分类改名/删除按成员操作表达，不宣称远端空分类已改 |
| 退订例外 | 既有400/404容忍可能把真实删除失败当目标不存在并清队；墓碑仍由pull确认回收，本机不复活但远端可残留。此已接受上限与状态ACK读回是不同机制 |

能力声明由生产能力矩阵和持久订阅回执承担，详见[能力矩阵与持久回执](../.agents/notes/implemented/architecture/2026-10-15-订阅写回能力矩阵与持久回执.md)、[订阅执行器](../.agents/notes/implemented/architecture/2026-10-14-订阅写回outbox执行器.md)。配置镜像仅本机，不产生远端退订；配置中的连接资料只作待确认建议，不混旧凭据激活新地址。

配置快照保持 schema2，新导出的源用可选 `folder_sync_id` 精确引用快照内分类，身份未知时整体拒绝；旧文件缺字段才按未裁剪的精确分类名匹配，重名取快照首槽并在预览警告。旧客户端可忽略新增字段，但这不能补回旧版本的同名分类归属能力。恢复本机源只在新增事务内解除该 URL 的当前/legacy 墓碑，不产生远端订退、不迁移账号，也不清除此前明确入队的远端退订；预览会提示其之后仍可能执行。完整数据库恢复另见[备份说明](backup.md)。

<a id="live-deployments"></a>
## 真实服务端的限定实测

2026-11-07，客户端从 `dev@1bb698c` 加修复后的固定 `snapshot-cd` 重编，运行既有生产 live 测试，原测试未改。每套服务使用2源120篇合成文章，GR 3项为认证/订阅、ID/正文、收藏写回恢复；Fever 4项为认证/订阅、未读/收藏/条目、读藏写回恢复及空临时库的完整同步。完整同步实测为 Fever 引擎的2源120篇落库，不外推为已跑 GR 引擎完整同步。

| 服务端与运行环境 | 实际输入地址 | 已通过覆盖 | 仍未通过或未覆盖 |
| --- | --- | --- | --- |
| Miniflux2.3.5，固定 faecfa2，Go1.26构建、PostgreSQL17.6 | 根地址 `http://127.0.0.1:1922` | GR 3项、Fever 4项，各项恰1 passed且exit0；完整未读/收藏ID集合前后相同 | 其他版本、部署反代、多设备并发与全部异常输入未逐一覆盖 |
| FreshRSS1.30.4，PHP8.4.27内置服务器 | GR `http://127.0.0.1:13555/api/greader.php`；Fever `http://127.0.0.1:13555/api/fever.php` | GR 3项、Fever 4项，各项恰1 passed且exit0；完整未读/收藏ID集合前后相同 | 本配置的根地址自动发现失败，显式路径通过不关闭该失败；其他Web服务器、反代和版本未逐一覆盖 |

FreshRSS 首次输入根地址时，未知 `/accounts/ClientLogin` 被 PHP 内置服务器交给上级入口，302后返回200 HTML，客户端未得到认证体而失败；该次未改变服务端状态。明确 API 路径是现有产品支持的输入，没有修改客户端或测试来绕过断言。此结果限定当前服务器路由形态，不推导所有 FreshRSS 根地址都失败。

原始每项结果、服务端完整ID快照和148文件客户端绑定在本机 `tmp/repair-2026-11-06/environment/evidence/live-cd-final-summary.json` 及其所列目录；当时活动工作区只多出已记录的 scheduler 注释变化。上述为真实生产客户端的定向运行证据，未单独作独立审计，也不替代安装版/WebView2、真实用户库或最终集成门禁。

## 验证入口与边界

| 检查对象 | 已有可执行覆盖 |
| --- | --- |
| 政策方向/字段版本 | conflict_policy单元、pull_field_versions_e2e、dual_client_e2e、unread_diff_e2e；屏障覆盖请求后本地写、清队后旧快照、字段独立性 |
| 集合完整/游标 | greader_compat_e2e、star_reconcile_truncation_e2e、pull_cursor_e2e、pull_window_e2e；缺字段/重复continuation/短contents/请求ID覆盖 |
| Fever历史与恢复 | fever_compat_e2e：125条、非空短页、Unknown/Pending、200页续取、i64::MAX、页写失败/重建map失败、流式收体与字符串ID |
| ACK和本地意图 | sync_ack_verification_e2e、sync_gap_repro_e2e、expired recovery、sync-recovery组件；未知不判删除、主目标结算、过期重建与账号代次 |
| 本轮同步正确性 | sync_correctness_e2e；断开逐请求、无 URL、新条目缺归属、重认证保变化/故障、出队与时间持久化失败、远端 ID 优先及冲突保全。独立反例与最终 setter 的固定版本证据见验收入口 |
| 鉴权/分类 | greader_compat_e2e的两种严格认证形态、feed_edit与sync_gap分类移动；普通mock_greader不验证真实认证 |
| 真实部署 | greader_live_e2e、fever_live_e2e、fever_sync_live_e2e的原7个ignored用例已在上表两套服务端逐项运行；仅覆盖约定合成账号/读写恢复范围，不由CI默认测试推定 |

测试名是覆盖入口，不是当前全部通过的结论。精确提交的门禁状态与最近证伪结果由[验收入口](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md)集中记录；未解决行为留在roadmap，避免矩阵与每篇Note复制轮次进度。
