<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-059 · Endpoint 自动适配：只填域名即可连接（FreshRSS / Miniflux 双协议）

**状态**：done

**目标**：**需求（owner 明确纠正）**：不管后端是 FreshRSS 还是 Miniflux，用户**只需要填写域名**，**不需要**知道或填写 `/api/greader.php` 这类 API 后缀。此前作者误读了需求（把用户的故障描述当成『用户想知道该填什么后缀』），并据错误理解裁决了『不改探测逻辑、只改文案』，导致 TASK-057 交付的是『教用户填完整路径』——方向反了。本任务改为**由应用自动适配**。**实证依据**：(1) GReader——`POST {域名}/accounts/ClientLogin` → **404**（端点不存在）、`POST {域名}/api/greader.php/accounts/ClientLogin` → **401**（端点存在、凭据被拒），即 **404 与 401/403/400 可可靠区分『路径不存在』与『路径正确但凭据错』**；(2) Fever——FreshRSS 真实端点在 `{域名}/api/fever.php?api`（实测 200 `{"api_version":4,"auth":0}`），而现有客户端写死拼 `{base}/fever/?api`（实测 FreshRSS 上 **404**），即 **Fever + FreshRSS 当前根本连不上**，与本缺陷同源（路径形态假设写死）。本任务让两个协议都支持『只填域名』，并保留旧用法兼容。

**依赖**：TASK-058
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-059.md
**界面检查**：设置 → 同步 → 「后端 Endpoint」：说明文案改为**只填域名**（不再要求 /api/greader.php 这类后缀）, placeholder 给出纯域名示例（如 https://demo.freshrss.org）, 填入纯域名后点「测试连接」：FreshRSS 与 Miniflux 均应连接成功（不再报 404）, 已填完整 API 路径的旧用法**仍然可用**（向后兼容，不回退）, 解析成功后设置页显示的仍是**用户填写的原始值**（不把内部解析结果回显成后缀，避免让用户以为必须填后缀）, 连接确实失败时，提示须如实说明（凭据错报凭据错、地址不可达报不可达），不得把凭据问题说成填法问题, 深色与浅色两套主题下文案完整可读、不截断、不溢出
**修改范围**：.workflow-kit/docs/UI-CONTRACT-REQ-059.md, .workflow-kit/tasks/evidence/baseline-2026-09-18-task059.md, .workflow-kit/tasks/evidence/TASK-059-*, src/**, src-tauri/src/**, src-tauri/tests/**, tools/**, tsconfig.test.json

## 验收标准

- **纯域名必须能连（两个协议各一测）**：对**只填域名**的输入，FreshRSS 形态的 mock 与 Miniflux 形态的 mock **都要**连接成功；给出两者的实测通过证据
- **旧用法不得回退**：已填完整 API 路径（`.../api/greader.php`）的输入仍须一次成功，且**不应**产生多余探测请求（首个候选即命中）
- **必须区分『路径不存在』与『凭据错误』**：给出测试证明——(a) 路径全错时给出『未找到 API 端点』类错误；(b) 路径正确但凭据错时给出**凭据类**错误（不得报成地址错）。这是本任务最容易做错的地方
- **探测有界且有序**：候选地址数量固定且有限（不得无限尝试）；任一候选返回『凭据被拒』(401/403/400) 时**立即停止**并报凭据错，不得继续尝试其余候选而掩盖真实原因
- **解析结果要缓存**：解析成功后须持久化，避免每次同步阶段都重复探测（`build_client` 在 `feeds_phase`/`states_phase`/调度同步中被反复调用）。给出缓存生效的证据（如第二次同步不再发探测请求）
- **用户输入原样保留**：设置页回显的仍应是用户填写的原始值（纯域名），不得把内部解析出的后缀回显给用户；用户修改地址后缓存须失效并重新解析
- **Fever 路径形态修正**：Fever 客户端须同时支持 `{域名}/fever/`（Miniflux 形态）与 `{域名}/api/fever.php`（FreshRSS 形态）；给出 FreshRSS 形态的实测通过证据
- **Fever 版本校验放宽（owner 追加授权）**：`fever.rs` 现写死 `if env.api_version != 3` 即拒绝；而 FreshRSS 的 Fever 实测返回 `{"api_version":4,"auth":0}`，故**仅修路径仍会报『不支持的 Fever API 版本 4』**。须放宽为**兼容 3 及以上**（信封结构与认证字段两者一致），并在报告中论证该放宽的依据与风险；`auth` 校验不得放松
- **前端文案与提示同步更正**：Endpoint 卡片的 desc/placeholder 改为『只填域名』；TASK-057 遗留的『需填 /api/greader.php』类文案与 404 提示**必须一并更正**（否则界面在教用户做已经不需要的事）。给出修改前后对照
- **UI 证据（ui_change=true）**：深色与浅色两套主题下的实机截图，覆盖 `ui_checks`；须为运行中应用的截图（非设计稿），并附交互报告
- **前端断言**：为文案更正补可复跑断言，须**修前失败、修后通过**，给出修前失败证据
- 既有测试不得回退：`cargo test`（基线 **141 passed / 0 failed / 9 ignored**）与 `npm run test:frontend`（基线 **280/280**）均须通过；通过数可增不可减，ignored 不得增加
- 四门禁全绿：`cargo test`、`npm run lint`、`npm run build`、`npm run test:frontend`
- 不引入新依赖；`Cargo.toml`/`Cargo.lock` 与 `package.json` 零改动
- **不改任何对外请求的语义**（除地址解析外）：认证、订阅、状态推送的请求内容与顺序不变
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成；行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）
- **用户真实数据库不得写入**：`%APPDATA%\com.fluxreader.app` 只读使用；端到端测试如需改数据，须先备份并在结束后逐字节还原（给出哈希一致的证据）

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-18 基线（TASK-058 之后）：`npm run lint` exit 0（0 warnings/0 errors）、`npm run build` exit 0、`npm run test:frontend` exit 0 且 **280/280**（既有 26 + 新增 254）、`cargo test` **141 passed / 0 failed / 9 ignored**。**本任务有意改变行为**：Endpoint 的解析由『原样当作 API 根』改为『按候选地址探测并自动适配』，并修正 Fever 的路径形态——这是**用户可见的连接行为变更**，已由 DEC-endpoint-autodetect-20260918 覆盖（owner 明确纠正需求并要求两个协议都做到只填域名）。**基线盲区须如实记录**：既有 Rust e2e 与前端回归**全部**使用『已知正确的 endpoint』（mock 的 `server.url()` 直接就是 API 根），**没有任何测试覆盖『用户填的是纯域名』这一输入形态**——这正是本缺陷此前逃过测试网、并且作者误读需求后又被 TASK-057 反向固化的原因。本任务必须补上该形态的测试。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task059.md
- 需求决定：DEC-endpoint-autodetect-20260918
- 补充：『只填域名 ⇒ 自动适配成功』的 Rust 测试（FreshRSS 形态与 Miniflux 形态各一，含 Fever）；该输入形态此前完全无覆盖（所有测试都用已知正确的 endpoint）。须有断言锁定新契约，防止回退成『必须填后缀』。；验证：cargo_test
- 补充：『路径不存在 vs 凭据错误』可区分的测试（含『凭据错时不得继续尝试其余候选』）；这是自动探测最容易做错的地方：若把 401 也当成『路径不对』继续试，用户会在凭据填错时收到误导性的『找不到 API』，比原来的行为更糟。必须有测试守住。；验证：cargo_test
- 补充：『旧用法（完整路径）仍一次成功且不产生多余探测』的测试；行为变更须证明向后兼容，否则会破坏已能正常工作的用户配置。；验证：cargo_test
- 适配：`src-tauri/src/greader.rs` 与 `src-tauri/src/fever.rs` 的地址构造，以及 `sync/phases.rs` 的 test_connection 与 `sync/credentials.rs` 的 build_client；这几处共同决定『用户输入 → 实际请求地址』的映射，是本任务的核心改动点。；验证：cargo_test
- 适配：`src/components/settings/SyncTab.tsx` 与 `endpointHint.ts` 中『需填 /api/greader.php』的文案与 404 提示；TASK-057 按其时（错误理解的）需求教用户填完整路径；本任务改为自动适配后该文案会**反向误导**用户，必须同步更正。既有引用旧文案的前端断言须相应适配而非删除。；验证：frontend
- 保留：其余全部 Rust 测试（含 TASK-055 墓碑、TASK-056 零 folder 新库与失败可见性、A-1 防复活等回归网）；本任务只改地址解析，不得改动推送顺序、对账口径、入队条件与墓碑语义；这些测试正是那些语义的保护。；验证：cargo_test
- 保留：其余前端断言（含 TASK-058 的同步失败可见性与 TASK-052 分页口径）；本任务只改 Endpoint 文案与连接提示，不得波及已验收的失败可见性、分页与 store 行为。；验证：frontend

## 执行与恢复

- 首次开始：2026-09-18T13:26:20.794697Z
- 原截止时间：2026-09-18T17:26:20.794697Z
- 当前截止时间：2026-09-19T03:32:57.301058Z
- 时钟：按墙钟计：额度 630 分钟，写入阶段已用约 337 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T23:21:37.046247Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T23:28:34.244308Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-09-19T01:33:00.523642Z：依据新决定追加预算；原始时钟与失败记录保留；下一步：先核对已有成果，再按原任务范围继续
- 2026-09-19T01:33:28.239978Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T01:39:30.143936Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收
- 2026-09-19T01:41:14.896679Z：依据新决定追加预算；原始时钟与失败记录保留；下一步：先核对已有成果，再按原任务范围继续
- 2026-09-19T01:41:34.451107Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-19T01:47:24.877007Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-059.json)

- [RUN-61037fec83504a9c87b2da94c03bc088](../runs/RUN-61037fec83504a9c87b2da94c03bc088.json)
- [RUN-01eaf25b714145809f013183d6333313](../runs/RUN-01eaf25b714145809f013183d6333313.json)
- [RUN-752722f56c6f41bf8a53f5f220f951a6](../runs/RUN-752722f56c6f41bf8a53f5f220f951a6.json)
- [RUN-6bafc20f36494ba7a702defabddd64c7](../runs/RUN-6bafc20f36494ba7a702defabddd64c7.json)
- [RUN-9a12519e47ed45e7abc860bba84d047f](../runs/RUN-9a12519e47ed45e7abc860bba84d047f.json)
- [RUN-22e370488c0b46659fdcffee183f3cfd](../runs/RUN-22e370488c0b46659fdcffee183f3cfd.json)
- [RUN-d2817bd098784e2fa67e55afedadc2b2](../runs/RUN-d2817bd098784e2fa67e55afedadc2b2.json)
- [RUN-8d380b551c27491198f44e4738bf14b5](../runs/RUN-8d380b551c27491198f44e4738bf14b5.json)
- [RUN-a197554176ae4222aabea1b5cfcc1756](../runs/RUN-a197554176ae4222aabea1b5cfcc1756.json)
- [RUN-23e61f6b9bc646b0a6944e60c440d50f](../runs/RUN-23e61f6b9bc646b0a6944e60c440d50f.json)
- [RUN-e59bda5f9aff4ca889210ac8b68d5ca9](../runs/RUN-e59bda5f9aff4ca889210ac8b68d5ca9.json)
- [RUN-50d36a6bbfaf4e2984d4b79a828fe66a](../runs/RUN-50d36a6bbfaf4e2984d4b79a828fe66a.json)
- [RUN-dd4206433d6e4f0e94987020e9e5db29](../runs/RUN-dd4206433d6e4f0e94987020e9e5db29.json)
- [RUN-8168ae973453419d88794e6f78f9c61e](../runs/RUN-8168ae973453419d88794e6f78f9c61e.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
