# FluxReader 审计驱动优化重构线路

日期：2026-10-08（Asia/Shanghai）。基线：425c6360995ddd9e89c4b2ac2aebf0e927bdc3b9，v0.17.1。

## 目标与授权

用户授权主控依据审计持续推进：Codex 制定路线/任务卡并验收；CodeBuddy 编写产品代码和回归；每次编写后独立子代理 review，反馈返工仍交原 CodeBuddy 会话。现有派发/返工烟测已完成，但不代表下列产品任务已完成。

保持 Tauri 2 + Rust + React/Zustand + SQLite 与五布局、双协议、AI、全文、OPML、桌面能力；不换技术栈，不以缩小功能范围代替修复。按数据正确性→协议/账号→交互→性能/收口推进。同步兼容性任务独立验收，不阻塞可以单独修复的本地缺陷。

产品修改只在 dev；不新建 worktree；主控负责提交/推送与 CI 状态，CodeBuddy 禁止 Git 写操作。dev 全绿、最后总 review 通过后常规合并 main，无未经请求的版本 bump/tag/release。原报告按用户要求留在 docs。

## 模型与失败切换

CodeBuddy 首选 --model deepseek-v4.1-flash --effort max。仅确认为配额/token 耗尽时，在保留原会话上下文与未提交工作前提下切到 --model custom-local:deepseek-v4.1-flash --effort max；不是超时就重启，也不切到别的模型。每次派发保存参数、PID、会话和终态，进程仍活着时继续观察同一实例。

## 路线与状态

| 卡片 | 交付边界 | 审计覆盖 | 前置卡 | 状态 |
| --- | --- | --- | --- | --- |
| [OPT-001](OPT-001.md) | 可靠 outbox 操作身份 | F01 | 无 | verified-ci |
| [OPT-002](OPT-002.md) | 正文安全边界与解析健壮性 | F07,F20 | 无 | verified-ci |
| [OPT-003](OPT-003.md) | 原子抓取与缓存确认 | F08 | OPT-001 | coding |
| [OPT-004](OPT-004.md) | Google Reader 目标服务端契约 | F02,F04 | 无 | verified-ci |
| [OPT-005](OPT-005.md) | Fever 身份与完整历史回溯 | F03,F19 | OPT-004 | coding |
| [OPT-006](OPT-006.md) | 账号生命周期与配置导入隔离 | F05,F06 | OPT-001, OPT-004 | queued |
| [OPT-007](OPT-007.md) | 可恢复 pull 与提交时冲突保护 | F09,F10,F11 | OPT-005, OPT-006 | queued |
| [OPT-008](OPT-008.md) | 内容身份与保守去重 | F12 | OPT-007 | queued |
| [OPT-009](OPT-009.md) | 可靠订阅写回和能力声明 | F17 | OPT-006, OPT-008 | queued |
| [OPT-010](OPT-010.md) | 配置快照合并与镜像语义 | F18 | OPT-006, OPT-009 | queued |
| [OPT-011](OPT-011.md) | AI 设置单点与输入范围一致 | F13,refreshInterval | OPT-006 | queued |
| [OPT-012](OPT-012.md) | 阅读器源码与浮层交互 | F14,closeAsk | 无 | verified-ci |
| [OPT-013](OPT-013.md) | AI 完成状态和缓存恢复 | F15 | OPT-002, OPT-011 | queued |
| [OPT-014](OPT-014.md) | 更新检查与凭据失败关闭 | F16,F22 | 无 | coding |
| [OPT-015](OPT-015.md) | 窗口启动恢复与幂等媒体键 | F21,F23 | 无 | queued |
| [OPT-016](OPT-016.md) | 行为回归分层与依赖卫生 | 测试网,依赖,CI | OPT-003, OPT-012, OPT-013, OPT-015 | queued |
| [OPT-017](OPT-017.md) | 有基线的搜索和媒体性能 | FTS,画廊,缓存,观测 | OPT-007, OPT-008, OPT-016 | queued |
| [OPT-018](OPT-018.md) | 全量收口和发布前验收 | 全部审计项 | OPT-001, OPT-002, OPT-003, OPT-004, OPT-005, OPT-006, OPT-007, OPT-008, OPT-009, OPT-010, OPT-011, OPT-012, OPT-013, OPT-014, OPT-015, OPT-016, OPT-017 | queued |

状态：queued / coding / review / rework / verified-local / verified-ci / integrated。未取得完整作用域证据的卡片不得记 complete。卡片中的写集可在读码后由主控明确调整，不允许执行者自行扩展。

## 每卡固定验收

1. 主控验证触发链与 Note 归属，发出明确需求、非目标、写集、负例和门禁。
2. CodeBuddy 先写行为反例再修复，运行其能承担的检查；不能伪造未跑结果、忽略失败或用源码 contains 代替关键行为。
3. 独立 reviewer 只读检查需求匹配、真实 diff、迁移/错误/并发边界；不能自己改代码或派嵌套 reviewer。
4. 有 P1/P2 未关闭必须返工；主控独立复跑并核对越界修改，维护同批 Note。
5. npm lint/build/test:frontend 与 cargo fmt 本地 + CI；完整 cargo clippy/test 仍由 windows-latest CI 承担；本轮独立执行已证实本机也可以链接，后续同样本地复跑。孤立探针不能冒充全项目门禁。
6. 模拟数据/隔离数据库可自行测试；真实账号、用户 DB、订阅删除、付费 AI 调用不纳入无边界测试。需要真实平台验收时记录缺口而不是假通过。

## 证据与任务卡

长期决定写 .agents/notes，优先更新原决定；任务卡只记录交付范围/状态/验证引用，不重建旧的工作流引擎。临时完整 CLI 输出与探针保存在被忽略的 tmp/optimization-20261008；可持久追溯的结论、提交/CI 标识写入卡片。

参考：[审计基线](../audit-2026-10-08.md)、[协作决定](../../.agents/notes/implemented/process/2026-10-08-主控与代码执行分离.md)。
