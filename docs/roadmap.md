# 当前缺口与已知限制

当前工作区为 `dev@1bb698c` 加本轮未提交修复；该提交自身不包含这些修复。本页只维护尚未关闭的问题、接受的边界与未完成验证。本轮同步、数据、前端（含画廊/焦点批）、依赖、云备份 WebDAV 与后端集成各批已按限定范围完成修复与独立复审；真实服务端 live 7+7 与原生隔离实机 15 通过/1 部分/1 未执行是限定运行证据，未单独作整批独审。本地整合门禁（Rust 五项、前端 lint/build/状态/组件、`test:ui-layout` 全量）已在当前未提交工作区通过；发布前独立审计有条件通过进入提交与 CI 流程（`tmp/prerelease-audit-2026-11-07/REPORT.md`），本波远端 CI 尚未运行，未提交、未合并 main、未发布。分批关闭范围、限定条件及原始失败链集中于[验收入口](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md#本轮修复的限定验收2026-11-07)。

原始反例保留在本机 `tmp/audit-2026-11-06/AUDIT.md` 及分域日志，修复审计在 `tmp/repair-2026-11-06/`，画廊/焦点批证据在 `tmp/frontend/gallery-ui-r1/final-r1/`；这些是忽略目录中的本地证据，不随仓库分发。旧问题编号用于追查历史，不把已关闭缺陷继续列为当前问题。

<a id="sync-correctness"></a>
## 同步正确性

| 问题 | 当前结果与限定 |
| --- | --- |
| 真实服务端状态与完整性边界 | GR light 保持 read-wins；已读未收藏晚到旧文仍依赖 full。格式完整但语义漏报的权威集合无法由分页守卫识别；详见[兼容矩阵](sync-compat-matrix.md) |
| FreshRSS 根地址发现 | 本次 PHP 内置服务器将未知 `/accounts/ClientLogin` 重定向302后返回200 HTML，根地址登录失败且服务端状态未变；显式 `/api/greader.php` 与 `/api/fever.php` 通过限定 live 检查。该部署的根地址自动发现仍未通过，不能由显式路径成功覆盖 |

<a id="reader-and-state"></a>
## 阅读正文与本地状态

滚动标读前置条件说明已随画廊/焦点批关闭：设置页补“仅在未读列表中生效”并保留原 unread 守卫，最终候选字节通过组件门禁与 `test:ui-layout` r8 全量（证据 `tmp/frontend/gallery-ui-r1/final-r1/` 的 `gates-r3/` 与 `ui-layout-r8/`）。本域其余未关闭边界集中在[未完成验证](#verification-boundaries)。

<a id="ai-and-content"></a>
## AI 与正文结构

本轮已确认的 AI 状态、仅片段恢复、列表结构、清理代次及响应预算问题已按限定审计关闭，证据统一见[验收入口](../.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md#本轮修复的限定验收2026-11-07)。真实付费模型、任意HTML/provider及安装版运行不由这些检查覆盖；仍接受的显示与媒体边界见下方取舍。

<a id="import-and-backup"></a>
## 导入、配置与备份

早库预快照凭据防御与 v31 空链接键清洗已在限定独审关闭。旧历史快照不自动改写，DPAPI密文不能跨 Windows 用户直接解密，应用内覆盖式恢复仍不提供，详见[备份说明](backup.md)；合成早库反例不代表已证明真实历史用户泄露。

<a id="interface-and-media"></a>
## 界面与媒体

画廊挂载范围与过程锚（G1/G2）、行侧陈旧滚动位置、对比度/活动项/换行（U1–U3）及滚动标读说明已随画廊/焦点批关闭：23 文件候选在定向复审时与当前工作区 23/23 逐字节一致，此后仅清单内 2026-10-16 画廊有界渲染 Note 作 2026-11-07 后继文字修订（2026-11-07 新 Note 不在原23文件清单内）；产品/测试字节未变；`test:ui-layout` r8 全量 exit0（980 首屏深滚、1440 wave-1 锚漂移 295px 等 r3–r7 历史失败点全部归零，`consoleErrors`/`exceptions` 为空），lint/build/test:frontend/test:components 四门禁 exit0（组件 136/136），原只读诊断者定向复审通过。证据：`tmp/frontend/gallery-ui-r1/final-r1/ui-layout-r8/metrics.json`、`gates-r3/`、`audit-row-root/REPORT.md` 与 `REVIEW.md`；r3–r7 原失败报告保留。限定：U1 的四深色盘选中导航对比度证据为静态 token 计算（5.61/5.89/4.64/5.21，均高于 4.5），未逐盘采集真实 computed style。

封面已完成的候选筛选、结果复用、失败TTL和全来源本机失效政策见[性能文档](performance.md#cover-budget)，启发式仍不能识别所有视觉角色。

<a id="verification-boundaries"></a>
## 未完成验证与条件风险

- **启动接手时序（NF4）**：受控 `get_setting` 回包延迟序列已随最终候选通过（共享启动 gate 防御经四门禁复验），但原生 IPC 触发可达性未验证，未计确证产品缺陷。共享 DB Mutex 不允许把原探针解释为慢 SQL 持锁时被列表超车。
- **云备份**：WebDAV 生产函数往返已在本机 WsgiDAV 两轮真实 HTTP 通过（凭据 DPAPI 语义保留、预览无写副作用，证据 `tmp/repair-2026-11-06/environment/evidence/cloud-backup-r1/REPORT.md`）。Gist 因无任务隔离凭据未验证，create/update/read、secret、删除与 404 均未运行，属待用户裁决项；不外推第三方托管 WebDAV。
- **真实部署**：隔离 FreshRSS 1.30.4、Miniflux 2.3.5 已各完成原生产客户端的7项限定 live 检查，覆盖 GR 3项、Fever 3项和 Fever 完整同步1项；版本、路径及根地址失败见[实测矩阵](sync-compat-matrix.md#live-deployments)。这不覆盖全部部署与协议异常、多设备并发。
- **原生系统集成**：隔离实机验证 17 项中 15 项通过（自启 disable 按 auto-launch 0.5.0 实际行为在 StartupApproved 留隔离名残留，Run 值已删）、1 项部分验证（通知横幅可见性/点击：OS 投递已证实，横幅受系统策略/勿扰影响未观察到，产品无点击激活处理器）、1 项未执行（物理拔插/真实注销登录自启），证据 `tmp/repair-2026-11-06/environment/evidence/native-verify-r1/checklist.md`；数据边界哈希级零改动。SMTC 会话 AUMID 实测为 `app.exe`（未打包 exe 按文件名注册）。仍未验证：MSI/NSIS 安装器验收、动态 DPI/物理拔屏与用户冷启跳位复测（旧窗口驱动仅覆盖固定双屏 3840×2160、1.5 缩放）。
- **性能**：真实库同步耗时、当前 WebView2 长会话与滚动 p95 未测。短时开发浏览器 rAF 和历史 SQLite 数字不填补这些空缺。
- **测试维护**：About 实际设置点击链、portal 卸载与警告到达阶段记录已随最终候选组件门禁复验（`gates-r3/` 组件 136/136）；更新请求未完成时切页/关闭后的迟到回包仍需定向补查。act warning 的到达阶段不等于真实异步来源，非致命政策与无阈值保持（当次组件运行记录 11 条）。
- **待观察崩溃**：旧浏览器 NotifCard 曾出现 React #185，后续反例阴性；已有 ErrorBoundary 捕获 error/componentStack，复发时应保留现场，不按旧观察宣称当前必崩。

<a id="accepted-tradeoffs"></a>
## 仍保留的产品与兼容取舍

- GR light 的 read-wins、标读向同文副本广播而未读/收藏只当前绑定，仍是既有客户端政策；跨源传播是否改为布局隔离尚待产品决定。
- Fever 历史 toggle 兼容的探测→mark 没有服务端条件写保证，保留多设备并发窗口；空正文条目可能每轮 with_ids 重拉。
- GR 退订 400/404 容忍可能清队但远端仍在；本机墓碑阻复活，不能把清队当远端确已消失。改变取舍前要确定读回与重试成本。
- 配置镜像只改本机订阅/设置，删除不产生远端退订；连接建议不直接激活凭据或旧绑定。恢复配置不等于恢复原账号阅读状态。
- 配置恢复只解除实际新增恢复 URL 的墓碑；此前明确入队的远端退订继续保留并在预览提示，之后仍可能执行。同名分类的旧快照未存身份时按精确名称取首槽并警告，无法重建原文件丢失的归属。
- 跨账号不迁移未确认意图；断开保留的直连读/藏不会自动补推新账号。显式恢复过期意图只按现有身份重建，不创造远端目标。
- 不提供应用内覆盖式数据库恢复；退出后手动恢复的边界见[备份说明](backup.md)。
- 流式译文以纯文本显示，完成后回读净化 HTML；不逐 delta 解析可能被截开的标签。无 src 的 data-src 图片和协议相对 iframe 仍是当前兼容边界。
- “RSS 原文”只在正文缓存实际持有不同于全文的独立 RSS 字节时显示；不持久保存双份正文。缓存淘汰或重启后不能凭按钮恢复未保留的源文。
- 社交短文固定正文区与按钮槽为异步水合稳定性取舍；紧凑化须重新验证媒体加载和锚。
- 旧二进制订阅改名没有 UPDATE 兼容触发器；自定义标题没有“恢复跟随源站标题”入口。存量内容 identity 仅触达时惰性收敛。
- 旧版断开未留下账号见证的间隙无法回溯；订阅发送确认与后继链重定之间进程终止仍有恢复上限。

<a id="maintenance"></a>
## 维护边界

2026-11-07 根工作区已整合 Vite8.2.4、undici7.30.2，完整与仅生产依赖的 npm audit 当次均为0；两项补丁差异与 Node22 定向兼容已独立核验，原两个 high 开发依赖报告保留作历史。新版依赖下的整合门禁已在当前工作区通过：Rust 五项见 `tmp/repair-2026-11-06/integration-rust/report.md`，前端四门禁与 `test:ui-layout` r8 见 `tmp/frontend/gallery-ui-r1/final-r1/gates-r3/` 与 `ui-layout-r8/`。audit为0是当次结果，不代表不存在未知问题。

同日按当前 Cargo.lock 与 `x86_64-pc-windows-msvc` 依赖图复核 RustSec/OSV，Windows 范围内剩余6项 `unmaintained` 信息提示：fxhash、unic-char-property、unic-char-range、unic-common、unic-ucd-ident、unic-ucd-version；此次未匹配到适用的非信息类漏洞告警。glib/GTK 不在该 Windows 依赖图中，不据此为 Windows 客户端引入跨大版本迁移。该结果是版本与目标平台静态适用性分析，不是运行安全或无漏洞保证；原始数据库版本、方法和输出见验收入口所列维护证据。未维护依赖随上游正常维护重访。

Windows 安装包仍无代码签名，首次安装的未知发布者提示仍适用；本轮未引入签名服务。action SHA pin 无自动保活机制。

保留但尚未处置的代码卫生点包括 `feeds.origin_was_local` 无置 1 写入方、仅测试消费的 `sync/phases.rs::sync_now`、历史 TASK/REQ 注释和共享 store 导出面。是否调整须依据实际维护成本，不按行数或清单长度启动重构。历史提交 `a26e8b4` 标题与卷入文件归属不一致已发生，保留历史，不重写哈希。
