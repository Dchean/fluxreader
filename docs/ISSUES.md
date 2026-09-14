# 首轮问题与风险清单

源码基准见 BASELINE。2026-09-13 已完成 TASK-002 的初始测试与构建测量，静态证据与实测结果分别标明；本清单仍不是完整代码审计。P1 表示影响验证或交付控制，P2 表示维护性/工具问题，不自动等同已确认的运行故障。完整证据见 [本次报告](../tasks/runs/TASK-002-20260913T041438Z.json)。

| ID | 类型 / 优先级 | 事实与证据 | 影响与后续动作 | 状态 |
| --- | --- | --- | --- | --- |
| ISSUE-001 | 验证 / P1 | TASK-002 已实测：前端 8/8、Rust 95 项通过，前端和 Windows 打包通过，格式检查失败 | 初次测量已建立；后续保护与完整验收仍需继续，不能标成全绿发布基线 | 初始测量已完成 |
| ISSUE-002 | CI / P1 | test:frontend 本机执行 8/8 通过；ci.yml 仍未调用 | 已证明本机可运行，后续应在 CI 既定环境验证并接入；本轮未改 CI | 差异已确认，待后续任务 |
| ISSUE-003 | 测试分层 / P1 | Rust/Node 测试及 MSI/NSIS 打包通过；没有安装、启动应用或运行真实桌面 UI E2E | 现有测试名称和打包结果不能证明完整桌面交互；需后续驱动能力与关键链路验证 | 未覆盖的边界仍存在 |
| ISSUE-004 | 发布 / P1 | release.yml 标签/手动触发且 releaseDraft=false；未显式等待同一提交的完整 CI | BATCH-005（TASK-019）已加固：tagged commit 的 CI 全绿守卫（check-runs 轮询，失败/超时即拒绝发布）+ concurrency 组 + 陈旧 releaseBody 中性化；守卫逻辑待真实发布时首次执行（NOT_RUN） | 文件加固完成；守卫待真实发布验证 |
| ISSUE-005 | 文档 / P1 | 旧 README 的版本、互斥类型、模块名称与源码不同；config_sync 中状态文件常量未证明完整文章状态同步 | README 已修订（BATCH-001/002 合并）；源码内声明性注释已于 BATCH-005 对齐事实（TASK-018，grep 复核 article_state_sync 残留为 0） | 已解决（注释与声明层面） |
| ISSUE-006 | 测试隔离 / P1 | 普通测试跳过 23 项，指定 target 后补跑 14 项且通过；仍有 7 项真实服务、2 项外部本地 fixture 未运行 | 保持明确目标列表和服务边界；不使用统一 --ignored 冒充无外部依赖门禁 | 已按计划实测，剩余 9 项未运行 |
| ISSUE-007 | 兼容性 / P1 | 两协议相关单元/本地 mock 检查已运行；真实 Miniflux/FreshRSS 服务未测 | 兼容组合、版本和操作能力需确认后实测；本机 mock 通过不等于真实服务器兼容 | 待产品确认与实测 |
| ISSUE-008 | 架构 / P2 | db.rs 注释说 SQL 集中；commands.rs、sync.rs 中存在 query_row/execute | 已全部收敛：commands.rs 的 13 处（BATCH-005，TASK-017）与 sync.rs 的 17 处（BATCH-007，TASK-022，merge 0ebd904）直接 SQL 均移至 db.rs 类型化函数；2026-09-14 实测两文件 `execute(|query_row(|prepare(|query_map(` 计数均为 0，db.rs 为 132 处 | 已解决（SQL 边界收敛完成）；db.rs 内部模块化列为 TASK-023 |
| ISSUE-009 | 职责 / P2 | store.ts、SettingsModal.tsx、commands.rs、sync.rs、db.rs 聚合多种职责 | 是候选调查区域；文件长本身不是重写证据，需结合依赖和变更风险 | 待深入分析 |
| ISSUE-010 | 环境 / P1 | 本次实际使用 Windows build 26200、Node 24.19.0、Rust 1.98.1；CI 前端为 Ubuntu/Node 22，Rust 配置 stable | 本次不是实际 CI 运行，也未证明最低 Rust 1.80；后续需收敛并验证环境 | 已记录真实环境，差异待处理 |
| ISSUE-011 | 并行隔离 / P2 | ai_e2e、sync_e2e 等仍使用固定临时库名；本轮为各命令设置本次运行专用 TEMP/TMP，Rust 测试命令串行 | BATCH-004（TASK-015）已将全部测试临时库改为进程 ID+纳秒唯一路径（std 方案，无新依赖），连续两轮全量测试通过；测试对 Tauri/全局状态依赖低（M2 分析确认） | 已解决 |
| ISSUE-012 | 产品范围 / P1 | DEC-008 / DEC-009 已确认所有 OPT 保留；OPT-004 仅同步订阅源、客户端设置与非敏感连接配置，排除 API Key/密码等凭据 | 需求范围问题已解决，任务、测试与验收文档已同步；具体字段清单是后续实现产物 | 已解决（文档范围决定，不表示实现已完成） |
| ISSUE-013 | 格式 / P2 | cargo fmt --check 返回 1；38 个 Rust 文件共 532 处格式差异 | TASK-006 已全仓格式化；BATCH-004（TASK-014）在 CI rust 作业新增 cargo fmt --check 门禁并连续多次 CI 通过 | 已解决 |
| ISSUE-014 | lint / P2 | oxlint 返回 0，6 条警告：回归脚本 3 条未使用项、Reader 1 条 effect 依赖、Overlays 2 条 effect 内 setState | 产品代码 3 条已由 BATCH-003（TASK-010）按分析 Note 修复（lint 6→3）；剩余 3 条来自技能副本（目录约定原样复制，不在范围） | 产品部分已解决；技能副本部分按约定保留 |
| ISSUE-015 | 工具环境 / P2 | 初次快照与 Vite 构建出现沙箱 EPERM；获准提升运行权限后成功 | 保留首次失败与重跑，区分环境限制和产品缺陷 | 本次执行已解决，记录保留 |
| ISSUE-016 | 文档依赖 / P2 | 交接时已将现有技能 43 个文件复制至项目 .agents/skills，并核对与源副本的哈希 | 新 agent 可从项目读取；具体版本与复制记录见交接报告 | 已补齐，随交接包验证 |
| ISSUE-017 | 协议兼容 / P1 | 真实服务测试（2026-09-13，test 账号）：greader.rs login 以 resp.json() 解析 ClientLogin，仅兼容响应 output=json 的服务（Miniflux/chean.top 实证 3/3 通过）；FreshRSS（ceaion.com）忽略 output=json 返回经典文本，3 项 live 测试全部失败（error decoding response body） | 修复方向：登录解析双格式兼容（JSON 失败回退 Auth= 文本行）| 已定位，修复任务 TASK-028 |
| ISSUE-018 | 协议兼容 / P1 | fever.rs 硬编码 `{base}/fever/?api`；FreshRSS 的 Fever 在 {base}/api/fever.php（应用公式 md5(user:pass) 下 auth=1 实证可达），当前端点配置无法到达（404） | 修复方向：endpoint 以 .php 结尾时按 `{base}?api` 直用 | 已定位，修复任务 TASK-029 |

## OPT-004 后续实现核对

配置同步目标范围已明确。现有 SyncPayload 含 app_settings / ai_config 等配置内容，后续需要按允许字段构建 payload，并验证敏感凭据排除与导入行为。现有配置同步测试已通过，但其通过不能证明新要求已经满足；仍需明确覆盖敏感凭据排除和导入边界的测试。此项作为实现差异继续跟踪，不能因 ISSUE-012 的范围决定已解决而认定现有实现已满足要求。本次只运行本地 mock，未运行真实外部同步。

## 新问题的最低记录要求

包含：问题 ID、源码提交、类别、观察证据、期望依据、实际现象、影响、复现方式或未复现原因、建议动作、关联任务和当前状态。

静态怀疑与可复现缺陷分开。没有证据时不得写成确定 bug；已知缺陷不得仅通过改快照/断言变成“通过”。

本次已记录格式失败和 lint 告警；95 项 Rust 测试与 8 项前端逻辑检查没有失败。这不代表不存在其他功能缺陷。后续已按用户交接目标授予管理 agent 低风险工作权限，具体边界见 HANDOFF / EXECUTION-POLICY；高风险、超限、合并和发布仍需用户确认。


## 真实服务兼容性实测（2026-09-13，ISSUE-006/007 首轮）

用户提供测试服务与账号（test/testtest）：Miniflux https://rss.chean.top、FreshRSS https://rss.ceaion.com。

| 组合 | 结果 | 证据 |
| --- | --- | --- |
| Miniflux × Google Reader | **3/3 通过** | greader_live（登录+订阅、条目+正文、edit-tag 往返）|
| Miniflux × Fever | **3/3 通过** | fever_live |
| Miniflux × Fever 完整同步管线 | **1/1 通过** | fever_sync_live（推送+拉取+状态对账）|
| FreshRSS × Google Reader | 0/3 失败（ISSUE-017） | 登录解析失败 |
| FreshRSS × Fever | 0/3 失败（ISSUE-018） | /fever/ 404 |

两账号已有订阅源，无需添加。待 TASK-028/029 修复后复测 FreshRSS 两组合。