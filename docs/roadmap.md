# 当前缺口与已知限制

记录截至 2026-11-01 仍成立的未完成项与已知边界，按影响分层。已收口的事项不再列出；修复方案不在本文展开。为什么这样取舍见 `.agents/notes/`，测量边界见 [performance.md](performance.md)，协议语义见 [sync-compat-matrix.md](sync-compat-matrix.md)。

## 一、用户可见的缺口

| 缺口 | 现状 | 影响 |
| --- | --- | --- |
| 「滚动出列表即标为已读」的前置条件未声明 | 只在「显示: 未读」模式下生效（`Timeline.tsx` 的 `if (timelineFilter !== 'unread') return;`），设置页与文案都没说 | 用户以为这个开关一直有效 |
| 配置同步的镜像删除不产生远端退订意图 | mirror 模式的本地 feed/分类删除已实现；普通 UI 删除走 typed outbox 持久退订；镜像删除明确不调用 `enqueue_unsubscribe` | 镜像同步后远端订阅可能残留，需用户手动退订 |
| 封面候选仍无质量启发式 | 候选已扩到全来源「无封面 + 有原文 URL」；`first_image`/og:image 仍无尺寸/角色判断，头像、emoji、favicon 可能被当封面；本地为服务端文章补上的封面不参与失效清理（保守边界） | 部分文章无封面或封面不正确 |
| `--text-tertiary` 深色下对比度约 4:1 | 处于 WCAG AA 边缘，属于全局视觉 token | 低视力用户在深色模式下阅读辅助文字吃力 |

## 二、同步与数据正确性（2026-10-19 审计）

审计基线是 `v0.18.0` 发布后的 `dev`（`daa2bb4`），方式是只读静态审计：未跑门禁，未访问真实服务端、真实用户库或真实 WebView；缺字段响应的真实形态未实机验证。以下条目按代码现状与调用链判定，修复前不应视为已验收。

### P1（已全部修复；2026-11-01 复核证实）

原 4 条 P1——P1-A1（集合缺字段当空集）、P1-A2（订阅列表缺字段清库）、P1-A3（首连前 light 消费游标与历史）、P1-C1（重试非失败上下文快照）——已由 2026-10-31 收敛轮（阶段 B1-B4）修复。2026-11-01 独立证伪复核在代码级证实：集合字段必填与失败关闭（`greader.rs`）、首连三层守卫（`greader_pull.rs` / `fever_pull.rs`）、四类重试均按失败快照回放（`nav.ts` / `bootstrap.ts` / `ui.ts`）。按「缺口修复后删除」约定原表不再保留；逐条证据见 [审计修复收敛路线](../.agents/notes/implemented/process/2026-10-31-审计修复收敛路线.md)。

### P2

审计共 18 项 P2：C-P2-1 已并入 P1-C1 的同类问题，C-P2-6（`release.md` 的 tag 触发事实）已随本轮文档整理修正；下表为其余 16 项。2026-11-01 独立复核：A-P2 系列按下表行内标注部分修复，其余各行（含全部 B/C 系列）复核仍成立。

| 编号 | 领域 | 问题 | 影响 |
| --- | --- | --- | --- |
| A-P2-1 | 同步/账号 | feed 墓碑没有账号归属 | 断开后换号，旧账号的删除墓碑仍会压制新账号同 URL 的订阅（2026-11-01 复核：部分修复——直连换号已同事务清墓碑；「断开→连新账号」仍泄漏） |
| A-P2-2 | 同步/账号 | 订阅 outbox 未接入自动调度 | 重启后的后台自动同步不处理订阅 outbox；「离线或重启自动重试」的文案不成立，行会一直停在 pending |
| A-P2-3 | 同步/账号 | subscribe 挂载失败后移动丢失链起点；成功挂载未参与后继重定 | 删除后重加再移动时，远端旧分类可能永不移除，后续 pull 也不会修正（2026-11-01 复核：部分修复——in-place 合并与 edit 链重定已修；subscribe 行链起点与挂载成功重定两窗口仍在） |
| A-P2-4 | 同步/账号 | 条目状态字段缺省为 false | 部分响应缺状态字段时，本地读/藏会被回滚成远端的「未读/未收藏」（2026-11-01 复核：部分修复——集合级已失败关闭；条目级缺省 false 仍在） |
| A-P2-5 | 同步/账号 | 分类集合缺省为空 | 账号专属分类墓碑被误回收，目录结构丢失；后续可能重新导入已删除的旧分类（2026-11-01 复核：部分修复——Fever groups 已失败关闭；GR tag/list 缺 `tags` 字段仍当空集） |
| A-P2-6 | 同步/账号 | 订阅 outbox 的 session 构建失败不写 attempts/last_error | 认证/端点/网络失败时 UI 只能显示「等待」，无法解释原因，也没有失败记录 |
| A-P2-7 | 同步/账号 | `item_contents` 对非空请求返回空 items 仍视为成功 | 已列举的条目正文被永久跳过，游标照常推进 |
| B-P2-1 | AI/配置 | 正文修订或全文提取后 `aiStatusById` 不失效 | 旧 AI 产物仍显示「完整」，与实际正文不匹配 |
| B-P2-2 | AI/配置 | 卡片 partial 在重启后不可达 | 重启后只有片段、没有整篇产物的条目不再取校验状态，用户必须打开阅读器才能看到 partial |
| B-P2-3 | AI/配置 | `cache_cleanup(scope='ai')` 对仅片段条目不推进 epoch | 批量清理后，仍在途的旧请求可以写回片段或整篇产物 |
| B-P2-4 | AI/配置 | 配置快照重复分类名静默歧义 | 同名不同 `sync_id` 的分类对象级折叠已由 OPT-010 修复（身份优先匹配、重复 sync_id 拒收）；但快照 `FeedSpec.folder` 只携带名称，feed 归属仍按名折叠——第二个同名分类被新建却收不到源，预览无法分辨（2026-11-01 复核：原「合并到第一个」的机制描述已过时，残余后果仍成立，根因是快照格式） |
| B-P2-5 | AI/配置 | AI 校验状态请求没有代次守卫 | 清除或新生成之后返回的旧回包可覆盖新状态，重新显示 partial/stale/complete |
| C-P2-2 | UI/性能 | 画廊方向键语义回归 | 行优先 lanes 下 `ArrowDown` 实际向右，与 `ArrowRight` 等价；现有测试把回归锁成预期 |
| C-P2-3 | UI/性能 | 同一 URL 的后续条目在扫描窗/批次外被负缓存跳过 | 指向同一 URL 的其它条目在本进程内不再补封面，只有重启或缓存逐出后才可能重试 |
| C-P2-4 | 测试/文档 | act 警告降为计数后没有阈值与按用例归属 | 真实的 act 外更新、定时器泄漏或卸载后更新可以静默通过，回归不可归因 |
| C-P2-5 | 测试/文档 | 优化卡与优化索引的最终 tip、main 合并与发布状态落后于仓库事实 | 追溯证据时会读到过期状态；涉及历史材料，随其归档决策一并处理（2026-11-01 复核：优化索引已加状态注记更正关键漂移，卡片原文按历史记录保留） |

## 三、证据边界与未完成验收

- **真实 WebView 性能测量缺口**：OPT-017 的证据是隔离 SQLite、jsdom 与组件回归；真实 WebView 的滚动帧率、图片解码、真实高度分布与同构建端到端前后对照未测。判定线的 50ms 不适用于现有证据，不能据此宣称性能收益已实机验证。
- **OPT-015Q 用户实机复测未完成**：用户曾在 `v0.17.1` 观察到窗口「先居中再跳」；隔离驱动在 dev 的生产自动建窗路径未复现，生产代码零改动。用户尚未在包含窗口恢复修复的构建上复测，构建归属未证实。
- **OPT-015N 的场景边界**：八个原生窗口场景只覆盖固定双屏 3840×2160、1.5 缩放；物理拔屏、动态 DPI 切换与完整 SMTC 媒体键整链不在证明范围内。
- **AboutTab 更新检查的真实组件点击未覆盖**：现有证据是 Rust 本地 HTTP 单测、`compareVersions` 纯函数单测与源级接线；真实点击 → 提示的整链没有组件回归（用例因 Node 22 的 act 调度不稳定已撤回）。
- **服务端 × 版本兼容未实机验收**：兼容矩阵的「已验证」列只覆盖测试替身与严格夹具，不代表任何具体部署版本；真实 Miniflux / FreshRSS 未逐一实机验收。
- **存量 identity 惰性收敛**：identity 重算与等价合并只对触达行生效，未触达的旧行不会一次性回填；收敛随读取/写入逐步发生。
- **NotifCard #185 待观察**：浏览器 mock 模式下点「通知」布局曾崩溃渲染树（`getSnapshot should be cached` → React #185），后续复现尝试全部阴性；真机 Tauri 下是否复现尚未验证。测量电池与实机清单在存档分支，主线没有对应错误钩子；若实机复发，须先补 `componentStack` 捕获才能归因。

## 四、已评估并明确不修的边界

这一条不是遗漏，改动它需要新的决策：

- **流式翻译的 delta 不经过 sanitizer**。逐 delta 过 sanitizer 可能切断跨 delta 的 HTML 标签，破坏流式渲染。行为折中是「流式期间按纯文本渲染，`done` 后切 HTML」，靠 CSP 的 `default-src 'self'`（脚本继承该值）作为纵深防御。当前原始 delta 使用 React 纯文本插值，完成后回读净化版；不能把「未逐段清洗」单独等同 XSS，也不能据此放宽 CSP。

## 五、代码卫生

- **只定义不用**：`feeds.origin_was_local` 无置 1 写入方（`purge_remote_data_in` 里的重置为 no-op），删除列需迁移；`sync/phases.rs::sync_now` 无生产调用方（前端封装 `api.syncNow` 已删；集成测试仍消费）。两项保留。
- **`toast` 上限仍会挤掉旧瞬时 toast**：`src/store/slices/ui.ts` 的 `.slice(-4)` 在连续失败时仍会挤掉最旧的瞬时 toast；带重试错误已由错误中心（有界 20 条 + 同文案合并 + 24h TTL）持久承接。
- **反向索引**：`src/` 与 `src-tauri/src/` 的注释里保留了历史上的工作流编号（`TASK-xxx`、`REQ-0xx`）与界面契约编号。这些编号离开工作流后不可反查；相关结论已按主题整理进 `.agents/notes/`，但注释里的编号未逐处改写。

## 六、产品语义待确认

内容/条目分离已实现（v22 `contents` + `articles.content_id`），正文单一所有权与最后引用回收已落地。剩余是跨源同文的产品语义——read 广播、star 不广播——尚未由产品最终确认；同文副本的读状态传播政策仍是未经产品确认的默认（见 [sync-compat-matrix.md](sync-compat-matrix.md) §3）。

## 七、收敛轮（2026-10-31）遗留与不做项

2026-10-31 门禁先行收敛轮（阶段 A 替身语义对齐、阶段 B1-B4 源码修复、阶段 C 全新代理证伪复核）已收口的清单见 [审计修复收敛路线](../.agents/notes/implemented/process/2026-10-31-审计修复收敛路线.md)。以下为本轮明确不做、留待后续决策的项：

- **P3 与美观批次**：WCAG 对比度批量收敛（五调色盘 × 深浅色 230 项关键配对计算 95 项违例，浅色 `star-color`/`danger`/`accent-light` 最重）；模态焦点陷阱缺失（Tab 可穿透到背景）；dialog 语义覆盖 1/9；产品树无 ErrorBoundary；settings 组件 async setState 无 alive 守卫（6 文件）。
- **供应链**（2026-11-01 OSV 全量比对口径）：Rust 侧 13 条 `cargo update` 级通告（ammonia×3、rustls×4、quinn-proto×3、tokio×1、rustls-webpki×1、webpki-roots×1；ammonia 需过清洗 payload 矩阵回归）+ glib 1 条（需跨大版本，非 update 级）+ 10 条 unmaintained；npm 开发链 5 个 high（vite 8.2.2×1、undici 7.30.0×4，均不进产物）；CI action 未 SHA pin；安装包无 Windows 代码签名。
- **性能**：同步端到端耗时基线缺失（需真实账号并单独约定读写边界）；Fever 空正文条目每轮 with_ids 重拉一次（回补记账候选）；F1 计数绝对耗时跨 SQLite 构建不可比（查询计划形态已由 CI 断言锁定）。
- **架构**：`state ↔ github_auth ↔ config_sync` 三节点模块环；`internals.ts` 47 导出共享内核；`subscriptions.rs pull_feeds`（约 465 行）与 `fever_pull.rs pull_entries_fever`（505 行）超长函数；9 个零直测生产模块；组件直调 `api.*` 37 处无自动门禁。
- **协议残留**：Fever 星标探测→mark 的 TOCTOU（Fever 协议无条件写，双设备并发同条目低频）；GR 远端显式标未读不在轻量同步回流（文档化 read-wins 政策，是否改双向属产品决策）；迁移 v31 无专属单测。
- **流程遗留（不可逆）**：提交 `a26e8b4` 信息为 B1 夹具修复，实际卷入 B4 的 5 个文件（B4 代码唯一落点，内容正确但归属失真）。

## 八、第二轮全面审计新增确认（2026-11-01）

审计基线 `83fafbd`（dev）。三路只读深审（Rust 基础设施、同步残余面、AI 全链路）+ 主控逐条抽查复核；Rust 结论为静态代码证据，编译级验证由 CI 承担。本节只登记上轮未覆盖的**新增**确认项；与既有条目同根的（如 B-P2-3 的片段窗口细节）已在原行补充。

### P2（新增）

| 编号 | 领域 | 问题 | 影响 |
| --- | --- | --- | --- |
| R2-P2-1 | OPML/目录 | OPML 导入不复用已有同名分类，`folders.name` 无 UNIQUE 约束 | 已有同名分类时静默新建重复目录，订阅分裂；重复目录经配置快照上传会放大 B-P2-4 的按名歧义（`commands/opml.rs:83`、`db/folders.rs:39`，无测试锁定） |
| R2-P2-2 | AI/安全 | 清除缓存与在途流式生成无互斥，`rawTranslatedIds` 保护标记被提前删除 | 五态 partial 时「清除」+「继续」并发，未消毒模型输出可进 `dangerouslySetInnerHTML` 渲染路径（`ai.ts:720/739`、`Timeline.tsx:941/1430`）；恶意 feed 诱导模型输出标记时构成本地 XSS 窗口，持续到流结束 |
| R2-P2-3 | AI/状态 | 全局 `translating` 布尔的终态复位被 ownsView 门控 | 生成中切文章后布尔永久卡死，自动翻译静默失效、翻译错误行被隐藏，直到用户完整留守一次生成或重启（`ai.ts:577/606/660/686`、`reader.ts:207/441`） |
| R2-P2-4 | AI/错误 | 三处 AI 流程 catch 吞掉 rejection，硬编码「AI 服务未配置或不可达」 | 正文超 64 KiB、429、欠费、网络超时全部同文案，无法诊断；invoke rejection 是唯一错误通道（`ai.ts:416/539/676`，`extractError` 已导入未用于此） |

### P3（新增，摘要）

- favicon 探测与全文提取的响应体读取无大小上限（feed 主管线有 16MiB 界，两条旁路没有；`ingestion/favicon.rs:46`、`commands/settings.rs` 全文提取段）；SMTC 线程对超大有限浮点入参 panic 后静默失效（`media.rs:133/147`）。
- push_feeds 分类挂载失败后队项照常剪除、pull 不回灌既有源远端分类——远端订阅永久留默认分类且无收敛通道（`subscriptions.rs:303/325/1245`）；push_feeds 绑定查找吞 DB 错误且未走规范化 URL 匹配（`:291`，仅 legacy add_feed 行）。
- 订阅 outbox 对已消失账号无生命周期终点，pending 行连同 attempts/last_error 永久滞留（`db/outbox.rs:103`；`commit_account_settings` / `commit_disconnect` 均不清理）。
- `aiStatusById` 不随 AI 配置（模型/提示词/base_url）变化失效，配置变更后旧 verified 结果短暂滞留显示层（动作时刻有权威复核兜底）；清除单一产物推进共享 `ai_epoch` 会连带拒绝另一产物的在途提交（保守正确但无声明，`db/articles.rs:982-992`）。
- `folders.name` 无 UNIQUE 使「remote_labels 按名映射」不变量无 schema 背书，重名目录间映射取 rowid 首个（`db/migrations.rs:23`、`db/sync_map.rs:747`，与 R2-P2-1 同根）。
- ShutdownToken 只覆盖封面补全循环，调度主循环与 sync_light 无终止消费（应用退出靠进程终结硬停，SQLite 事务性保一致，属覆盖度边界）；pull_feeds 对每个远端订阅重复读并解析墓碑 JSON（千级订阅每轮数千次解析，纯性能）。
- 代码注释漂移三处（不改行为，随下次相关代码改动顺手更正）：`db/outbox.rs:51/136` 仍写「经幂等 quick_add 解析」（B1 起恢复路径实为订阅列表解析）、`sync/subscriptions.rs:63` 的「靠下次 pull 对账收敛」声明不成立、`db/sync_queue.rs:98` doc 与 SQL 对 `article_id IS NULL` 行为存在理论性偏差。
