# REQ-007 排查清单：空壳功能与隐藏 Bug（TASK-029）

- 排查范围：src/（前端全部 20 文件）、src-tauri/src/（后端全部 22 文件）+ tests 交叉核对；快照基线 git 8966ece，业务源码与 HEAD 一致。
- 方法：两轮独立全量只读审读 + 关键发现人工抽查复核（F1/F5/A-1/C-1/A-2 均已亲自到源码确认）。
- 置信度标记：**确认**=代码路径闭合验证；**疑似**=需运行验证或产品确认。
- 本清单仅为排查结论；**任何修复需用户逐项确认后才纳入批次**。

## P0 破坏性风险（建议最优先）

| # | 发现 | 位置 | 置信度 |
|---|---|---|---|
| P0-1 | 轻量同步对账把"拉取失败"当"空集合"：GReader 收藏流拉取失败→本地全部收藏被静默取消；Fever 未读集合拉取失败→本地全部标为已读。一次网络抖动即可造成静默状态丢失 | src-tauri/src/sync.rs:635-644、816-820（unwrap_or_default → reconcile_*） | 确认 |
| P0-2 | Tauri 生产环境后端异常时，错误路径回退渲染 mock 演示数据（假订阅/假文章进真实 UI），用户可能误以为数据还在 | src/store.ts:964-991、src/mockData.ts | 确认 |

## P1 用户直接可感知缺陷

| # | 发现 | 位置 | 置信度 | 关联 |
|---|---|---|---|---|
| P1-1 | 删除订阅不回传远端且无墓碑：下次 pull 按远端列表把已删订阅连文章拉回（复活）；源码注释表明"不推远端"是有意的，但复活后果未处理 | src-tauri/src/commands.rs:222-230、sync.rs:434-459 | 确认 | REQ-002 |
| P1-2 | 编辑订阅（改名/移动目录）从不推远端：greader.rs 已实现 edit_subscription/unsubscribe/subscribe 四个写方法但全仓零调用（死代码）；双端标题/目录永久分歧 | src-tauri/src/commands.rs:243-276、greader.rs:430-485 | 确认 | REQ-002 |
| P1-3 | push_feeds 丢弃队列 payload 里的 folder_id：订阅推到远端后分类映射丢失，OPML 目录结构远端全落默认分类 | src-tauri/src/sync.rs:317-334 | 确认 | REQ-002 |
| P1-4 | 分类改名/删除在 pull 时按远端旧 label "复活"为重复空目录（find_folder_by_name 失败即 create） | src-tauri/src/commands.rs:85-96、sync.rs:385-400 | 确认 | REQ-002 |
| P1-5 | 未连接期间做出的已读/收藏变更：不入队、不补推，连接后 pull 对账还会用远端状态覆盖（Fever 会把本地已读打回未读） | src-tauri/src/commands.rs:370-411、sync.rs:926-958 | 确认 | REQ-002/003 |
| P1-6 | 本地直连抓取的新文章不回写同步系统（无 remote_id、无队列动作）：两端数量天然不一致；纯本地源属协议限制，但"远端也有的源"绑定前状态变更永久滞留队列 | src-tauri/src/ingestion.rs:330-502、sync.rs:184-190 | 确认（部分属设计） | REQ-003 |
| P1-7 | 社交/通知卡片"翻译"按钮是空壳：只切换译文块显示，译文唯一生成时机是 article 布局打开文章，社交源文章永远到不了该布局→按钮高亮但译文恒空 | src/components/Timeline.tsx:382-392、548-554、store.ts（toggleReaderTranslation） | 确认 | 新增 |
| P1-8 | 正文水合/详情拉取无 .catch：一次 IPC 失败→Reader 空白无提示、社交卡永久"加载正文…"且无重试（与 REQ-001 症状直接相关） | src/store.ts:356、422-448；Timeline.tsx:351-353 | 确认 | REQ-001 |
| P1-9 | 空正文条目（content_html 为 NULL）无限重复水合：content 恒为空串→守卫永不过→虚拟滚动下重复 IPC 洪峰 + 永久假加载态 | src/store.ts:417-443 | 疑似（需确认社交源是否产生空正文） | REQ-001 |
| P1-10 | GitHub 登录 WebDAV 冲突检测用 String(e)：Tauri 错误对象恒显 "[object Object]"，WebDAV→Gist 切换确认框永远不弹 | src/store.ts:1054-1064（api.ts:14-24 已有 extractError 修复方案，此处漏改） | 确认 | 新增 |
| P1-11 | AI 连通失败 toast 直接插值 ${e}：同样恒显 "[object Object]"，AI 配置失败无法排障 | src/components/SettingsModal.tsx:720-722 | 确认 | 新增 |
| P1-12 | 从搜索/命令面板打开文章（anchorToArticle）不执行"打开时标已读"：与列表点开行为分叉，未读计数不降 | src/store.ts:949-958 | 确认 | 新增 |
| P1-13 | "全部已读"只传订阅范围不带视图筛选：在收藏/今天视图点全部已读会把范围内所有文章（含未显示的）标已读，与 toast 文案不符且不可逆 | src/store.ts:300-314 | 疑似（后端实现待核） | 新增 |
| P1-14 | loadMoreArticles 失败静默吞错（无 toast），且分页不带当前订阅/目录筛选，单源视图滚动加载口径错位 | src/store.ts:862-891 | 确认（吞错）/疑似（口径） | 新增 |
| P1-15 | 设置→通用"启动时打开"的"文章"选项是死选项：bootstrapSettings 校验白名单不含它，静默回落未读 | src/components/SettingsModal.tsx:218-230、store.ts:1415-1423 | 确认 | 新增 |

## P2 体验与一致性问题

| # | 发现 | 位置 | 置信度 | 关联 |
|---|---|---|---|---|
| P2-1 | summaryGenerating 是全局单布尔：为任一文章生成摘要时，所有空摘要卡同时显示"正在生成…" | src/store.ts:176、647-663；Timeline.tsx:512 | 确认 | 新增 |
| P2-2 | PlayerBar 续播竞态：进度读取是异步 IPC，本地音频/快速 metadata 时续播静默失效 | src/components/PlayerBar.tsx:57-77、104-116 | 疑似 | 新增 |
| P2-3 | 同步协议下拉是全应用唯一原生 `<select>`，其余均为 FluxDropdown（REQ-008 直接例证） | src/components/SettingsModal.tsx:998-1007 | 确认 | REQ-008 |
| P2-4 | AI"模型选择"改了不保存（须重新测试连通才持久化）；"保存提示词"实际保存整套端点配置，与注释语义相反 | src/components/SettingsModal.tsx:728-789 | 确认 | 新增 |
| P2-5 | Reader 阅读时长在正文水合前恒为"1 分钟阅读"，水合后跳变 | src/components/Reader.tsx:69-71 | 确认 | 新增 |
| P2-6 | AutoStartSwitch 失败时设置镜像不回滚，重开设置页显示错误值 | src/components/SettingsModal.tsx:141-152 | 确认 | 新增 |
| P2-7 | About"检查更新"在版本号未就绪/回退 0.8.0 时误判（compareVersions(remote, '') 恒大于 0） | src/components/SettingsModal.tsx:1438-1471 | 疑似 | 新增 |
| P2-8 | 搜索结果 Promise 竞态无代际守卫（防抖已缓解，低概率旧结果覆盖） | src/components/Overlays.tsx:72-105 | 疑似 | 新增 |
| P2-9 | "Miniflux 兜底路径"整体未实现：fallback_entries 恒 0，直连失败源的内容缺失，两个查询函数零调用；模块注释宣称的能力不存在 | src-tauri/src/sync.rs:5、34；db/feeds.rs:257-277 | 确认 | 新增 |
| P2-10 | extract_fulltext 防退化时静默返回原文，用户点"提取全文"看似成功但内容未变、标志不置位 | src-tauri/src/commands.rs:567-569 | 疑似 | 新增 |
| P2-11 | AI 小问题：ai_summarize 不校验空正文（浪费 token）；AiConfig preset 未知时静默回退 deepseek-chat | src-tauri/src/commands.rs:1102-1115；ai.rs:63-82 | 确认（轻微） | 新增 |
| P2-12 | 配置同步无删除语义：远端删除的订阅/分类本地永不删除（设计边界，但 skipped 计数易让用户误以为已对齐） | src-tauri/src/config_sync.rs:141-261 | 确认（待产品确认） | 新增 |

## P3 卫生 / 死代码 / 低概率隐患

| # | 发现 | 位置 |
|---|---|---|
| P3-1 | layoutNeedsBody 恒 false 死函数 + with_content 注释与实现脱节 | src/store.ts:117-119、261-264 |
| P3-2 | api.syncNow 前端零调用；commands::sync_now 与 sync::sync_now 两份等价实现（漂移风险） | src/lib/api.ts:492-495、commands.rs:959-969、sync.rs:1150-1161 |
| P3-3 | 队列卫生：无绑定条目永久滞留（无 TTL/兜底回收），pending 保护使状态分裂固化 | src-tauri/src/sync.rs:184-190、db/sync_map.rs:88-97 |
| P3-4 | take_sync_queue 错误当空队列吞掉（push_feeds、sync_local_feeds 三处） | sync.rs:324、commands.rs:869-893 |
| P3-5 | push 阶段多处 .ok().flatten()/let _ = 吞错（已读广播缺副本静默少推不可自愈） | sync.rs:187、197、348-353 |
| P3-6 | read_credentials 把 DB 读取失败当"未配置"，states_phase 误导性报 notConnected | sync.rs:46-58 |
| P3-7 | config_sync create_folder 失败→folder_id=0→外键违约回滚；apply_payload 静默吞 layout/AI 标志更新错误 | config_sync.rs:155-215、188 |
| P3-8 | set_setting 清墓碑失败被吞（关智能去重后重复文章仍被拦截） | commands.rs:507 |
| P3-9 | init 期 panic 风险点三处（default_window_icon().unwrap()、mutex 中毒即 panic） | lib.rs:145、203；ingestion.rs:534-537 |
| P3-10 | AiEvent::Error 变体从不发送（若前端依赖该事件则永远收不到） | commands.rs:1015、1209 |
| P3-11 | pull 单向：远端已退订的订阅本地永不删除（多端不一致，疑有意保守设计，需产品确认） | sync.rs:403-462 |
| P3-12 | 死代码：icons.tsx LAYOUT_LABELS、types.ts 旧版 ToastMessage、format.ts formatClock 无引用；两处空 className 残留 | icons.tsx:68-71、types.ts:81-84、format.ts:28-31、SettingsModal.tsx:533、611 |

## 排除项（已验证无问题，避免重复排查）

- 前端 50 个 invoke 命令在后端全部有对应 #[tauri::command] 注册，无"调用不存在的命令"。
- notifyOnNewArticles/autoRefresh/refreshInterval/fetchConcurrency/smartDedup/syncMode 虽前端只写不读，但 scheduler/commands 实时消费，非空壳。
- 快捷键表 6 项在实现中均有对应处理（独立审查补充：App.tsx:206-211 另有 Space 播放/暂停快捷键未列入设置页快捷键表，或注释与表不符——归入 P3 备注）；无 TODO/FIXME/unimplemented!。
- ~~tests 的 `#[ignore]` 均为真实服务器 live 测试~~ —— **本项已被 TASK-054 实证推翻并订正**（2026-09-18）。
  排查当时未逐条核对 ignore 的**理由是否成立**，得出了一条错误结论：实际共 23 个 `#[ignore]` 属性行，
  其中 **14 个的理由 `spins a local mock server` 不成立**（同目录 `sync_gap_repro_e2e.rs` 用**同一个 mock**
  且默认运行，说明该理由与实际不符），这 14 个已由 TASK-054 转正；另 9 个（需真实 Miniflux 账号+网络、
  或需固定 `127.0.0.1:8765` 外部服务）理由成立，保持忽略。
  完整清单、判定依据与计数闭合见 [`FINDINGS-IGNORED-TESTS.md`](FINDINGS-IGNORED-TESTS.md)。
  **教训**：排查「排除项」时，仅凭 `#[ignore]` 的存在与理由**字样**不足以判定其成立，须逐条核对
  「该理由描述的环境依赖是否真实存在」。

## 统计与建议批次归属

| 优先级 | 数量 |
|---|---|
| P0 | 2 |
| P1 | 15 |
| P2 | 12 |
| P3 | 12 |
| 合计 | 41（另 B-6 media.rs 非 Windows 空实现为有意设计、B-4 STATE_FILE_NAME 为有意预留，不列入） |

- 已在批次内的：P1-8/P1-9 → TASK-030（REQ-001）；P1-1~P1-6、P3-3 → TASK-031 定位（REQ-002/003）；P2-3 → REQ-008 控件统一。
- 建议新增首批修复候选（待确认）：P0-1、P0-2、P1-7、P1-10、P1-11（用户可感知且成本低）。
