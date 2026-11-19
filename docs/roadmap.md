# 当前缺口与已知限制

本页只保留仍成立的开放缺口、接受的边界与未验收范围。已关闭项与被取代的旧取舍从本页删除，历史理由见对应决定记录；当前版本为 v0.19.0（`main` 已发布），以下差距不能由历史 CI 或单域证据合并抵消。

<a id="sync-correctness"></a>
## 同步正确性与源内身份

- **真实宿主事件连续性未验**：零新增真实变化已有回执与生产消费路径，但“真实宿主事件 → ReadingSession 消费 → 窗口保位”的完整链只在受控环境验证；DB marker 不能替代窗口保位。
- **当前组合的真实协议部署**：固定 FreshRSS 1.30.4 / Miniflux 2.3.5 各 7 项与 binding/dual/slow 共 12 场景只对应 5dc3 基线；localSync 组合改变共同事务/schema，不能借用旧基线结论。真实 WebDAV 多端持续收敛（两设备互改、断网重连、旧快照不复活删除）仍未验收。
- **远端物理删除关闭**：已知覆盖与替换对象精确 ACK 不能证明未知/离线读取者已无保护义务，因此不按年龄回收同步正文；远端对象增长、配额与列举成本仍未解决。
- **源内身份与早到绑定**：不同源独立、同源同内容分组、弱空观察不转移旧读藏已有实现与限定证据；真实慢内容与 native 链、当前组合的重新验证仍未完成。
- **目录名称、退订收藏与配置删除**：名称约束不改变跨设备稳定身份；删除/配置应用必须沿同一保留规则，不能按名称全表合并。真实界面列出/阅读/计数与新设备恢复组合仍待验。

<a id="reader-and-state"></a>
## 阅读正文与本地状态

- **真实 WebView2 连续阅读未验**：ReadingSession、五分区会话成员、日期失效与位置模块有纯逻辑/真实挂载/Edge headless 证据，但新组合及真实 WebView2 宿主事件、连续阅读链未验。
- **退订收藏的界面闭环**：后端保留收藏与稳定身份已有证据；真实界面阅读、计数、新设备恢复与配置删除组合仍待闭合，保留收藏不能只是当前库单次查询可见。
- **正文保留控件**：默认永久或 1–3650 天、原子设置保存、草稿校验与失败回滚已有专项；真实 Edge 的 Tab/Shift+Tab、方向键/Enter、错误长值与 980 窄窗，以及完整组合/native 仍待验。
- **正文保护范围**：只覆盖可恢复的文字/HTML 与媒体引用，不保证第三方图片/音频链接永久有效；缓存淘汰与持久数据删除是两个生命周期，不能互相替代。

<a id="ai-and-content"></a>
## AI 与正文结构

- **自动触发链的真实性**：布局/组/源三层策略与 700ms 中心停顿撤销有受控前端证据；真实中心停顿的自动触发、原生宿主与收费服务未验证，不能把一般布局通过当作 AI 专项通过。
- **付费模型与任意 HTML/provider 兼容**仍需与测试边界分开报告；流式未消毒文本只在本机内存过渡，安全回读后解除草稿。
- **AI 产物与本机配置**：AI 服务配置、密钥、产物与片段不进入同步/便携；普通设置中的 AI 行为开关会同步。此边界不因自动触发策略变化而扩大。

<a id="import-and-backup"></a>
## WebDAV、换机与新数据基线

- **规模验证标准为 10k distinct 4KiB 全流程**（4KiB 正文、全部相位、全部断言、2 秒门槛不变、仍 ignored）；**50k distinct-body 为本版开放边界，未验证**，历史证据 `r9-i1-50k-03/04` 保留。50k state-only 主测通过，但整库 apply 的 retention 排水在 60 分钟硬上限内未完成。规模基线与实测见 [性能基线](performance.md) 的当前本地同步基线段。
- **已知规模瓶颈**：整库 state 应用到新库会为每条被应用的 read/star 字段生成 `retention/` 工作键，只影响一次性整库应用，不影响导出、前台延迟或正确性。修复需 schema 索引/迁移与旧库兼容验证，本轮不做；工作键规模与排空耗时见 [性能基线](performance.md) 的当前本地同步基线段。
- **换机恢复的原生链**：便携 v4 预览/确认/事务恢复有实现与专项证据；真实原生文件选择器、跨机器、断电恢复与安装版 WebView2 未验，不能由应用内测试推定。
- **新库/新格式**：独立 `fluxreader-v2.db` 空间保留旧库、误放旧格式只读拒绝、旧云快照失败不当空库；真实宿主提示与新设备恢复仍待验。

<a id="interface-and-media"></a>
## 界面与媒体

- **原生宿主**：五布局/社交流式正文/画廊/J-K/Tab-Space/模态焦点在真实 headless Edge 有门禁；真实 WebView2 的打印/滚动 p95、解码、原生窗口与系统读屏未验。
- **图片代理**：代理/队列/lease/字节预算与失败占位有受控 IPC 证据；真实 Tauri raw/fallback、生产 CSP、解码释放、可见锚与预算压力未验，不能把普通 HTTP 图片布局外推为 native 性能或 OOM 结论。
- **motion**：reduced-motion 的 transition 0s 修复与两种媒体偏好门禁已落地；历史 159px/295px 漂移与 CI 失败保留，不改写为“从未发生”。
- **设置浮层快捷键让路**：Ctrl+S/Alt+J 穿透修复有独审证据；真实宿主控件消费顺序仍需持续回归。
- **列表宽度上限不一致**：`src/store/settingsValidation.ts` 的 `NUMERIC_RANGES.listWidth` 接受 280–560，而 `App.tsx` 的拖拽夹取与 `base.css` 的 clamp 是 280–420。越界值能通过校验，但被 UI 夹到 420（同步或便携带入的值与本地界面表现不一致）。属行为项，未修；修前须定「放宽 UI 到 560 还是收紧校验到 420」。
- **Edge headless 门禁与 TEMP**：`npm run test:ui-layout` 在 `%TEMP%`/`%TMP%` 指向仓库外的环境、基线 `30cfd65` 上运行验证通过（exit 0、`ok:true`、`attempt=1`、`retried=false`；4 视口 2 尺寸 × 2 动效 + 6 键盘视口全跑，画廊/社交域断言与 `consoleErrors`/`exceptions`/`unexpectedNetworkFailures` 均空；`firstMount` 48.8 s / 120 s 预算）。上一轮的「红」已由独立只读证伪逐链复核为环境红：`%TEMP%` 指进仓库时 Edge profile 落在 Vite watch 树内，FSWatcher 撞到被锁的 `Default\Network\Cookies(-journal)` 触发 `EBUSY`，Vite exit 1。**工具侧防假红已加固，并有用例级证据**（基线 `5f50cea`，日志在 `tmp/r1-gate-2026-11-19/`）：系统临时目录落在仓库内时启动前拒绝并给可执行提示（`02-after-temp-inside.log`，约 0.6 s、exit 1，且不先删除上一次的证据）；`startVite` 监听子进程 `exit`/`error`，启动期退出即快速失败并带输出尾部（`08-vite-startup-exit-fastfail.log`，约 2 s、exit 1），其中 `error`（spawn 失败）分支为静态推导，未做注入实测；`evidence.vite` 用 `exitKind` 区分「自行退出」与「被 runner 终止」并记录 `exitedAfterMs`/`killedAfterMs`；证据清理只删本 runner 产物，未知目录与文件保留并点名。故障注入（`05-fault-injection.log`）与 CI 变体 `UI_LAYOUT_TRACE=viewport`（`06-ci-trace-variant.log`）已在本机复跑，均 exit 0。剩余风险：`vite.config.ts` 未忽略 `tmp/`——就绪后 Vite 崩溃仍按 120 s 预算等待（`07-vite-exit-fastfail.log` 实测约 255 s 后失败）；启动即崩仍按原语义重试一次；CI 已在 `bd92a73` 的 `windows-latest` 跑通（ui-layout / rust / frontend 三 job 全绿，run 44525587721），但只有一次绿跑、不足以证明不 flaky（首挂载已用掉 40.7% 预算）；TEMP 守卫只覆盖启动前成因，比较的是解析后的系统临时目录（Windows 上 `os.tmpdir()` 只读 `TEMP` 再 `TMP`，实测不读 `TMPDIR`；大小写已折叠、junction/短名已用 realpath 展开，有 `09-guard-probes.log` 证据；目录不存在等异常形态未验证）。证据 `tmp/ui-layout-2026-11-19b/`（`30cfd65` 绿跑与对照）与本轮 `tmp/r1-gate-2026-11-19/`。性质：本机运行验证；本节不代表门禁不会假红。

<a id="verification-boundaries"></a>
## 未完成验证与条件风险

- **原生 Windows**：历史隔离实机 17 项中 15 通过、1 部分（通知横幅/点击）、1 未执行（物理拔插/真实注销登录自启）。MSI/NSIS 安装器、动态 DPI/物理拔屏、用户冷启跳位仍未完成；SMTC AUMID 当时为未打包 `app.exe`，不等于安装版结果。
- **本地门禁环境**：非提权会话里由 Rust 构建的进程可能无法创建真实文件符号链接（`CreateSymbolicLinkW` errno 1314），retention 测试用目录 junction 作等价 reparse-point 夹具；真实 symlink 路径的唯一测试 `writer_windows_symlink_alias_resolves_actual_file_identity` 带 `#[ignore]`，CI 的 `cargo test --no-fail-fast` 不运行 ignored 用例——本地与 CI 均未覆盖，只在具体任务约定读写边界后单独执行。
- **性能与资源**：10k 容量与 50k 状态 exact 自然 exit 0；50k distinct-body 未验证。长会话、真实规模滚动 p95、图片 native 预算与完整前台 SLO 未验收；真实持久存储 I/O 损坏/WAL 故障注入仍无安全手段。
- **待观察崩溃**：旧浏览器 NotifCard 曾见 React #185，后续反例阴性；ErrorBoundary 已捕获 `error + componentStack`，复发时保留现场与复现输入，不按旧观察声称当前必崩。
- **审计与发布**：本版已发布不等于所有主题均通过独立审计；各篇 Note 的验证时点与上限仍以原文为准。

<a id="accepted-tradeoffs"></a>
## 继续保留的边界

- 单 FreshRSS/Miniflux、GR/Fever 与本机提前抓取是已确认范围；不新增多账号、其他 RSS 服务或跨源状态传播。
- GR/Fever 冲突接受服务端接收顺序；WebDAV 按同设备顺序、已观察因果与固定并发规则收敛，不承诺离线真实点击/上传先后。
- 凭据继续 Windows DPAPI 失败关闭，不经普通设置/同步 payload 泄露；HTML 净化、外链边界、事务与一致备份不移除。
- 未确认意图、写后验证、账号代际与字段版本继续保护；跨账号不迁移未确认意图，断开后迟到网络结果不写回新账号。
- “RSS 原文”仅在确实持有独立 RSS 字节时提供，不承诺同时持久保存 RSS 与提取全文两份。
- 源自定义标题仍无“恢复跟随源站标题”入口；已发出的退订无法撤回等历史上限不由新契约自动修复。
- 无代码签名，Windows 未知发布者提示仍适用；action SHA pin 没有自动保活机制。
- 历史提交哈希与 tag 不重写；旧版本说明的 2026-11-02 规范化例外继续有效，见 [release.md](release.md)。
- favicon 探测只打 http/https 公网目标：私网/保留段（含 loopback）在守卫处拒绝，自托管于 localhost/内网的源因此不显示图标，失败并入 `FAVICON_TRIED` 负缓存静默（`ingestion/favicon.rs` 的 `discover_favicon`/`target_allowed`/`ip_disallowed`）。解析判定与建连是两次独立解析，理论上可被 DNS 重绑穿透——完整封堵需按 IP 连接并处理 Host/SNI，对 favicon 探测不成比例，接受该残余。
- 跨源 favicon 由 WebView 直连渲染（探活 http(s)/2xx/非 HTML、no-referrer），不引入域名白名单：渲染引用外部资源与用户浏览器同级且已有探活门；白名单会静默吞掉合法 CDN 跨源图标、改变既有预期；无清单可维护，负向代价大于残余收益。若引入属产品决策，落点在 WebView 探活门并需用户例外途径。

<a id="doc-todo"></a>
## 文档待办

已知的文档缺口（缺失、失实、过期），或已发现但本轮不更新的差异。**下次触碰对应模块时必须处理**，没有过期作废。

| 文档 | 位置 | 差异 | 登记日期 | 下次触碰的模块 |
| --- | --- | --- | --- | --- |
| roadmap.md | 维护边界末句 | 「GC17」「旧 bool/dedup 清理」代号在受跟踪内容无出处（本句下已加临时指向） | 2026-11-15 | 维护边界改动或 tmp 清档任务 |
| `docs/audits.md` | 覆盖矩阵 | 2026-11-19 文档批已把 docs 11 篇补到「逐篇（本轮文档批）」；剩余真实缺口：仓库根 `README.md` 停在 2026-11-15（其产品面向内容本轮只改为链接入口，未逐条复核） | 2026-11-19 | 下一轮审计登记时 |
| `docs/contracts.md` | 契约单点表的三处事实 | 阶段B 静态审计记与实际行为有出入：写事务与锁序行（`commands/sync.rs` 的 `sync_local_feeds` 入队三语句各自提交，与 outbox 模块头「同一事务落库」不符，裁决见本表 outbox 行）、设置名与值域行（`p/*` 投影值绕过 `validate_app_setting`）、条目身份与去重键行（`db/url_norm.rs` 的 query 对非 UTF-8 百分号字节有损）。**待用户裁决**：补文档声明现状，还是修代码（缺 `outbox`/`body_retention` 两行已由 2026-11-19 文档批补齐，此项已关闭） | 2026-11-19 | 裁决后或触碰契约表时 |
| `docs/contracts.md` | 订阅写回队列（outbox）行 | 未写事务边界：`db/outbox.rs` 的 `enqueue_subscribe` 自带 SELECT/DELETE/INSERT 但不含事务，依赖调用方。**待用户裁决**：补文档，还是把入队收进调用方事务 | 2026-11-19 | 待用户裁决后 |
| `docs/contracts.md` | 正文保留与释放（本机）行 | 未写失败语义：阶段B 静态审计记「永久保留模式下错误被静默」，本批未复核。**待用户裁决**：补文档声明失败语义，还是修代码 | 2026-11-19 | 待用户裁决后 |
| `src-tauri/src/db/backup.rs` | `automatic_backup_retention_preserves_symlinks` 的文档注释 | 注释称 CI（windows-latest）仍覆盖真实文件 symlink 路径；该测试带 `#[ignore]`，CI 只跑 `cargo test --no-fail-fast`，不运行 ignored 用例 | 2026-11-19 | 备份保留测试或该注释被触碰时 |
| `docs/sync-compat-matrix.md` 与 `docs/roadmap.md` | matrix「真实服务端的限定实测」段 vs 本页「当前组合的真实协议部署」条目 | 真实部署基线标识不一致：matrix 记客户端基线 `dev@1bb698c`，本页记「只对应 5dc3 基线」；需统一标识或各自注明指代（哪个是实测产物、哪个是范围声明） | 2026-11-19 | 下次触碰真实部署证据或这两篇时 |

<a id="maintenance"></a>
## 维护边界

依赖升级不因清提示而顺带进行：Windows 目标 Cargo 依赖仍有少量 unmaintained 提示，当次未匹配适用非信息类漏洞告警；这些是当时证据，不是当前无漏洞保证。GC17、远端 body DELETE 与旧 bool/dedup 清理不在本版候选（“GC17” 等代号仅在本机审计材料 `tmp/refactor-2026-11-09/` 有出处，未随库留存；下次触碰维护边界时先固化含义再清理该目录）；需要时按新任务与独立验证重新授权。
