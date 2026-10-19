# FluxReader 审计驱动优化重构线路

日期：2026-10-08（Asia/Shanghai）。基线：425c6360995ddd9e89c4b2ac2aebf0e927bdc3b9，v0.17.1。

## 目标与授权

用户授权主控依据审计持续推进：Codex 制定路线/任务卡并验收；CodeBuddy 编写产品代码和回归；每次编写后独立子代理 review，反馈返工仍交原 CodeBuddy 会话。现有派发/返工烟测已完成，但不代表下列产品任务已完成。

保持 Tauri 2 + Rust + React/Zustand + SQLite 与五布局、双协议、AI、全文、OPML、桌面能力；不换技术栈，不以缩小功能范围代替修复。按数据正确性→协议/账号→交互→性能/收口推进。同步兼容性任务独立验收，不阻塞可以单独修复的本地缺陷。

产品修改只在 dev；不新建 worktree；主控负责提交/推送与 CI 状态，CodeBuddy 禁止 Git 写操作。dev 全绿、最后总 review 通过后常规合并 main，无未经请求的版本 bump/tag/release。原报告按用户要求留在 docs。

## 模型与失败切换

CodeBuddy 首选 --model deepseek-v4.1-flash --effort max。仅确认为配额/token 耗尽时，在保留原会话上下文与未提交工作前提下切到 --model custom-local:deepseek-v4.1-flash --effort max；不是超时就重启，也不切到别的模型。每次派发保存参数、PID、会话和终态，进程仍活着时继续观察同一实例。

## 进度快照（2026-10-15，Asia/Shanghai）

- **18 张主卡：15 张完成验收并通过提交级 CI，3 张尚未完成。** 未完成项为 OPT-008 总收口、OPT-017 实际性能优化、OPT-018 最终验收与发布；父子卡不重复计数，不按卡片数量推算工作量百分比。
- 已完成：OPT-001～007、OPT-009（B1/B2/B3 已随 55c379b 通过 CI）、010、011、012、013（013A/B1/B2/B3 全部 CI 通过）、014、015、015Q、016。
- 剩余：008（主卡总收口，待 018）、017（已拆 017B～017F 五个子卡，逐卡派发）、018（最终验收、main 合并与发布）。
- 状态取值：`queued`（未开工）/ `coding`（实现中）/ `review`（实现完成、门禁通过、待独立复审，尚未提交/CI）/ `verified-local`（本地/夹具已验收）/ `verified-ci`（已提交并通过提交级 CI）。
- 最新产品提交 **2b1a395** 已推送 dev，[CI 38716779377](https://github.com/Dchean/fluxreader/actions/runs/38716779377) completed/success（前端 56s、Windows Rust 7m51s）。更早提交级 CI 依次为 7339679 / [38697114198](https://github.com/Dchean/fluxreader/actions/runs/38697114198)、55c379b / [38677466863](https://github.com/Dchean/fluxreader/actions/runs/38677466863)、e80957b / [38714382972](https://github.com/Dchean/fluxreader/actions/runs/38714382972)。未合并 main，未发布。

## 路线与状态

| 卡片 | 交付边界 | 审计覆盖 | 前置卡 | 状态 | 当前进展 / 下一步 |
| --- | --- | --- | --- | --- | --- |
| [OPT-001](OPT-001.md) | 可靠 outbox 操作身份 | F01 | 无 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-002](OPT-002.md) | 正文安全边界与解析健壮性 | F07,F20 | 无 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-003](OPT-003.md) | 原子抓取与缓存确认 | F08 | OPT-001 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-004](OPT-004.md) | Google Reader 目标服务端契约 | F02,F04 | 无 | verified-ci | 已提交、独立审查及 CI 通过；真实服务端边界见单卡 |
| [OPT-005](OPT-005.md) | Fever 身份与完整历史回溯 | F03,F19 | OPT-004 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-006](OPT-006.md) | 账号生命周期与配置导入隔离 | F05,F06 | OPT-001, OPT-004 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-007](OPT-007.md) | 可恢复 pull 与提交时冲突保护 | F09,F10,F11 | OPT-005, OPT-006 | verified-ci | c25d367 独立审查、主控复核与提交级CI通过 |
| [OPT-008](OPT-008.md) | 内容身份与保守去重 | F12 | OPT-007 | verified-ci | 008A及008B第二阶段本地门禁、独立复审与Windows CI通过；待总收口 |
| [OPT-009](OPT-009.md) | 可靠订阅写回和能力声明 | F17 | OPT-006, OPT-008 | verified-ci | 55c379b 整卡初审 PASS、主控复核与 [CI 38677466863](https://github.com/Dchean/fluxreader/actions/runs/38677466863) 成功；待 OPT-008 总收口 |
| [OPT-010](OPT-010.md) | 配置快照合并与镜像语义 | F18 | OPT-006, OPT-009 | verified-ci | 7339679 初审/返工/复审与主控复核通过，[CI 38697114198](https://github.com/Dchean/fluxreader/actions/runs/38697114198) 成功；待 OPT-008 总收口 |
| [OPT-011](OPT-011.md) | AI 设置单点与输入范围一致 | F13,refreshInterval | OPT-006 | verified-ci | c25d367 独立审查、主控复核与提交级CI通过 |
| [OPT-012](OPT-012.md) | 阅读器源码与浮层交互 | F14,closeAsk | 无 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-013](OPT-013.md) | AI 完成状态和缓存恢复 | F15 | OPT-002, OPT-008B, OPT-011 | verified-ci | 013A/B1/B2/B3 全部通过提交级 CI（B3 = af3e6a1 / CI 38768730596）；父卡收口完成 |
| [OPT-014](OPT-014.md) | 更新检查与凭据失败关闭 | F16,F22 | 无 | verified-ci | 已提交、独立审查及 CI 通过 |
| [OPT-015](OPT-015.md) | 窗口启动恢复与幂等媒体键 | F21,F23 | 无 | verified-ci | 015P 首显最大化修复已通过 CI；015N 八场景本地原生验收通过 |
| [OPT-015Q](OPT-015Q.md) | 生产自动建窗路径冷启位置恢复 | F23 | OPT-015 | verified-ci | R2 差集 12/12 绿、015N 8/8、独立复审条件通过；dev 未复现跳位，生产零改动；e80957b CI 38714382972 通过；待随 OPT-015 系列合并后请用户实机复测 |
| [OPT-016](OPT-016.md) | 行为回归分层与依赖卫生 | 测试网,依赖,CI | OPT-003, OPT-012, OPT-013, OPT-015 | verified-ci | c25d367 独立审查、主控复核与提交级CI通过 |
| [OPT-017](OPT-017.md) | 有基线的搜索和媒体性能 | FTS,画廊,缓存,观测 | OPT-007, OPT-008, OPT-016 | coding | 017A 基线、017B 搜索/FTS、017C 画廊、017D 封面、017E 错误中心、017G 读闭包已过提交级 CI；017F 进行中 |
| [OPT-018](OPT-018.md) | 全量收口和发布前验收 | 全部审计项 | OPT-001 至 OPT-017 | queued | 尚未进入最终验收；依赖前述主卡全部收口 |

状态：queued / coding / review / rework / verified-local / verified-ci / integrated。未取得完整作用域证据的卡片不得记 complete。卡片中的写集可在读码后由主控明确调整，不允许执行者自行扩展。

## 每卡固定验收

1. 主控验证触发链与 Note 归属，发出明确需求、非目标、写集、负例和门禁。
2. CodeBuddy 先写行为反例再修复，运行其能承担的检查；不能伪造未跑结果、忽略失败或用源码 contains 代替关键行为。变异验证必须附「还原校验输出」日志（`diff -q` 或等效的逐字比对结果），不接受仅有自述的「已还原」。
3. 独立 reviewer 只读检查需求匹配、真实 diff、迁移/错误/并发边界；不能自己改代码或派嵌套 reviewer。
4. 有 P1/P2 未关闭必须返工；主控核对关键差异、最终版本与验证证据，只针对缺口、验证后变化或集成新风险补测，不默认重跑执行者已充分完成的整套检查。维护同批 Note。
5. npm lint/build/test:frontend/test:components 与 cargo fmt/clippy/test 由执行负责人本地承担，提交级 CI 再验证精确提交（Rust 使用 windows-latest）。本机已证实可以完整链接，不能再沿用缺 MSVC 的历史豁免；主控无需在充分且版本对应的证据上重复整套命令。孤立探针不能冒充全项目门禁，真实服务端 live 测试仍单独约定边界。
6. 模拟数据/隔离数据库可自行测试；真实账号、用户 DB、订阅删除、付费 AI 调用不纳入无边界测试。需要真实平台验收时记录缺口而不是假通过。

## 证据与任务卡

长期决定写 .agents/notes，优先更新原决定；任务卡只记录交付范围/状态/验证引用，不重建旧的工作流引擎。临时完整 CLI 输出与探针保存在被忽略的 tmp/optimization-20261008；可持久追溯的结论、提交/CI 标识写入卡片。

参考：[审计基线](../audit-2026-10-08.md)、[协作决定](../../.agents/notes/implemented/process/2026-10-08-主控与代码执行分离.md)。

## 独立子卡（不替代父卡完整目标）

| 子卡 | 范围 | 状态 | 父卡剩余工作 / 验收边界 |
| --- | --- | --- | --- |
| [OPT-008A](OPT-008A.md) | 保守 URL 匹配与版本化重建 | verified-ci | 008：仍缺内容/条目实体分离 |
| [OPT-008B](OPT-008B.md) | 内容实体与各订阅源条目分离 | verified-ci | R3A独立复审通过；提交8156726的Windows CI 38381181417成功；待总收口 |
| [OPT-013A](OPT-013A.md) | SSE 完成信号与错误分型 | verified-ci | 013：仍缺长文分段、缓存版本与恢复 |
| [OPT-015N](OPT-015N.md) | 隔离 Windows 原生窗口恢复验收 | verified-local | 八场景及独立审查通过；物理拔屏、动态 DPI 与完整 SMTC 音频链未覆盖 |
| [OPT-015P](OPT-015P.md) | 原生最大化首显闪烁修复 | verified-ci | 已提交并通过 CI |
| [OPT-016A](OPT-016A.md) | CI 执行责任、受控 HTTP 与依赖补丁 | verified-ci | 已完成；父卡仍需范围收口 |
| [OPT-016B](OPT-016B.md) | 真实 React 挂载组件回归 | verified-ci | 已完成；后续功能改动仍须维护相关回归 |
| [OPT-016C](OPT-016C.md) | 前端状态回归按领域拆分 | verified-ci | 已完成并通过独立审查及 CI |
| [OPT-017A](OPT-017A.md) | 隔离数据层性能基线 | verified-local | 修订指标与原始数据已复核；仅本地基线，不是性能优化完成 |
| [OPT-017B](OPT-017B.md) | 搜索路径与 FTS 写放大处置 | verified-ci | cf82d21 / CI 38823585612；v29 删旧 FTS、trigram 候选索引 + 等值回落 |
| [OPT-017C](OPT-017C.md) | 画廊布局有界渲染 | verified-ci | cf46b7e / CI 38856753155；lanes + 垂直虚拟化，通用上界式与参考口径 48 |
| [OPT-017D](OPT-017D.md) | 封面补全覆盖面与预算 | verified-ci | 9e49e3d / CI 38872257111；候选扩到服务端无图文章 + 持久扫描游标 + 预算/可观测 |
| [OPT-017E](OPT-017E.md) | 重试入口持久可达与有界错误提示 | verified-ci | 84be116 / CI 38886714784；错误中心（20 条/实体 key 去重）+ 重试语义单点 |
| [OPT-017F](OPT-017F.md) | 重复实现与零消费入口清理 | queued | 先搜消费者再删；numericId 双实现等值后收敛 |
| [OPT-017G](OPT-017G.md) | 读闭包查询规划与批量标读成本 | verified-ci | b126373 / CI 38841560090；v30 部分索引 `(url_norm) WHERE is_read=0`，50k×8KiB 1000 条标读 65772→736ms |
| [OPT-009B2](OPT-009B2.md) | 订阅 outbox 生产者、分类收敛与 tombstone 失败关闭 | verified-local | R2 复审返工实现完成：退订目标不存在（FreshRSS 400）不再永久 Retry（容忍为已满足，墓碑判据不变）、删除移除基准链起点优先 + 三类后继行重定统一（移动未发送/已发送时序都收敛）、P3-B/D/E/F 已处置登记；返工后门禁 45 suites/635 passed/0 failed，fmt/clippy 通过；独立 CodeBuddy 审计（条件通过，P3-1 已订正）+ 主控复核通过；未提交未跑 CI |
| [OPT-009B3](OPT-009B3.md) | 订阅写回能力声明、持久回执与确认文案 | verified-local | 六项交付经全新只读会话 `audit-opt009b3-r1-20261015` 独立审计通过（无 P0/P1/P2）并由主控逐条回查源码锚点复核；5 条 P3 不阻断，P3-1 措辞已订正；写集外 7 处追加式增量已追认；未提交、未跑 CI |

上述 10 张子卡与 18 张主卡共有 28 个卡片文件，但存在父子覆盖关系，不能按 28 个独立交付项计算进度。取证子卡的 verified-local 不等于产品提交已通过 CI。父卡只在子卡与剩余范围全部验收后关闭。
