# TASK-102 V/X 真机取证报告（独立审查 worker）

- 日期：2026-09-30
- 审查对象：candidate_digest=e1208e0c5bced0a1b557878c345299010805ccb56598c722f2ae72ce9d80dcc8（未提交工作区改动，已核验候选清单 63 文件 sha256 与工作区逐字节一致）
- 取证环境：tmp/audit-r3/harness（headless Chrome 1440x900 via CDP + 忠实假后端 mock_backend.py，`window.__TAURI_INTERNALS__` 注入走真实 IPC 代码路径，前端零改动）
- 驱动脚本：tmp/task-102/review-sync-evidence.mjs（独立审查 worker 自写；断言与截图均出自本次真实运行）
- 页面路径：主界面 → Ctrl+, 打开设置 → 「同步」页签（SyncTab 实卡渲染，非演示模式早退）

## 结果总览：X1 PASS / X2 PASS / X3 PASS（DOM 断言 9/9 PASS，0 FAIL）

| # | 检查 | 结果 | 证据 |
|---|---|---|---|
| MODE | SyncTab 实卡渲染（12 卡，无「演示模式」早退） | PASS | 卡片清单见下 |
| X1-a | 协议卡 FluxDropdown 为 SettingCard 直接子元素（无包裹 div） | PASS | dropParent="setting-card" |
| X1-b | 协议下拉与其他卡控件列同一右缘（≤2px 容差） | PASS | dropRight=1142；8 个参照控件（连接状态 tag / Endpoint·用户名·密码 input / 后台自动同步 switch / 网页登录 GitHub 按钮 / 同步内容 tag / GitHub Token input）右缘全部=1142，**偏差 0px** |
| X2-a | 动作区常驻提示唯一且单行 ≤40 字 | PASS | 「测试连接」仅验证登录；「保存并同步」确认后拉取订阅与文章。= **30 字** |
| X2-b | 提示与两按钮语义一一对应 | PASS | 测试连接=仅验证登录；保存并同步=确认后拉取订阅与文章 |
| X2-c | 「断开会移除/连接会移除」「约 1 秒回传」全页常驻文案零出现 | PASS | hasOldDisconnect=false；hasOldPushback=false；hasRemovePhrase=false（body innerText 全文检查） |
| X2-d | API 密码提示在协议卡 desc 位且 ≤32 字，无第二行常驻 hint | PASS | desc="Fever / GReader 均用「API 密码」，非登录密码"=**32 字**；全部 hint 均不含「API 密码」 |
| X3-a | 同步页真机全部 SettingCard desc ≤48 字（11 条） | PASS | 最大 47 字（GitHub Token 卡）；清单见下 |
| SHOT | 浅/深两态截图，data-theme 分别为 light/dark | PASS | 见下方两个 PNG |

## DOM 实测数据

- 卡片清单（12）：连接状态 / 同步协议 / 后端 Endpoint / 用户名 / 密码 / 同步模式 / 后台自动同步 / 清理时间范围 / 网页登录 GitHub / 同步内容与状态 / 同步后端 / GitHub Token（classic PAT）
- 控件右缘（.setting-card 双列布局控件列）：协议卡 .flux-dropdown=1142px，与其余 8 卡控件完全相等（dev=0）——X1 错位（下拉左移）已修复
- 同步页可见 desc（11 条，全部 ≤48）：未连接（27）/ Fever / GReader 均用「API 密码」，非登录密码（32）/ 只填域名即可：FreshRSS / Miniflux 自动适配（31）/ Miniflux「集成」页配置的用户名…（45）/ 本机抓取=直连各订阅源…（41）/ 按刷新间隔到期时自动做轻量增量同步…（28）/ 删除该时间之前的本地缓存（收藏文章与待同步状态始终保留）（28）/ 跳转浏览器完成 GitHub 授权…（35）/ 同步订阅源结构、分类布局…（43）/ Gist 可网页登录自动配置…（45）/ 手动填入；classic PAT 需勾 gist scope…（47）
- 断开确认框文案未改动（git diff 核对 HEAD:304 = 工作区:311 逐字同文），破坏性语义仍由 ConfirmDialog 唯一承载

## 证据文件

- 浅色：TASK-102-sync-light.png（data-theme=light，同步页全景：协议下拉与各卡控件右缘齐平、API 密码提示在 desc 位、动作区单行提示）
- 深色：TASK-102-sync-dark.png（data-theme=dark，同布局深色主题下同样成立）
- 截图由审查 worker 于本次运行经 Page.captureScreenshot 真实捕获

## 附注（范围定界，均非缺陷）

- 独立源级扫描（审查 worker 自写脚本 tmp/task-102/review-x3-scan.py，212 条去重文案字面量）：X3 范围（desc/hint/subtitle/说明文案）内无 >48 字；唯一 49 字项为断开确认框 message（UI 契约「明确不做」项）；另 2 条 >48（57/91 字）为 settings/shared.ts 的 DEFAULT_PROMPTS（AI 提示词模板，AiTab 可编辑字段的功能性初始值，非说明文案）。
- 动作区提示「唯一性」按含「测试连接」唯一定位；同屏另有 CacheCleanupSection / ConfigSyncSection / 侧栏 footer 各自的合法 hint（源级恰一块由 t101-v1-e 在 SyncTab.tsx 源内锁定）。
