# TASK-059 UI 交互报告（Endpoint 只填域名）

- 任务：TASK-059（批次 BATCH-6108ba756ba5490f924879b7f22b2f18）
- UI 契约：`.workflow-kit/docs/UI-CONTRACT-REQ-059.md`
- 日期：2026-09-19（本地）
- **前置事故**：本轮实机验证曾误写用户真实库，已逐字节还原；
  经过、证据与防再犯措施见 [TASK-059-incident-real-db-write.md](TASK-059-incident-real-db-write.md)。

## 验证方式（实机，非设计稿）

- 应用：**debug 构建的真实应用**（`src-tauri/target/debug/app.exe`），
  前端经 Vite dev server（debug 构建的 `devUrl`，5173）加载；
- 后端：`tools/mock_greader_ui_server.py --layout freshrss`
  ——本地协议服务端，**只**在 `/api/greader.php`（GReader）与 `/api/fever.php`（Fever）
  提供服务，**根路径一律 404**，精确复现真实 FreshRSS 的布局。
  启动自检：`POST /accounts/ClientLogin → 404`、`POST /api/greader.php/accounts/ClientLogin → 401`、
  `GET /api/fever.php?api → 200`、`GET /fever/?api → 404`（与 2026-09-18 对 demo.freshrss.org
  的实测结论逐条一致）；
- 驱动：`tools/t059_ui_e2e.mjs`，全程经 CDP（WebView2 `--remote-debugging-port=9222`）
  在页面内执行 JS，**真实点击真实控件**（设置中心 → 同步页签 → 输入框 → 「测试连接」），
  不注入任何操作系统级键鼠事件；
- 凭据：mock 只接受 `demo` / `demo-pass`，密码不符即 401（用于验证「凭据错不得报成地址错」）。

## 场景与实测结果

| # | 场景 | 实测 toast / 观察 | 结论 |
| --- | --- | --- | --- |
| A | 文案（深色主题） | desc：`只填域名即可：FreshRSS / Miniflux 自动适配`；placeholder：`如 https://demo.freshrss.org` | 通过 |
| S1 | **只填域名** + FreshRSS 形态（GReader） | `已连接：demo（2 个订阅）`；输入框回显 `http://127.0.0.1:8901`（= 用户原样输入，未回显解析出的后缀） | 通过 |
| S2 | 「保存并同步」真实拉取 | `已连接：demo（2 个订阅）`，随后侧栏 `全部 6 / 未读 6`（2 源 × 2 篇被真实拉入） | 通过 |
| S3 | 旧用法：已填完整路径 `…/api/greader.php` | `已连接：demo（2 个订阅）` | 通过（向后兼容） |
| S4 | **凭据错**（密码故意填错） | `连接失败：ClientLogin → 401 Unauthorized（已定位 API：http://127.0.0.1:8901/api/greader.php）` | 通过（未误报为「找不到 API」） |
| S5 | Fever 协议 + 只填域名（FreshRSS 形态） | `已连接：demo（1 个订阅）` | 通过 |
| S6 | 浅色主题下同一操作 | `已连接：demo（2 个订阅）`；desc 与 placeholder 完整可读 | 通过 |

## 后端请求序列（探测行为的直接证据）

`mock_greader_ui_server.py` 记录每一条请求（含被 404 拒绝的探测），本轮 27 条，关键片段：

```text
# S1 只填域名（FreshRSS 形态）：有界探测 2 个候选后命中
REQ POST /accounts/ClientLogin                  -> 404     ← 候选1：根路径无 API
REQ POST /api/greader.php/accounts/ClientLogin  -> 200     ← 候选2：命中
REQ GET  /api/greader.php/reader/api/0/subscription/list   -> 200

# S2 保存并同步：首轮探测后落库缓存，随后各同步阶段**不再探测**
REQ POST /accounts/ClientLogin                  -> 404
REQ POST /api/greader.php/accounts/ClientLogin  -> 200
REQ GET  /api/greader.php/reader/api/0/subscription/list   -> 200
REQ POST /api/greader.php/accounts/ClientLogin  -> 200     ← 缓存命中：直接打解析出的地址
REQ GET  /api/greader.php/reader/api/0/tag/list            -> 200
REQ POST /api/greader.php/accounts/ClientLogin  -> 200     ← 同上（无 404 探测）
REQ GET  /api/greader.php/reader/api/0/stream/items/ids    -> 200
REQ POST /api/greader.php/reader/api/0/stream/items/contents -> 200
REQ GET  /feed1.xml -> 200 / GET /feed2.xml -> 200

# S4 凭据错：候选2 返回 401 即**立即停止**，不再试其它候选
REQ POST /accounts/ClientLogin                  -> 404
REQ POST /api/greader.php/accounts/ClientLogin  -> 401     ← 停下并如实报凭据错

# S5 Fever + 只填域名（FreshRSS 形态）：同样有界探测后命中
REQ POST /fever/?api&api_key=…                  -> 404
REQ POST /api/fever.php?api&api_key=…           -> 200
REQ POST /api/fever.php?api&api_key=…&feeds     -> 200
REQ POST /api/fever.php?api&api_key=…&groups    -> 200
```

## 逐条核对 `ui_checks`

| # | 核对项 | 结果 | 实测 |
| --- | --- | --- | --- |
| 1 | desc 改为「只填域名」 | **通过** | `只填域名即可：FreshRSS / Miniflux 自动适配` |
| 2 | placeholder 给纯域名示例 | **通过** | `如 https://demo.freshrss.org` |
| 3 | 纯域名 + FreshRSS ⇒ 连接成功 | **通过** | S1：`已连接：demo（2 个订阅）` |
| 4 | 纯域名 + Miniflux ⇒ 连接成功 | **通过（Rust e2e）** | 本轮 HTTP 层用 mock 端口模拟 FreshRSS 形态（同一端口无法同时是两种形态）；Miniflux 形态由 `endpoint_autodetect_e2e` 的 `bare_domain_resolves_miniflux_layout` 覆盖 |
| 5 | 旧用法（完整路径）仍可用 | **通过** | S3 一次成功；请求序列显示只发 1 次 ClientLogin（首个候选即命中，无多余探测） |
| 6 | 回显仍是用户填写的原始值 | **通过** | S1 回显 `http://127.0.0.1:8901`，未回显解析出的 `/api/greader.php` |
| 7 | 凭据错报凭据错 | **通过** | S4：`ClientLogin → 401 Unauthorized（已定位 API：…）`，且断言 `误报为找不到API = false` |
| 8 | 地址确实不可达/找不到 API 时如实说明 | **通过** | e2e 覆盖「全部候选 404 ⇒ 『在该地址下找不到 … API』」；Rust 侧 `missing_path_vs_bad_credentials_are_distinguished` |
| 9 | 深色主题完整可读、不截断 | **通过** | 见 `TASK-059-ui-dark-bare-domain-connected.png` |
| 10 | 浅色主题完整可读、不截断 | **通过** | 见 `TASK-059-ui-light-bare-domain-connected.png` |
| 11 | `测试连接`/`保存并同步` 禁用态与加载态不受影响 | **通过** | 契约外行为未改动；驱动仍能正常点击两按钮（点击失败会报 `DISABLED:`） |
| 12 | 协议下拉（FluxDropdown）不受影响 | **通过** | S5 经真实下拉切到 Fever 并连通 |

## 证据文件

| 文件 | 内容 |
| --- | --- |
| `TASK-059-ui-dark-bare-domain-connected.png` | 深色 · 同步页 · 只填域名连通（toast `已连接：demo（2 个订阅）`） |
| `TASK-059-ui-light-bare-domain-connected.png` | 浅色 · 同一操作 |
| `TASK-059-ui-dark-bad-credential.png` | 深色 · 凭据错提示（如实报 401，未说成填法问题） |
| `TASK-059-ui-e2e-result.json` | 全部场景原始结果 + 真实库备份/还原哈希 + 后端请求序列 |

**主题标签已硬校验**：截图前断言 `data-theme` 与实测背景色亮度（深色要求
`rgb(30,34,42)`、浅色要求 `rgb(255,255,255)`），不符即中止且不落盘——
首轮实测确实产出过「名为 dark 实为 light」的截图，故改为强制校验（见下节）。

## 自查发现并修正的两个问题（如实记录）

### 问题 1：首轮深色截图实为浅色

首轮 S1 截图命名为 `…-dark-…png`，**实际渲染为浅色**。核对方式不是看文件名，
而是在运行中的应用里读 `getComputedStyle`：浅色下 `.settings-modal` 背景
`rgb(255,255,255)`，深色下 `rgb(30,34,42)`。修正：驱动改为**每次都显式设定主题 +
截图前硬校验主题与背景亮度**，并在结果 JSON 里记录实测值。

### 问题 2：Endpoint 说明被 `-webkit-line-clamp` 截断

首版文案 `只填域名即可（如 https://demo.freshrss.org）：FreshRSS 与 Miniflux 会自动适配`
在实机里显示为 `只填域名即可（如 https://demo.freshrss.org）：FreshRSS 与…`。

在运行中的应用里量取该节点（`.setting-card-text p`）：

```text
boxWidth = 234px   line-clamp = 2   clientHeight = 35px   scrollHeight = 52px  → clamped = true
```

即**该说明区只有两行**，超出的部分被截断成「…」。顺带量到：**TASK-057 的旧文案
需要 4 行（69px vs 35px）**，也就是**一直在被截断**（用户读不到后半句「FreshRSS 填 API 路径」）——
这属于既有缺陷，本任务一并修掉。

改为 `只填域名即可：FreshRSS / Miniflux 自动适配`，实测：

```text
clientHeight = 17px   scrollHeight = 17px   → 单行完整显示，未截断
```

并在前端回归里加了**长度预算断言**（列入 `(e1)`），防止再写出会被截断的说明。

## 未覆盖 / 限制

- **UI 证据与终版候选的时间差（如实说明）**：三张截图与交互结果 JSON 采集于
  **独立审查第 1 轮之前**的候选（`63ca6a97…`）。第 1 轮 FAIL 后的修复**只动了后端**：
  `fever.rs` 的写路径改用 `api_entry()`、mock 的前缀判定收紧、
  「找不到 API」消息补上 `HTTP 404`。这些**不改变设置页任何文案的渲染**，
  也不改变截图所展示的两条路径（成功 toast 与 401 凭据 toast）的文本——
  受影响的只有「全部候选 404」那条消息，而它不在任何截图里。
  故截图对终版候选仍然有效；此处不重采，是为了**避免再次改动用户真实库**
  （重采必须驱动「保存并同步」）。
- **Miniflux 形态未做实机截图**：同一 mock 端口无法同时呈现两种布局；
  本轮实机验证的是 FreshRSS 形态（即用户实测出问题的那一种），
  Miniflux 形态由 Rust e2e（`bare_domain_resolves_miniflux_layout`）覆盖。
- **窄窗口未另测**：契约本次的视觉要求是「深/浅两套主题下文案完整可读、不截断、不溢出」，
  已按两主题取证；Endpoint 卡片在默认窗口下未溢出（desc 单行、placeholder 余量充足）。
- **Fever 的 `auth` 失败未做实机截图**：`auth=0` 的拒绝路径由 Rust e2e
  （`fever_still_rejects_bad_auth`）覆盖，未走实机。
- **真实库写入事故**：见第 1 节链接的事故记录；本轮已还原并加装强制备份/校验。