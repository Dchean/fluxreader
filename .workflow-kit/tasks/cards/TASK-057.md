<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-057 · 修 Endpoint 填法指引与失败提示：直填 FreshRSS 域名登录失败（Bug 1）

**状态**：ready

**目标**：用户实测：**直接填写域名无法登录**（`https://demo.freshrss.org` 连不上），必须填**完整 API 路径** `https://demo.freshrss.org/api/greader.php` 才行。根因经实证定位：`greader.rs` 把 endpoint **原样当作根 URL**（`let base = endpoint.trim_end_matches('/')`，随后拼接 `/accounts/ClientLogin`），从不规范化、也不识别 API 路径。而两种协议的真实布局不同——**Miniflux** 的 GReader API 位于站点**根**（`https://reader.example.com/accounts/ClientLogin`），**FreshRSS** 位于**子路径** `/api/greader.php`（`https://主机/api/greader.php/accounts/ClientLogin`）。实测对照：`POST https://demo.freshrss.org/accounts/ClientLogin` → **404**（HTML 错误页，端点不存在）；`POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` → **401**（端点存在，仅凭据不对）。设置页当前文案『例如 https://reader.example.com』**只教了 Miniflux 的填法**，FreshRSS 用户按提示填写必然失败。**owner 明确裁决『不改探测逻辑，只改文案与错误提示』**——即本任务**不新增自动探测/回退请求**，只让用户能填对、并在填错时看得懂为什么。

**依赖**：TASK-055
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-057.md
**界面检查**：设置 → 同步 → 「后端 Endpoint」卡片：说明文案同时给出 Miniflux 与 FreshRSS 两种填法, Endpoint 输入框 placeholder 反映两种协议的真实填法（不再只给 Miniflux 形式）, 填入纯域名（如 https://demo.freshrss.org）保存失败时，错误提示**可操作**：明确指出 Endpoint 需指向 API 路径、并给出 FreshRSS 的完整写法, 错误提示与既有 toast 风格一致（沿用 extractError / showToast，不新造控件）, 深色与浅色两套主题下文案与提示均完整可读、不截断、不溢出, 既有「测试连接」与「保存并同步」按钮行为、禁用态与加载态不变
**修改范围**：.workflow-kit/docs/UI-CONTRACT-REQ-057.md, .workflow-kit/tasks/evidence/baseline-2026-09-18-task057.md, .workflow-kit/tasks/evidence/TASK-057-*, src/**, tools/**, tsconfig.test.json

## 验收标准

- **文案给出两种协议的真实填法**：Endpoint 卡片的 desc 与 placeholder 必须同时说明 Miniflux（站点根）与 FreshRSS（`/api/greader.php` 子路径）的填法，并给出 FreshRSS 的完整示例。不得只给一种
- **失败提示可操作**：当保存/测试连接因 Endpoint 形态错误而失败（典型：根路径返回 404）时，提示必须让用户知道**下一步该填什么**，而不是只报 HTTP 状态码。须在报告中给出修复前后提示文案的对照
- **不得引入自动回退探测**：给出 `git diff` 证据证明未新增任何自动尝试 `/api/greader.php` 的请求逻辑（owner 明确边界）
- **UI 证据（本任务 ui_change=true）**：按 UI 契约提供**深色与浅色两套主题**下的截图，覆盖 `ui_checks` 列出的全部状态；截图须为**实机运行截图**（非设计稿），并附交互报告说明验证方式
- **前端行为断言**：为本次文案/提示改动补可复跑断言（沿用 `npm run test:frontend` 既有框架，`tsconfig.test.json` 已列入允许路径）；断言须**修前失败、修后通过**，给出修前失败证据
- **既有前端回归不回归**：`npm run test:frontend` 通过（基线 241/241，可增不可减）
- 既有 Rust 测试不得回退：`cargo test` 通过（基线 **137 passed / 0 failed / 9 ignored**）
- 四门禁全绿：`cargo test`、`npm run lint`、`npm run build`、`npm run test:frontend`
- 不引入新依赖；`package.json` 零改动；不改 Rust 源码（本任务纯前端 + 文档）
- **不改 endpoint 解析语义**：`greader.rs` 零改动（给出证据）
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成；行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）
- **用户真实数据库不得写入**：`%APPDATA%\com.fluxreader.app` 下任何文件只读使用；不得在其中创建/修改数据

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-18 基线（TASK-055 之后）：`npm run lint` exit 0（0 warnings/0 errors）、`npm run build` exit 0、`npm run test:frontend` exit 0 且 241/241（既有回归 26 + 新增 store 行为断言 215）、`cargo test` 137 passed / 0 failed / 9 ignored。**本任务有意改变行为**（用户可见文案与失败提示），属产品文案契约变化，已由 DEC-user-testing-bugs-20260918 覆盖（owner 明确选择『只改文案与错误提示』）。**本任务 owner 明确不改探测逻辑**：不新增自动回退请求，endpoint 解析语义保持原样（`greader.rs` 零改动）。前端既有 241 项断言中与同步设置文案相关的部分可能因文案变更需要**适配**（而非删除）——若确需改动，须逐个说明理由并在报告中列出，且总数不得减少。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task057.md
- 需求决定：DEC-user-testing-bugs-20260918
- 补充：Endpoint 文案/占位符与失败提示的可复跑前端断言（须修前失败、修后通过）；本任务的核心交付是用户可见文案与提示，此前无任何断言锁定它们；必须有断言防止文案回退成『只教 Miniflux 填法』。；验证：frontend
- 适配：`src/components/settings/SyncTab.tsx` 的 Endpoint 卡片 desc/placeholder 与保存失败提示分支；文案与提示分支是本任务要改的对象；若既有断言引用了旧文案，须相应适配（不得删除断言）。；验证：frontend
- 保留：其余前端 241 项断言（含 store 行为、D1–D5 竞态与哨兵判定、REQ-004/005/006/008 既有回归）；本任务只改同步设置区的文案与提示，不得波及其它已验收行为；它们是『改动未外溢』的直接证据。；验证：frontend
- 保留：全部 Rust 测试（137 passed / 9 ignored，含 TASK-055 墓碑复现测试）；本任务按 owner 边界不改 Rust 源码（`greader.rs` 零改动），Rust 侧全绿即证明端点解析语义未被触碰。；验证：cargo_test

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-057.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
