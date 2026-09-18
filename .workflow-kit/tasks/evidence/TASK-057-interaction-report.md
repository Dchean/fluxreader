# TASK-057 UI 交互报告（Endpoint 填法指引与失败提示）

- 任务：TASK-057（批次 BATCH-68ca4c8d0dff468fb49faef5eea686fb）
- 授权：DEC-user-testing-bugs-20260918（owner 选择『不改探测逻辑，只改文案与错误提示』）
- UI 契约：`.workflow-kit/docs/UI-CONTRACT-REQ-057.md`
- 日期：2026-09-18

## 验证方式（实机，非设计稿）

应用以 `cargo build`（debug）编译后启动，前端经 Vite dev server（`localhost:5173`）加载。
界面驱动**全程使用 CDP（WebView2 `--remote-debugging-port=9222`）在页面内执行 JS**，
**未注入任何操作系统级键鼠事件**（不打扰用户其它窗口）。

截图由 `Page.captureScreenshot` 取自**运行中的应用**，非设计稿或静态渲染。

## 证据文件

| 文件 | 内容 |
| --- | --- |
| `TASK-057-ui-dark-sync-tab.png` | 深色主题 · 设置 → 同步 · 后端 Endpoint 卡片（新文案与 placeholder） |
| `TASK-057-ui-light-sync-tab.png` | 浅色主题 · 同一界面 |
| `TASK-057-ui-dark-404-hint.png` | 深色主题 · 填**纯域名** `https://demo.freshrss.org` 点「测试连接」→ 可操作的 404 提示 |

## 逐条核对 `ui_checks`

| # | 核对项 | 结果 | 实测读数 |
| --- | --- | --- | --- |
| 1 | desc 同时给出 Miniflux 与 FreshRSS 两种填法 | **通过** | 渲染文本：`后端 EndpointMiniflux 填站点根（如 https://reader.example.com）；FreshRSS 填 API 路径（如 https://demo.freshrss.org/api/greader.php）` |
| 2 | placeholder 反映真实填法（不再只给 Miniflux 形式） | **通过（经两次宽度修正）** | 最终 `如 https://主机/api/greader.php`（内容盒 219px 下实测 162.09px，余量 56.91px，截图中完整可见） |
| 3 | 纯域名保存失败时提示**可操作** | **通过** | 对真实 FreshRSS 实例实测 toast：`连接失败：ClientLogin → 404 Not Found（该地址下没有 GReader API：请确认 Endpoint 是否需指向 API 路径。Miniflux 填站点根，FreshRSS 需填 https://主机/api/greader.php）` |
| 4 | 提示与既有 toast 风格一致，未新造控件 | **通过** | 沿用既有 `showToast` 通道，截图可见为底部既有 toast 条 |
| 5 | 深/浅两套主题下文案完整可读、不截断、不溢出 | **通过（修正后）** | 两主题截图各一张；desc 实测换行 2 行、`scrollW == clientW == 397`、`scrollH == clientH == 35`（无溢出） |
| 6 | 既有「测试连接」「保存并同步」行为、禁用态、加载态不变 | **通过** | 两按钮位置与样式未变（截图可见）；本次未改动其状态逻辑 |

## 宽度实测与修正（两次误判，第三次量对）

placeholder 在本任务中**错了两次**，最终值经真实控件测量确定：

| 次 | 取值 | 实测宽度 | 内容盒 219px 下 |
| --- | --- | --- | --- |
| 1 | `https://reader.example.com 或 https://主机/api/greader.php` | **310.16px** | 截断，**丢掉 `/api/greader.php`** |
| 2 | `https://demo.freshrss.org/api/greader.php` | **219.46px** | **仍差 0.46px，末位字符被裁** |
| 3（现取值） | `如 https://主机/api/greader.php` | **162.09px** | **完整可读**，余量 56.91px |

**两次都错在盒模型**：该输入框 `box-sizing: border-box`、`width: 240px`、`padding: 6px 10px`、
`border: 1px` → **内容盒仅 219px**。第 1 次未测量；第 2 次用了
`getBoundingClientRect().width`（**外框** 240px）当门槛，偏乐观 21px。
第 2 次由第 1 轮独立审查指出，我随后以 canvas `measureText` 复测确认。

**截图可目视核对**：本轮 UI 证据已**清空 Endpoint 输入框**，
使 placeholder 真正显示在截图中（先前截图里输入框有值，placeholder 不可见——
该缺陷由第 1 轮审查一并指出）。三张证据中该文案均完整可见、无截断。

## 明确未做的事（owner 边界）

- **未新增任何 endpoint 自动探测/回退请求**：实现仅为文案与提示映射，
  `src-tauri/src/**` 零改动（`git diff --stat -- src-tauri` 为空），`greader.rs` 未触碰。
- 未改认证流程、未改 `ClientLogin` 解析、未改表结构、未引入新依赖。
- 未改本卡片之外的任何设置区块。

## 边界与残留

- **未验证真实账号的成功路径**：`demo.freshrss.org` 的公开演示凭据不可得
  （实测任意组合 `ClientLogin` 均回 401），故「填对完整路径后能登录成功」这一分支
  未在本轮以真实账号走通；该分支的**拉取**行为由 TASK-056 的 mock e2e 覆盖。
  此处如实记录为限制，不声称已验证。
- 提示中同时保留原始状态码（`ClientLogin → 404 Not Found`）与指引文字，
  便于熟悉 HTTP 的用户快速定位、也便于不熟悉的用户照做。
