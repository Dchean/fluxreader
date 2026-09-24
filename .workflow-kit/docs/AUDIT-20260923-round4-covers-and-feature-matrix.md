# fluxreader 审计第四轮（封面专项 + 全功能矩阵 · 真机取证）

- 基线：`git HEAD = d6234e5`（main；业务代码零改动，仅 `.workflow-kit/` 文档与 `tmp/` 脚本）
- 授权与口径（用户指令）：**继续用与真实数据一致的模拟数据测试**、**测试所有功能是否如期运行、确保没有遗漏**、
  **不跑门禁**（修复时再跑）、证伪优先
- 前序报告：[第一轮](AUDIT-20260922-shells-and-regressions.md) · [第三轮（真机 DOM）](AUDIT-20260922-round3-dom.md)
- 本轮新增：路线 **M**（数据库/schema 完整性）、路线 **R**（封面管线与外围子系统）两份并行只读审计 +
  **主控真机功能矩阵**（s7a/s7b/s8/s9/s10/s11 六组场景，覆盖读路径、写路径、浮层、播放器、失败可见性）

## 0. 方法：这一轮的「模拟数据与真实数据一致」是怎么做到的

1. **结构**：假后端的建表语句来自用户真实库的 `sqlite_master` 导出（`tmp/audit-r3/harness/schema.sql`）；
   真实库只以 `?mode=ro` 读过一次，抽样前后原库 SHA-256 一致（`1790cdcc044bcebe…`），含凭据的副本已删除。
2. **值格式**：`published_at` 一律 RFC3339 带 offset、`fetched_at` 用 SQLite 空间格式、`source ∈ {direct,miniflux}`、
   `url_norm`/`remote_dup_ids` 非空——均照抄真实行的形态。
3. **业务分布**：folders/feeds 直接照抄真实库（分类名、布局绑定、feed 级 layout 覆盖、origin），
   文章数按 0.6 缩放（4992 → 3005），未读按源内最新若干条分布。
4. **封面/防盗链**：本轮封面场景的判定依据是**我自己对真实图床的实测**（见 §1.3），
   再把这套策略同时装进浏览器侧（CDP 拦截）与后端侧（`fetch_image` 复刻 `settings.rs:155-200` 的 Referer 候选链）。
5. **harness 教训（不是产品缺陷）**：`ModalOverlay` **无条件渲染** children（关闭态仅 `opacity:0`+`inert`），
   于是 `document.querySelector('.mini-dialog-actions button')` 会命中**关闭弹窗里的按钮**，
   程序化 `.click()` 绕过 `inert` 造成「点错弹窗」的假象（本轮第一次跑 7b 时踩到，已在 s8/s9 用
   `.modal-overlay.open` 限定作用域修正）。**这类假象不进结论。**

---

# 一、用户报告的「文章封面部分获取不到」：六个独立成因（含量化与真机证据）

## 1.1 成因 A（影响面最大）：miniflux 源的无封面文章**永不被补全**

`db/articles.rs:674-687` 的 `articles_without_cover` 只取 `source='direct'`，`scheduler.rs:300-301`
注释称「Miniflux 源入库时已用正文第一图兜底」，但 `sync/entries.rs:222` 用的就是**同一条** `first_image` 规则——
正文里没有 `<img>` 就没有封面，且此后无任何补救路径。

真实库量化：`miniflux` 4714 篇中 **757 篇无封面（16%）**，其中 709 篇来自 feed 6（LINUX DO）。
`direct` 278 篇中 52 篇无封面（这 52 篇才进补全队列）。`[实测]`

## 1.2 成因 B：封面补全循环**饥饿** —— 一旦最新 20 条候选全失败，更老的候选在本进程内永不被尝试

`scheduler.rs:316-321` 每轮取 `LIMIT 20` 且 `ORDER BY published_at DESC`（`db/articles.rs:680`），
`scheduler.rs:374-377` **先把 URL 记进进程内 `tried`（负缓存）再发请求**，`:323-326` 在 `targets` 为空时
`sleep(60s); continue`（不推进窗口）。于是：最新 20 条一旦整批失败 → 之后每轮 `all` 还是那 20 条 → 全被 `tried` 过滤
→ `targets` 为空 → 死循环等待，**第 21 条及以后永远进不了窗口**。

真实库证据（route-R 实测 + 我复核）：52 条候选中**前 20 条全属 `kirikira.moe`**（published_at 同一秒），
该域实测 **403×3**；而第 27-48 位的 `hpx.tw` 实测 **200 且含合法 `og:image`**。
逐行转写后的仿真：18 轮 → **20 次请求 / 0 个封面 / 32 条从未被尝试**。`[实测]+[仅读码]`

## 1.3 成因 C：**只有画廊卡片走图片代理**，其余封面位全部直连 + `no-referrer` ⇒ 白名单式防盗链 100% 失败

代码事实（本轮逐点核对）：

| 封面渲染点 | 文件:行 | 是否走 `proxyImageUrl` |
| --- | --- | --- |
| 文章卡片缩略图 `card-cover-thumb` | `Timeline.tsx:374` | ❌ 直连 + `referrerPolicy="no-referrer"` |
| 播客卡片封面 `podcast-cover-box` | `Timeline.tsx:687` | ❌ 直连 + no-referrer |
| 播放条小封面 `player-cover` | `PlayerBar.tsx:216` | ❌ 直连 + no-referrer |
| 全屏播放器封面 | `PlayerBar.tsx:332` | ❌ 直连 + no-referrer |
| 灯箱大图 | `Overlays.tsx:338` | ❌ 直连（画廊路径下 src 已是代理后的 data: URL） |
| 侧栏 favicon | `Sidebar.tsx:231` | ❌ 直连（但**有** `onError` 兜底） |
| **画廊卡片** | `Timeline.tsx:565-573` | ✅ `proxyImageUrl`（唯一） |

白名单只有 3 个域（`imageProxy.ts:10-11`：`cdnfile.sspai.com`、`rssfile.sspai.com`、`*.doubanio.com`）。

**我对真实图床的实测**（每域多样本，三种 Referer，见 `tmp/audit-r4/adjudicate_hosts.py` 原始输出）：

```
cdnfile.sspai.com   n=12  无Referer: 403×12   origin: 200×12   文章URL: 200×12
img9.doubanio.com   n=8   无Referer: 418×8    origin: 200×8    文章URL: 200×8
img3.doubanio.com   n=6   无Referer: 418×6    origin: 200×6    文章URL: 200×6
img1.doubanio.com   n=6   无Referer: 418×6    origin: 200×6    文章URL: 200×6
cdn3.ldstatic.com   n=30  200×29 404×1（公开，不受 Referer 影响）
image.woshipm.com   n=6   全部 200（公开）      s.anyway.red n=6 全部 200（公开）
```

**真机 DOM 证据**（`s6_cover.mjs`，把上面的策略装进浏览器）：文章布局里同一组主机分别挂给源 14 与源 1，

```
文章卡片：card-cover-thumb  sspai → naturalWidth=0（失败）  doubanio → 0（失败）
                          ldstatic → 1（成功）            /dead/ → 0（失败）
          fetch_image 调用 = 0        ← 文章卡片完全不走代理
CDN 拦截统计：sspai403×2  doubanio418×3  dead404×2  ok×1
画廊卡片：s9 场景里点开画廊图 → 灯箱 src = data:image/png;base64,…  ← 同一张封面经后端代理后成功
          （s6 的 B 段用类名筛选封面 img 时命中 0 张——画廊卡片的 `<img>` 没有 className，
            属我探针选择器的问题，不作为结论；画廊走代理这一事实由代码 Timeline.tsx:565-573 与 s9 的 data: URL 双证）
```

⇒ **少数派（322 篇）与豆瓣（28 篇）的封面在文章列表/播客卡/播放器里必然破图，而同一张图在画廊布局里正常**。
这正是「部分封面获取不到」中最像「时好时坏」的一种。`[实测]`

> **订正 route-R 的一条结论**：route-R 用单个样本测得「sspai 无 Referer 即 200，故白名单注释已过时」，
> 并据此推翻。我以 12 个真实 URL 复测：**无 Referer 403×12/12，带 Referer 200×12/12** → 原结论不成立，
> 白名单对 sspai 仍然必要。**这也是本轮唯一一处我推翻子审计结论的地方。**

## 1.4 成因 D：死链/失效封面**没有任何纠正通道**

- `articles_without_cover` 的判据只是 `image_url IS NULL OR =''`（`db/articles.rs:676-679`），
  **已有非空 URL 就不再看**；两处写入都是 `COALESCE(image_url, ?1)`（`db/articles.rs:534`、`scheduler.rs:407`），
  只填空不校验存活。
- 卡片/播放器/灯箱**没有 `onError`**（对照：`Sidebar.tsx:230-237` favicon 有、`imageProxy.ts:71-73` 正文图有）。
- 实测死链比例：`cdn3.ldstatic.com` 30 个真实 URL 中 1 个 404（≈3.3% ⇒ 真实库 1092 篇里约 36 篇）；
  `cdk.linux.do/favicon.ico` 类 403 另有 5 篇。这些在界面上就是破图，无占位、无重试、无回写。`[实测]`

## 1.5 成因 E：`first_image` 盲取正文第一个 `<img>`，把头像/表情/favicon 当封面

`sanitize.rs:253-263` 无尺寸/角色启发式。真实库统计：`avatar` 类封面 118 篇、`favicon` 5 篇、
emoji sprite 358 篇（route-R 的 q* 探针统计）⇒ **481 篇的「封面」不是封面**。
当前只有 5 篇真的显示出来（其余所属源是社交/通知布局，那些布局不渲染封面），所以暂时表现为「偶尔一张怪图」。`[实测]`

## 1.6 成因 F：媒体字段解析缺口（有图不取）

`media:content` **缺 `type` 属性时被跳过**（而同函数的 enclosure 分支有扩展名兜底）；
`media:thumbnail` 的 `.uri` 不做相对路径解析、也不排除 `data:` URI。⇒ 一部分本来有封面的条目取不到图。`[仅读码]`

---

# 二、全功能矩阵（本轮核心交付）：跑到了什么、确证了什么

脚本与原始输出：`tmp/audit-r3/harness/s7a_matrix.mjs` … `s11_cleanup.mjs`（逐个可重跑，全部打印 ✅/❌ 与证据）。

## 2.1 读路径（s7a）——**全部通过**

| 编号 | 检查项 | 结果 | 证据 |
| --- | --- | --- | --- |
| F1 | 冷启动：骨架消失、无错误态、首批 IPC（folders/feeds/articles/counts 各 1 次） | ✅ | cards=14，`err=null` |
| F2 | 设置恢复：DB `themeMode/palette` → `<html>` 属性 | ✅ | `data-theme=dark data-palette=blue` |
| F3 | **5 布局 × 4 视图 = 20 组合**：渲染的卡片类与后端计数一致 | ✅ 20/20 | 集合级断言（可见 id 全属该布局），无回落、无串台 |
| F4/F5 | 范围=全部 / 分类（日常）：可见卡片全部属于该范围 | ✅ | 14/14 属该分类 |
| F6 | 范围=单源（feed 15，18 篇老文章） | ⚠️ GAP | 可见 0 vs 后端 18（P0-1 家族，已被 round-3 记录） |
| F7 | 点开文章 → 标读 + 渲染正文 | ✅ | `is_read 0→1` |
| F8/F9 | 快捷键 S 收藏切换 / J 移动选中并打开 | ✅ | `is_starred 1→0`；选中 3004→2999 |
| F10 | 右键菜单「收藏/取消收藏」 | ✅ | `is_starred 1→0` |
| F11 | 「显示: 全部/未读」在全部视图生效 | ✅ | 卡片 14→1 且全部为未读 |
| F12 | 排序切换 | ✅（局部） | 顺序变化；已知 P2-4：不重拉 |
| F13 | 侧栏「未读」角标 = 后端真实未读 | ✅ | 30 = 30 |

## 2.2 列表可达性（s7b B1）——**三个布局的用户到不了内容**

| 布局 | 可见卡片 | 后端 | 差 | 容器可滚动 | 判定 |
| --- | --- | --- | --- | --- | --- |
| 文章 | 14 | 1268 | 1254 | ✅ 可滚动 | 可继续加载 |
| 社交 | 12 | 1554 | 1542 | ✅ 可滚动 | 可继续加载 |
| **画廊** | 5 | 44 | 39 | ❌ 不可滚动 | **差 39 条到不了** |
| **播客** | 6 | 119 | 113 | ❌ 不可滚动 | **差 113 条到不了** |
| **通知** | 1 | 20 | 19 | ❌ 不可滚动 | **差 19 条到不了** |

机理与 round-3 §2.1 同源（首批 500 条不含这些布局的条目 → 空列表补拉只拉一页 → 容器不可滚动 ⇒ 死胡同），
而空态旁仍渲染提示「滚动加载更多」——**一个用户无法执行的提示**。这是本轮覆盖面里最影响日常使用的一条。`[实测]`

## 2.3 写路径与外围（s7b/s8/s9/s10/s11）——**除下述外全部通过**

| 功能 | 结果 | 落库/IPC 证据 |
| --- | --- | --- |
| 分类折叠/展开 | ✅ | `folders.collapsed 0→1` |
| 重命名分类（含确认弹窗） | ✅ | `folders.name → 日常改名`（且改的是目标分类 id=2） |
| 新建分类 | ✅ | folders 5→6 |
| 添加订阅源（标题/布局/AI 开关表单） | ✅ | feeds 16→17，`add_feed` IPC=1 |
| 编辑订阅源（改标题） | ✅ | `feeds.title → 审计源A改名`，`update_feed` IPC=1 |
| 右键删除订阅源 | ✅ | 行删除，`delete_feed` IPC=1 |
| 刷新单源 / 刷新全部 | ✅ | `refresh_feed` / `refresh_all_feeds` IPC + toast |
| 设置页 8 个页签 | ✅ 8/8 | 面板标题逐一对上 |
| 主题切换 | ✅ | DB `themeMode dark→light` + `<html data-theme=light>` |
| 阅读页字号滑杆 | ✅ | DB `fontSize 16→18` |
| 拖拽列宽 | ✅ | DB `listWidth 320→358`（松手落库；拖动期不落库＝N9 语义） |
| 缓存清理「清理 AI 缓存」/「清理旧文章」 | ✅ | `cache_cleanup(scope=ai)` / `(scope=articles)` + 确认弹窗 |
| OPML 导出 | ✅ | `opml_export` IPC + 「OPML 导出完成」 |
| 侧栏同步药丸 | ✅ | 打开设置并定位「同步」页 |
| 命令面板：开/关、命令项执行 | ✅ | 「打开设置…」真的打开了设置 |
| 灯箱：画廊点图 → 打开 → Esc 关闭 | ✅ | 打开时 src 是代理后的 data: URL |
| Ctrl+K / Ctrl+, / Esc 逐层关闭 | ✅ | 用 `.modal-overlay.open` 判定，逐层递减 |
| 播放器：播客卡片播放 → 播放条 + audio 挂载 → 关闭 | ✅ | `active=true`，`audio src=…mp3` |
| 滚动到正文底部标读（`markReadOnScrollBottom`） | ✅ | `is_read 0→1` |
| 失败可见性：注入 `set_read` 拒绝 | ✅ | toast「标读失败：注入的失败：set_read」，库内未变 |
| 同步页连接测试（空表单） | ✅ | 前端校验拦截并提示「请填写 Endpoint、用户名和密码」 |

## 2.4 P3-11 的真机复核（历史结论确认）

- 今天视图下「显示: 全部/未读」按钮**存在**（标签「全部」），点击后**列表 4→4 完全不变**，
  而侧栏「全部」数字 `1268 → 32`。收藏视图同样（8→8）。⇒ round-1/round-2 的 P3-11 在真机成立：
  按钮渲染范围（`activeViewFilter !== 'unread'`）比生效范围（`=== 'all'`）宽，用户看到的是「点了没反应但数字变了」。`[实测]`

## 2.5 本轮**未覆盖**的功能（如实登记，供下一轮补齐）

- 配置同步的真机往返（Gist/WebDAV 上传/下载/应用）：需真实凭据，本轮只验证了「未配置」态与入口存在。
- GitHub 设备流登录全流程（需要真实授权）。
- OPML **导入**（需要文件选择对话框）、音频真实播放（无音频源）、SMTC 系统媒体键、托盘/关闭流程、
  开机自启写注册表、系统通知权限申请——这些都要真实 Windows 会话，沿用前轮结论「未验证」。
- 「滚出列表标读」（`markReadOnScrollOut`）：虚拟列表下难以稳定构造滚动出视口，本轮未测得。
- 手动同步 `sync_phase` 的成功路径（后端为 mock，未连接真实服务）。

---

# 三、路线 M（数据库/schema）与路线 R（外围）的独立结论（我抽样复核过前提）

**路线 M · 数据库完整性**（`tmp/audit-r4/route-M/REPORT.md`，64 列 / 94 条 SQL）：

- **M-5（P1）**：列表查询 `ORDER BY COALESCE(published_at, fetched_at)` 是表达式排序 ⇒ `idx_articles_published`
  对 8 个列表变体**全部失效**，每次 `SCAN articles` + 临时 B 树。**我自己复核**：真实 schema 上
  `EXPLAIN QUERY PLAN` 实测 `SCAN a` + `USE TEMP B-TREE FOR ORDER BY`；且真实库 `published_at IS NULL` = 0，
  即 `COALESCE` 的兜底从未生效、却让索引失效。
- **M-7（P2）**：`sync_queue` 零索引 ⇒ 每次标读/标星的去重 `DELETE` 全表扫；`push_states_now` 里还 `SCAN articles`。
- **M-9（P2）**：一次「全部已读」= 1 SELECT + 1 UPDATE + 2N 次往返（真机 74 条 ⇒ ~150 次），全程持锁。
- **M-14（P2）**：v6→v7 后置回填在事务外、错误被 `let _` 吞、以 `user_version` 当完成标记 ⇒ 半途中断**永不重试**
  （真实库已跑完，属潜伏）。
- **M-1…M-4（P3）**：死列 4 个：`articles.enclosure_mime`（后端读、前端 `articleRowToEntry` 不映射）、
  `deduped_urls.kept_at`、`folders.created_at`、`feeds.created_at`。
- **M-21（P3）**：跨源同文副本只广播 `read` 不广播 `star` ⇒ 真实库 16 组同文中 1 组星标状态分歧。

**路线 R · 封面与外围**（`tmp/audit-r4/route-R/REPORT.md`）：除 §1 的封面成因外，
另报 `App.tsx:96-101` 的 `else if` 使「既抓新文章又有源失败」时失败提示永不显示（R-8）、
通知失败静默（`let _ = …show()`）、侧栏 ⚠ 提示「点击重试」但该 span 无可点的 onClick（R-10b）。

> 两条冲突已由我裁决：① sspai 白名单（见 §1.3 订正）；② 侧栏 ⚠「点击重试」——
> route-S（round-3）称「点击只是把范围切到该源」，route-R 称「该 span 无 onClick」。这两条**不矛盾**：
> `Sidebar.tsx:243` 的 `title` 在**外层可点区域**上（点整行会切范围），而 ⚠ 图标本身没有独立的重试 handler，
> 即「提示承诺重试、实际动作是切范围」——按 round-3 的表述（S-3）为准，R-10b 的「无 onClick」只是描述同一事实的另一面。

---

# 四、修复优先级（封面优先，附证据）

| 序 | 事项 | 依据 | 备注 |
| --- | --- | --- | --- |
| 1 | **封面补全饥饿**：负缓存改成「失败也可重试、窗口要推进」 | §1.2（52 候选中 32 条永不被尝试） | 最小改动：把 `tried` 插入移到成功之后，或 `tried` 只对「确定无 og:image」生效并保留失败重试 |
| 2 | **封面统一走代理**：`card-cover-thumb`/`podcast-cover-box`/`player-cover`/灯箱改调 `proxyImageUrl`（或给 `<img>` 加 `onError` 回退到代理） | §1.3（sspai 403×12、doubanio 418×20；DOM 实测 naturalWidth=0） | 白名单也可扩到实测需要的域 |
| 3 | **无封面不再「永久无解」**：miniflux 源纳入补全，或明确产品口径 | §1.1（757 篇 = 16%） | 需 owner 决策：是否为 miniflux 源也抓文章页 |
| 4 | **死链纠正通道**：`onError` → 占位图 + 标记待重取；补全判据加入「存活校验」 | §1.4（ldstatic ≈3.3% 404，永久破图） | 与 #2 同一处改动 |
| 5 | **封面质量启发式**：`first_image` 排除 favicon/avatar/emoji、要求最小尺寸 | §1.5（481 篇错图） | 可与 #3 一并做 |
| 6 | 三个布局的「不可滚动 + 提示不可执行」（画廊/播客/通知） | §2.2 | 与 P0-1 同源：空列表哨兵改可点击按钮，或空时续拉到出现内容 |
| 7 | M-5/M-7/M-9 的索引与往返优化 | 三·M | 表达式索引 `COALESCE(published_at, fetched_at)`（我已验证可建且计划变为 covering index） |
| 8 | 既有未决项（P0-1 口径、标读口径、P1-3/P1-4/P1-5、P2-15 等） | 前两轮报告 | 不变 |

---

# 五、证据索引

| 产物 | 内容 |
| --- | --- |
| `tmp/audit-r4/probe_cover_real.py` / `probe_cover_quality.py` / `probe_cover_hosts.py` / `adjudicate_hosts.py` | 真实库封面覆盖统计（只读）+ 真实图床防盗链实测（含原始状态码表） |
| `tmp/audit-r3/harness/mock_backend.py` | 忠实假后端：真实 schema、逐字 SQL、`fetch_image` 的 Referer 候选链、故障注入、IPC 日志（带时间戳） |
| `tmp/audit-r3/harness/drive.mjs` | Chrome/CDP 驱动 + Tauri IPC shim + 封面 CDN 策略拦截（`enableCoverCdnPolicy`） |
| `tmp/audit-r3/harness/s6_cover.mjs` + `s6_stdout.log` | 封面真机证据（文章卡片 vs 画廊卡片、naturalWidth、fetch_image 次数） |
| `tmp/audit-r3/harness/s7a_matrix.mjs` / `s7b_writes.mjs` / `s8_recheck.mjs` / `s9_final.mjs` / `s10_last.mjs` / `s11_cleanup.mjs` | 全功能矩阵六组（含各自的 `*_stdout.log` 与 `*_result.json`） |
| `tmp/audit-r4/route-M/REPORT.md` | 数据库/schema 完整性（64 列 / 94 条 SQL 计划 / 迁移与不变量） |
| `tmp/audit-r4/route-R/REPORT.md` | 封面链路与外围子系统 |
| 未完成 | 路线 N（同步引擎）与路线 Q（设置往返）两次派发均未产出（子代理会话未启动），本轮**没有**覆盖这两块 |

> `tmp/` 已被 `.gitignore` 忽略，不入库；业务代码本轮零改动（`git status -- src src-tauri tools` 为空）。
