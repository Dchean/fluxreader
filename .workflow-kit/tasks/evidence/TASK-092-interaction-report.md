# TASK-092 真机交互报告（封面图片位统一代理与失败回退 + 失效上报）

- 任务：TASK-092 / RUN-53130f3d2605478c958b31e4e68ea4a2（REQ-106 之③，前端）
- 取证环境：Chrome（CDP 驱动，脚本 `tmp/task-092/s092_covers.mjs`）+ 忠实假后端 `tmp/task-092/mock_backend_092.py`
  （`http://127.0.0.1:8893`，前端产物 `D:\fluxreader\dist`，合成库 `mock-27248.db`：folders=5 feeds=16 articles=3005 unread=74）。
  浏览器侧拦截模拟图床：sspai 无 Referer 返回 403、doubanio 返回 418、`/dead/` 返回 404、`/login-wall/` 返回 200 + HTML、`/slow/` 后端 fetch_image 延迟 1.8s。
- 数据来源：下文数值全部取自 `tmp/task-092/ui/s092_result.json`（运行日志 `tmp/task-092/ui/run.log`），未做估算。
- 运行次数说明：取证脚本共运行 5 次。首轮产物（原 12 条）备份在 `tmp/task-092/ui-run1/`；为补齐「灯箱代理路径、播客/播放条代理成功路径、深色下播放条/灯箱截图」缺口扩写脚本后，
  第 5 次运行成功，本报告以第 5 次为准：原 12 条 A1–D12 + 补充 A1b/A1c 共 14 条全部 PASS。运行结束后后端（pid 27248）与浏览器（pid 40116）已 `taskkill /T /F` 清理。
- 本次运行汇总：页面异常（含 unhandledrejection，两个会话合计）=`[]`；浏览器侧 CDN 拦截统计 `{"sspai403":2,"doubanio418":1,"ok":3,"dead404":3,"referers":[]}`。
- fixture：源 14「热门文章 - 日榜」前 8 篇封面按场景设定：sspai=2995、douban=2993、ldOk=2988、ldDead=2984、sspaiDead=2981、loginWall=2978、empty=2971、slow=2970；播客 podId=3001；画廊死链 galDead=[2637]；补跑 podOk=2698、galOk=3002、galTarget=2742。
- 截图（均在 `tmp/task-092/ui/`）：`TASK-092-covers-dark.png`、`TASK-092-covers-light.png`、`TASK-092-lightbox-failure-light.png`、`TASK-092-player-failure-light.png`、`TASK-092-player-proxy-ok-dark.png`、`TASK-092-lightbox-proxy-dark.png`。

## 结论总表

| 检查项 | 结论 | 截图 |
|---|---|---|
| A1.proxy-by-policy | PASS | TASK-092-covers-dark.png |
| A1b.proxy-podcast-player（补） | PASS | TASK-092-player-proxy-ok-dark.png |
| A1c.lightbox-route（补） | PASS | TASK-092-lightbox-proxy-dark.png |
| A2.no-overreach | PASS | TASK-092-covers-dark.png |
| A3.gallery-unchanged | PASS | （无专门截图，DOM 数据） |
| A4.no-layout-shift | PASS | TASK-092-covers-dark.png |
| B5.fallback-placeholder | PASS | TASK-092-covers-dark.png / -light.png |
| B6.theme-light-dark | PASS | TASK-092-covers-dark.png / -light.png |
| B7.no-retry-storm | PASS | （IPC 计数） |
| C8.report-idempotent | PASS | （IPC 计数 + 假后端库） |
| C9.shared-cover-handling | PASS | TASK-092-player-failure-light.png |
| D10.empty-cover | PASS | TASK-092-covers-dark.png |
| D11.non-image-bytes | PASS | TASK-092-covers-dark.png |
| D12.lightbox-failure | PASS | TASK-092-lightbox-failure-light.png |

## A1.proxy-by-policy — 需代理域名经 fetch_image 渲染为 data: URL

- 检查项：文章卡等图片位按 `lib/imageProxy.ts` 既有判定决定代理；需代理域名（sspai/doubanio）经 `api.fetchImage` 渲染为 data: URL。
- 操作路径：文章布局 → 展开分类 → 选源「热门文章 - 日榜」（源 14）。
- 观察：
  - sspai（2995）：img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，complete=true，naturalWidth=48，盒 82×58，圆角 6px，卡片高 131。
  - doubanio（2993）：img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，naturalWidth=48，盒 82×58。
  - IPC：fetch_image(sspai)=1、fetch_image(doubanio)=1，pageUrl=null（列表行不带 url，与真实 `list_articles(with_content=false)` 一致）。
  - 浏览器对这两个原始 URL 的直连请求数=0/0（修前为直连 403/418）。
- 截图：TASK-092-covers-dark.png
- 结论：PASS

## A1b.proxy-podcast-player（补）— 播客卡/迷你播放条/全屏播放器代理成功，三处共用一次取图

- 检查项：A1 在播客卡、迷你播放条、全屏播放器三处的代理成功路径；三处展示同一 cover 时只取图一次。
- 操作路径：深色新会话 → 播客布局 → 卡片（id=2698，sspai 正常封面）→ 点播放 → 展开全屏播放器。
- 观察：
  - 主题=dark。
  - 播客卡：src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，complete=true，naturalWidth=48，盒 60×60，圆角 6px。
  - 迷你播放条：placeholder=null；img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，naturalWidth=48，complete=true。
  - 全屏播放器：placeholder=null；img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，naturalWidth=48，complete=true。
  - IPC：fetch_image(该 URL)=1（pageUrl=null）；report_broken_cover(该 URL)=0。
- 截图：TASK-092-player-proxy-ok-dark.png
- 结论：PASS

## A1c.lightbox-route（补）— 灯箱按 imageProxy 判定取图

- 检查项：画廊代理成功后传入灯箱的 data: 直接显示、不再取图；传入需代理的原始 URL 时灯箱走 fetch_image，失败显示占位并按条目上报一次。
- 操作路径：深色会话 → 画廊 → 点代理成功卡（id=3002）→ Esc → 点 sspai 死链卡（id=2742，画廊代理失败后按修前逻辑直连原 URL）→ 灯箱 → Esc。
- 观察：
  - 代理成功卡进灯箱：open=true，placeholder=null，img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=direct（data: 直接显示），naturalWidth=48；新增 fetch_image=0。
  - 死链卡画廊形态：src=`https://cdnfile.sspai.com/dead/gallery-only.png`，naturalWidth=0。
  - 死链卡进灯箱：open=true，placeholder class=`lightbox-img cover-fallback` state=failed，img=null。
  - IPC：该 URL 的 fetch_image 进灯箱前/后=0/2。其中灯箱 CoverImage 发出 1 次（无 pageUrl）；另 1 次带 pageUrl=`https://art1.example/p/2742` 的来自画廊卡片自身——点开触发 get_article 水合出 item.url，画廊 useEffect 依赖 `[imageUrl, url]` 变化而重取，属画廊既有行为，本任务未改。
    点击前后时间线（dt 相对点击 ms）：get_article dt=1 → fetch_image dt=2（无 pageUrl）→ fetch_image dt=8（带 pageUrl）→ report_broken_cover dt=12。
  - report_broken_cover=`[{"articleId":2742,"url":"https://cdnfile.sspai.com/dead/gallery-only.png"}]`（1 次）。
  - Esc 后已关闭=true。
- 截图：TASK-092-lightbox-proxy-dark.png
- 结论：PASS

## A2.no-overreach — 不需代理的域名保持直连

- 检查项：不需代理的域名保持直连，不额外触发 fetch_image。
- 操作路径：同 A1（源 14 的 ldstatic 封面）。
- 观察：
  - ldstatic 正常图（2988）：img src=`https://cdn3.ldstatic.com/optimized/4X/f…`，route=direct，complete=true，naturalWidth=1，盒 82×82，卡片高 155。
  - IPC：fetch_image(ldstatic 正常/死链)=0/0。
  - 浏览器直连请求 ldstatic 正常图=1 次。
- 截图：TASK-092-covers-dark.png
- 结论：PASS

## A3.gallery-unchanged — 画廊卡片既有代理行为保留

- 检查项：画廊卡片仍由 `proxyImageUrl` 取图，不经新组件，行为逐字保留。
- 操作路径：切到画廊布局（源 1，与源 14 同一组封面主机）。
- 观察（画廊卡片 DOM）：
  - 3002：src=`data:image/png;base64,iVBORw0KGgoAAAANSUhEUg…`，naturalWidth=48，complete=true，noImage=false，fallback=false。
  - 2846：src=`data:image/png;base64,iVBORw0KGgoAAAANSUhEUg…`，naturalWidth=48，complete=true，fallback=false。
  - 2742：src=`https://cdn3.ldstatic.com/optimized/4X/f/7/4…`，naturalWidth=1，complete=true，fallback=false。
  - 2637（ldstatic 死链）：src=`https://cdn3.ldstatic.com/optimized/4X/dead/…`，naturalWidth=0，complete=true，fallback=false——仍是修前形态（直连 img、无回退类），画廊卡片本任务不改。
  - 2525：img=null，noImage=true（既有「无图」占位）。
  - 2412：src=`https://img.example/2412.jpg`，naturalWidth=0，complete=true，fallback=false。
  - IPC：fetch_image 两次，host 分别为 `cdnfile.sspai.com`、`img9.doubanio.com`；画廊直连请求=1。
- 截图：无专门截图（DOM 数据为准）。
- 结论：PASS（画廊死链仍显示为破图是画廊既有行为，属本任务 non-goal「画廊卡片行为不变」的直接结果，见文末备注）。

## A4.no-layout-shift — 代理未返回前先出占位，尺寸/圆角一致

- 检查项：代理未返回前先出占位，封面容器尺寸/圆角/比例与修前一致。
- 操作路径：选源 14 后 350ms 读「慢图床」卡片（id=2970，后端 fetch_image 1.8s 才返回），返回后再读一次。
- 观察：
  - 返回前：占位 class=`card-cover-thumb cover-fallback`，state=pending，盒 82×58，圆角 6px，卡片高 131；此时无 img。
  - 返回后：img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，naturalWidth=48，盒 82×58，圆角 6px，卡片高 131。
  - 对照：直连成功图（ldstatic 2988）盒 82×82、卡片高 155。
  - 说明：`.card-cover-thumb` 规则（width:82px; align-self:stretch; object-fit:cover）本任务未改，封面高度随行高/图片固有比例变化是修前既有行为；占位沿用同一类名，因而在同一卡片内返回前后盒尺寸与卡片高均不变（82×58 / 131 → 82×58 / 131）。
- 截图：TASK-092-covers-dark.png
- 结论：PASS

## B5.fallback-placeholder — 代理失败 / 直连失败显示占位

- 检查项：代理失败或直连失败时显示占位，无破图图标、不留空白、不撑破卡片。
- 操作路径：同 A1（源 14：ldstatic 死链 2984 = 直连 404；sspai `/dead/` 2981 = 代理三候选全 404）。
- 观察：
  - 直连死链（2984）：占位 class=`card-cover-thumb cover-fallback`，state=failed，盒 82×58，圆角 6px，背景 `rgba(45, 52, 65, 0.85)`，图标色 `rgb(122, 132, 148)`，hasIcon=true，img=null。
  - 代理失败（2981）：占位 class=`card-cover-thumb cover-fallback`，state=failed，盒 82×58，圆角 6px，背景/图标色同上，hasIcon=true，img=null。
  - 页面上 complete 且 naturalWidth=0 的封面 img 数=0（无破图）。
  - 卡片高 131/131（对照成功卡 131）。
- 截图：TASK-092-covers-dark.png、TASK-092-covers-light.png
- 结论：PASS

## B6.theme-light-dark — 占位在浅色/深色主题下都可读

- 检查项：占位复用既有视觉语言，浅色与深色主题下都可读（各一张截图）。
- 操作路径：深色主题（库 themeMode=dark）截图 → 改库 themeMode=light 重载（新会话）→ 选源 14 截图。
- 观察：
  - 深色：占位背景 `rgba(45, 52, 65, 0.85)`，图标色 `rgb(122, 132, 148)`，卡片背景 `rgba(35, 40, 50, 0.65)`。
  - 浅色：占位背景 `rgba(228, 234, 246, 0.95)`，图标色 `rgb(111, 122, 140)`，卡片背景 `rgba(238, 242, 249, 0.85)`。
  - 两者均取自 `--bg-card-hover` / `--text-tertiary`，与画廊「无图」占位同源。
  - 浅色会话封面位：sspai（2995）img src=`data:image/png;base64,iVBORw0KGgoAAAANSU…`，route=proxy，naturalWidth=48，盒 82×58；sspai 代理失败（2981）占位 state=failed，盒 82×58，hasIcon=true；ldstatic 死链（2984）img=null、占位=null、卡片高 113——其 cover 已被上一会话的上报经 `clear_article_cover_if_matches` 清空（direct 行），新会话按无封面渲染。
- 截图：TASK-092-covers-dark.png、TASK-092-covers-light.png
- 结论：PASS

## B7.no-retry-storm — 同一会话内失败图片不重复请求

- 检查项：同一会话内失败的图片不被重复请求。
- 操作路径：在源 14 与「少数派」之间来回切换 3 次（卡片卸载/重挂载）。
- 观察：
  - fetch_image 总数 5 → 5；新增的都是切到其它源时的封面，失败 URL 新增 0（三轮记录 0/1/1 均为其它源封面，非失败 URL）。
  - 浏览器对 ldstatic 死链的直连请求 1 → 1。
  - 重挂载后：sspai 代理失败卡（miniflux 行，库未清）= 占位 state=failed；ldstatic 死链卡（direct 行，库已被上报清空）img=null、占位=无（重新拉列表后该行已无 cover，或仍命中缓存出占位——两种都不发请求）。
- 截图：无（IPC 计数为准）。
- 结论：PASS

## C8.report-idempotent — 失败上报，同条目同 URL 只上报一次

- 检查项：失败时调用 `report_broken_cover(articleId, url)`；同一条目同一 URL 只上报一次。
- 操作路径：首轮渲染后读 IPC；再经 3 次切源重挂载后复读；查假后端库。
- 观察：
  - 首轮上报次数 死链/代理失败/非图片 = 1/1/1；重挂载 3 轮后 = 1/1/1。
  - 后端日志实参：`{"articleId":2981,"url":"https://cdnfile.sspai.com/dead/gone-cover.png"}`、`{"articleId":2978,"url":"https://cdnfile.sspai.com/login-wall/cover.png"}`、`{"articleId":2984,"url":"https://cdn3.ldstatic.com/optimized/4X/dead/dead-cover.png"}`（参数名 camelCase，符合 Tauri 命令契约）。
  - 后端返回 = `[true,true,true]`。
  - 库中 image_url：`[[2978,"https://cdnfile.sspai.com/login-wall/cover.png"],[2981,"https://cdnfile.sspai.com/dead/gone-cover.png"],[2984,null]]`——仅 source='direct' 行（2984）被清空，miniflux 行保持（后端 TASK-091 口径，本任务未改）。
- 截图：无（IPC 计数 + 假后端库为准）。
- 结论：PASS

## C9.shared-cover-handling — 播放条/全屏播放器与卡片展示同一 cover 时共用同一处理

- 检查项：播放条/灯箱展示同一 cover 字段时共用同一处理，不重复上报（任务定义口径）。本次取证覆盖播客卡/迷你播放条/全屏播放器三处展示同一 cover 的场景；灯箱共用同一 CoverImage 处理与按条目单次上报已在 A1c、D12 取证。
- 操作路径：播客布局 → 卡片（id=3001，doubanio `/dead/`）→ 点播放 → 展开全屏播放器。
- 观察：
  - 播客卡占位：class=`podcast-cover-box cover-fallback`，state=failed，盒 60×60，圆角 6px，背景 `rgba(228, 234, 246, 0.95)`，图标色 `rgb(111, 122, 140)`，hasIcon=true，img=null。
  - 迷你播放条：占位 class=`player-cover cover-fallback`，state=failed，w=42，img=null。
  - 全屏播放器：占位 class=`player-full-cover player-full-cover-fallback cover-fallback`，state=failed，w=200，img=null。
  - IPC：fetch_image(该 URL) 卡片后/播放条+全屏后 = 1/1（三处共用一次取图，不重复取图）；report_broken_cover = 1/1（articleId=3001，不重复上报）。
- 截图：TASK-092-player-failure-light.png
- 结论：PASS

## D10.empty-cover — cover 为空：不渲染 img、不代理、不上报

- 检查项：cover 为 null/空串时不渲染 img、不代理、不上报。
- 操作路径：同 A1（源 14 第 7 篇 image_url=NULL）。
- 观察：
  - 该卡封面位：img=null、占位=null（run.log 卡片封面位快照 `empty` 条目：found=true、img=null、placeholder=null、卡片高 113）。
  - 以其 id 发出的 fetch_image / report_broken_cover = 0 / 0。
  - 卡片 `.card-main-content` 子元素数=1。
- 截图：TASK-092-covers-dark.png
- 结论：PASS

## D11.non-image-bytes — 代理字节不是图片（登录页 HTML）：占位 + 上报，不注入 DOM

- 检查项：代理返回的字节不是图片时走占位 + 上报，不注入 DOM。
- 操作路径：同 A1（源 14：sspai `/login-wall/`（id=2978），图床返回 200 + HTML）。
- 观察：
  - 占位：class=`card-cover-thumb cover-fallback`，state=failed，盒 82×58，圆角 6px，背景 `rgba(45, 52, 65, 0.85)`，图标色 `rgb(122, 132, 148)`，hasIcon=true，img=null。
  - IPC：fetch_image=1；report_broken_cover(2978)=1（后端日志实参 `{"articleId":2978,"url":"https://cdnfile.sspai.com/login-wall/cover.png"}`，ok=true）。
  - DOM 中 data:text/* 的 img=0（HTML 字节未被注入为图片）。
- 截图：TASK-092-covers-dark.png
- 结论：PASS

## D12.lightbox-failure — 灯箱图片失败显示占位，关闭/Esc 行为不变

- 检查项：灯箱图片失败时显示占位，关闭/Esc 行为不变。
- 操作路径：画廊点击死链卡片（id=2637）→ 灯箱 → Esc → 再次打开 → 真实鼠标点击占位。
- 观察：
  - 打开：open=true，占位 class=`lightbox-img cover-fallback`，state=failed，480×360，icon=true，img=null。
  - Esc 后已关闭（仍打开=false）；再次打开 open=true、state=failed（缓存命中，不再请求）。
  - 真实鼠标点击占位后仍打开=false。
  - 该条目 report_broken_cover 上报次数：首开/再开后=1/1。
- 截图：TASK-092-lightbox-failure-light.png
- 结论：PASS
