# 四阶段性能测量 · owner 实机操作清单（TASK-128 版）

本清单是给**真人**在**真机**上跑的：Agent 只能跑浏览器 mock 烟雾（验证工具不崩），
大库/媒体/同步并发这些数字必须在装了应用、有真实数据的机器上采集。

工具：`tools/phase4_measure.mjs`（测量电池，Node）+ `tools/phase4_seed.py`（规模数据注入/还原）。
本清单里的每条判定线都只对**当次受测条件**成立——报告自带 `conditions` 与 `conditionsBySection`（每段一份），
引用数字时必须连条件一起引用。

> ## ⚠ 数据写入警告（跑之前必读，TASK-128 R1 修正）
>
> 电池**不是全只读的**：为取「同步在飞」样本，`lockWaitProxy` 段会通过命令面板点一次
> **「刷新全部订阅源」**，那是一次**真实手动同步**：
> `triggerManualSync` → `syncPhase('feeds'/'states')` + `refreshAllFeeds` + `reloadFromBackend`，
> 也就是**联网抓取你在应用里配置的全部订阅源、写入本机 DB、并可能把状态推给已连接的后端**。
> 其余各段（切换/搜索/滚动/长会话/各布局内存）只做点击、滚动、读堆与 IPC 查询，不写应用数据；
> 电池本身不碰 DB 文件（备份/注入/还原都是 `phase4_seed.py` 的事）。
>
> 不希望这次同步发生，二选一：
> 1. 加 `--no-sync-probe` 跑电池：锁等待段**不点**刷新，只保留空闲态采样 + 纯 IPC 对照，
>    `syncInFlight` 记 `skipped`，报告里也不会有同步在飞态的锁等待数字；
> 2. 先在应用里**断开后端**（只去掉向后端推送/拉取；同步仍会写本机 DB），或干脆断网（副作用是该段会出现抓取失败）。
>
> 无论哪种：跑完照旧按第 ⑤ 步还原真实库。

---

## 0. 前置条件（缺一条就不要开始）

1. **副屏**：按用户显示器规则，应用窗口必须放副屏（先探测主副屏：
   `powershell -c "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::AllScreens"`，
   `Primary=True` 是主屏，禁止占用）。测量期间不要在主屏做重活，否则帧率数据会串味。
2. **应用已安装且能启动**（v0.17.0 或更高；本机没有 MSVC 链接器，不能自己编 Rust——用 CI 产物或发布包）。
3. **Node ≥ 20**（本工具用 `fetch`/`WebSocket` 内置实现，无第三方依赖）。
4. **Python 3**（只用于 `phase4_seed.py`）。
5. **磁盘空间**：备份 + 注入 5 万行大约几百 MB；`tmp/` 下要留得下。
6. 关闭会干扰内存统计的东西：其它 WebView2 应用（Teams/VS Code 的 Electron 不算 WebView2，无妨）、
   正在跑的下载/编译任务。**注意**：本机会有很多 `msedgewebview2.exe`（实测 47 个），
   工具已按「命令行含 `com.fluxreader.app`」过滤，只统计被测应用自己的 WebView2。

---

## 1. 全流程（按顺序执行，中途失败先看第 5 节）

```bash
# ① 备份（强制，先做；不做备份不要往下走）
#    注入前建议也完全退出应用（应用的活动连接会和注入互相干扰：它可能读不到新行）——
#    脚本检测到 FluxReader 在跑会警告但不阻止；这里退出后，② 再启动即可。
python tools/phase4_seed.py --info              # 先看清楚当前库长什么样（只读）
python tools/phase4_seed.py --seed 50000 --media --layouts --media-ratio 0.6
#    --seed N      注入 N 行（会先自动备份到 tmp/phase4/real-db-backup）
#    --media       给部分行写真实图片/音频列（picsum.photos / SoundHelix）
#    --layouts     五种内容布局都铺上卡（轮转改写 feeds.layout；源不够会自动补 1 个种子分类）
#    想换图源：--image-url "http://<你的内网图床>/{seed}/800/450"；换音频：--audio-url "..."

# ② 启应用（必须带远程调试端口）
#   Windows 命令行：
set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
"%LOCALAPPDATA%\FluxReader\FluxReader.exe"        # 或开始菜单启动（先设好上面的环境变量）
#   PowerShell：
#   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'; & "$env:LOCALAPPDATA\FluxReader\FluxReader.exe"
#   验证端口通了：
curl http://127.0.0.1:9222/json/list               # 应返回 JSON，里面有 type=page 的条目

# ③ 跑测量电池（项目根目录；--out 用固定文件名，后面的查看命令照抄即可）
node tools/phase4_measure.mjs --out tmp/phase4/measure-run.json
#   可选 --reload：先刷新页面再测量（重复测量时保证同一起点）
#   可选 --no-sync-probe：锁等待段不点「刷新全部订阅源」（跳过那一次真实同步；见顶部警告）
#   换端口：T059_CDP_PORT=9223 node tools/phase4_measure.mjs --out tmp/phase4/measure-run.json
#   换进程名（例如开发版 exe 叫别的）：T128_APP_PROCESS=你的exe名
#   想按规模/日期区分多份报告：把上面这条命令的 --out 改名后，④ 里的路径要跟着改
#   （--out tmp/phase4/measure-<规模>-<日期>.json 这种带尖括号的写法不能直接粘贴）

# ④ 看报告（控制台会直接打印 conditions / conditionsBySection / verdicts / 结论行；JSON 里有全部明细）
node -e "const r=require('./tmp/phase4/measure-run.json'); console.log(r.conditionsBySection); console.log(r.verdicts)"

# ⑤ 还原真实库（测量结束立刻做）
#    先完全退出 FluxReader（确认进程结束）——运行中的 SQLite 连接持有 .db/-wal/-shm 句柄，
#    覆盖可能静默失效甚至损坏库，而还原的 SHA256 只校验目标文件、这种破坏下必然通过。
tasklist /FI "IMAGENAME eq FluxReader.exe"        # 应回答：没有运行的任务匹配指定标准
#    确认真没了，再还原：
python tools/phase4_seed.py --restore
#    脚本自己也会检测：发现 FluxReader 仍在运行即拒绝还原（除非加 --force，危险，不建议）

# ⑥ 校验还原（必须做；看到原规模才算收工）
python tools/phase4_seed.py --info
```

**还原纪律**：`--seed` 前强制备份三件套（`.db` / `-wal` / `-shm`）并写 SHA256 清单；
`--restore` 逐字节拷回并复核哈希，哈希不一致会直接报错退出（不要继续，先查杀软/云同步）。
**`--restore` 前必须先完全退出应用**：应用在跑时它的 SQLite 连接仍持有这三个文件的句柄，
覆盖可能被静默拒绝或让 WAL 恢复把库改坏——而 SHA256 校验的是「拷过去的目标文件」，
这种破坏下它照样通过。脚本会检测 `FluxReader.exe` 是否在跑并在命中时拒绝还原
（开发版 exe 名不同时用 `T128_APP_PROCESS=你的exe名`，与电池同一口径）。
`--info` 是只读的（连接上开 `PRAGMA query_only=1`），它不写任何东西。

---

## 2. 报告里有什么（字段含义）

| 字段 | 含义 | 口径 |
| --- | --- | --- |
| `conditions` | 电池**起点**的受测条件 | 布局/已加载实体/可见卡片/媒体开关/网络/同步是否在飞/版本 |
| `conditionsBySection[<段名>]` | **每段自己的**条件快照 | 结论行引用的是本段那一份（如 `longSession`、`memoryByLayout`、`lockWaitProxy`）。锁等待段自己会触发同步，借用起点那份会印出与 JSON 自相矛盾的「同步在飞」——所以别再拿 `conditions` 当所有段的条件 |
| `measures.switchLatency[]` | 切换延迟 | `firstResultMs`=点击→**卡片 id 集合变化**（新查询落地）；`stableMs`=其后**连续 3 帧 rAF 无 DOM 变更**；另有 `cardsBefore/cardsAfter`。超时只写 `timedOut: true`，**不给毫秒数** |
| `measures.search` | 搜索分段 | `openMs`=Ctrl+K→浮层打开；`resultMs`=键入→**搜索浮层的「文章」分组**（`[role=group][aria-label=文章]`，标签查不到时回退到最后一个分组）内出现含 token 且**该分组**结果数变化的结果节点；命令/订阅源分组同样渲染 `.cp-item`，不算数。`ownership.tokenAbsentBeforeTyping` 是键入前的硬断言（读的是**浮层打开之后**的条目文本），为 `false` 时本段 `invalid`、不给 `resultMs`。`negativeProbe` 用「确定不存在于数据的 token」反证判据不是笼统的 body 文本匹配 |
| `measures.scroll.positions[]` | 深页滚动 | 每档先滚到位并等虚拟列表静默 300ms，再采 3 秒 rAF 帧间隔；`top/mid/bottom` 三档各一组 p50/p95/max/jank |
| `measures.memoryByLayout[]` | 各布局内存 | `heapUsedMB`=**JS 堆**；`rss`=**进程 RSS**（含 Rust+WebView2+图片解码）。两个口径不能互相换算。每条自带 `conditions`/`syncInFlight`（该布局测量时刻）；**测量时渲染树被卸载的布局**（`appCrashedHere`）`heapUsedMB=null` + `heapInvalidReason`，原始读数留在 `heapUsedMBBeforeCrash` 备查 |
| `measures.longSession` | 长会话 | 8 轮「切布局→等稳定→滚一屏」后的 JS 堆/RSS 首末增量与 DOM 节点增量，外加**逐轮堆序列** `perRoundHeap[]`（每轮 `jsHeapMB`/`domNodes`，**不强制 GC**，只看趋势）与 `perRoundHeapMonotonicSteps/perRoundHeapSteps`（相邻递增步数/总步数）。中断则 `growthMB: null` + `growthInvalidReason`（增量作废，不输出可能被误读的数字） |
| `measures.lockWaitProxy` | 锁等待**代理** | `sync_queue_stats` IPC 往返采样 20 次的 p50/p95/p99，空闲态与同步在飞态各一组；另附纯 IPC 对照（读版本号）。`syncProbeTriggered` 标明这次是否真的点了「刷新全部订阅源」（= 一次真实同步，见顶部警告；`--no-sync-probe` 时该态记 `skipped`） |
| `appHealth` / `pageErrors` | 应用是否还活着 | 渲染树被卸载会记录崩溃点、页面错误与是否自动重载恢复 |
| `verdicts` / `conclusions` | 判定与结论行 | 每条结论都带**本段条件**前缀，不能脱离条件引用 |

### 数字怎么看（判定线）

| 指标 | 判定线 | 越线了说明什么 |
| --- | --- | --- |
| `switchLatency.firstResultMs` | > 1000 ms | 切换后新结果迟迟不落地（查询/分页/IPC 慢） |
| `stableMs` | 没有硬线 | 渲染抖动时长；比 `firstResultMs` 大很多说明「结果到了但界面还在动」 |
| `search.openMs + resultMs` | > 1000 ms | 搜索全链路慢（FTS 或渲染）。仅当 `ownership.tokenAbsentBeforeTyping=true` 且命中落在文章分组时该数字才可用 |
| `scroll.positions[*].p95` | > 50 ms | 该位置滚动掉帧；**注意**只代表被测位置与数据规模 |
| `longSession.growthMB` + `domNodesDelta` | **首末增量为显著正值，且 DOM 节点同向增长** | 长会话缓存无界（审计点名 `entries` 持续追加、`entryMutationVersion` 无淘汰）。首末两个点不能证明「单调」——趋势请看 `perRoundHeap[]` 的逐轮序列与 `perRoundHeapMonotonicSteps/perRoundHeapSteps` |
| `memoryByLayout[*].heapUsedMB` | 两档规模间超线性 | 需同条件跑两档规模（如 20k 与 50k）再比较，单档不能下结论。`null`（失效/崩溃）的布局不参与比较 |
| `lockWaitProxy.syncInFlight.p95` | > 50 ms（代理线） | 可能有锁等待，**但这是 IPC 往返 + 单连接查询之和**，要确认必须上 Rust 侧计时；记数时连同 `conditionsInFlight.syncInFlight` 一起看 |

---

## 3. 哪些是代理指标（不要当成事实）

- **锁等待**：`sync_queue_stats` 的 IPC 往返 = 序列化 + IPC + 建连接 + 查询 + 可能的锁等待，
  **不是**数据库锁计时。报告里 `note` 已标注；要直接锁计时需要 Rust 侧埋点（本卡不做）。
- **滚动帧间隔**：程序化 `scrollTop` 推进 + `requestAnimationFrame` 采样，
  **不等于**真实滚轮/触控输入的手感延迟，也不覆盖惯性滚动。
- **进程 RSS**：包含 Rust 堆、WebView2、GPU 进程、图片/音频解码缓冲；
  **JS 堆只看渲染进程里的 JS 对象**。两者不能相减，也不要用 RSS 去验证 JS 堆的结论。
- **`conditions.articlesLoaded`**：来自侧栏角标（当前布局口径），mock/无 IPC 时不代表全库——
  报告另给 `loadedItemsEstimate`（虚拟列表总高 ÷ 估算行高）作量级参考。
- **`cardsVisible`**：虚拟化只挂载视口附近约 30 张，「卡片数少」不代表「数据少」。
- **长会话的 `perRoundHeap[]`**：逐轮读数**不强制 GC**，里面混着尚未回收的垃圾，只看趋势形状；
  `growthMB` 才是首末两点（两次都先 `HeapProfiler.collectGarbage`）的可比增量。两者不要混读。

---

## 4. 每次测量都记下来的东西（写进你的记录）

规模（`--info` 的 `articles_by_layout` 与 `media_coverage`）、应用版本、窗口/副屏配置、
是否联网、同步是否连着后端、跑的是哪一档判定线。
报告里对应的是 `conditions`（电池起点）与 `conditionsBySection[<段名>]`（每段开始那一刻）——
**引用某段的数字就用那一段的条件**，两次测量的数字只有在这些条件相同时才可比。

---

## 5. 出问题怎么办

| 现象 | 处置 |
| --- | --- |
| 任何一步失败、或你中途想停 | **先退出应用（确认进程结束）再还原**：`python tools/phase4_seed.py --restore`，然后 `--info` 确认回到原规模 |
| `--restore` 拒绝执行（提示 FluxReader 仍在运行） | 这是保护：先完全退出应用（窗口关掉后再用 `tasklist /FI "IMAGENAME eq FluxReader.exe"` 确认进程结束），再重跑 `--restore`。`--force` 只在明知风险（应用活动连接下覆盖可能静默失效或损坏库）时用 |
| 报告里出现 `fatal`（找不到页面） | 应用没带调试端口启动：关掉应用 → 设好 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` → 重启 → `curl http://127.0.0.1:9222/json/list` 确认 |
| 某段只有 `skipped` / `timedOut` | 看 `skipped`/`reason` 字段（如 `not-tauri`=纯浏览器、`no-timeline-scroller`=列表不可滚、`layout-button-not-found`=该分类被折叠、`no-sync-probe`=锁等待段按开关跳过同步触发）。**按原样记录，不要手填数字** |
| 搜索段 `invalid: token-present-in-palette-before-typing` | 选中的 token 键入前就出现在浮层条目里，命中证明不了是新查询结果：报告不给 `resultMs`，如实记下并换数据/换 token 重跑（这正是不限定分组时会误判的那条路径） |
| `appHealth.alive=false` | 应用渲染树中途崩了（报告里有崩溃点、页面错误与是否自动重载）。崩溃后的段会跳过或标注 `postRecoveryReload`——**崩溃前与重载后的堆数字不可混读**；崩溃布局的 `heapUsedMB` 记为 `null` |
| `--restore` 报哈希不一致 | 停下，别继续测量：先查杀软/云同步/文件锁，再从 `tmp/phase4/real-db-backup` 手工比对 |
| 注入时提示「检测到 FluxReader 正在运行」 | 注入本身不阻止，但应用的活动连接可能读不到新行：建议先完全退出应用再 `--seed` 注入，注入完再启动应用测量 |
| 注入失败 | 脚本会 `rollback` 并退出（真实库未受损）；看报错再决定 |
| 只想看库现在什么样 | `python tools/phase4_seed.py --info`（只读，安全） |

---

## 6. 已知观察（本卡测试期间发现，供 owner 判断，不属本卡范围）

在**浏览器 mock 模式**（无 Tauri IPC、`npm run dev` + 无头 Chrome）下点「通知」布局，
应用会崩渲染树：`The result of getSnapshot should be cached to avoid an infinite loop`
→ `Maximum update depth exceeded`（报错组件 `<NotifCard>`）。现象与测量工具无关
（不注入测量库、直接点按钮同样复现）。它会让「遍历五布局」的测量段中断，
所以工具把遍历放在最后一段、并在中断时显式标注（`appHealth.crashes`）。
真机 Tauri 数据下是否复现未在本卡验证——**若实机也复现，这是一个独立缺陷，另开卡处理**。
