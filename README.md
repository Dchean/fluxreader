# FluxReader

本地优先的 Windows 桌面 RSS 客户端，使用 Tauri 2、Rust、React、TypeScript、Zustand 与 SQLite。现有核心能力包括五种内容布局、Google Reader / Fever 协议同步，以及 AI 摘要和翻译。

初始测试与构建基线已完成。项目按“管理 agent 负责规划、调度和独立验收，Claude Code 负责代码”的方式交接；低风险工作按授权自动推进，高风险、超限、合并与发布由用户确认。本次交接准备未启动编码批次，实际执行状态以 PROJECT 为准。

## 从这里开始

- [交接入口](docs/HANDOFF.md)：如何在其他管理 agent 中开始，以及 Claude 的调用与验收方式。
- [管理 agent 启动指令](docs/prompts/MANAGER-START.md)：复制到具备项目文件与终端能力的新会话。
- [文档入口](docs/README.md)：事实、需求、架构、验证和流程的导航。
- [项目状态](tasks/PROJECT.json)：当前阶段、用户决定和待批准动作。
- [执行规则](AGENTS.md)：适用于不同 agent 的公共约定。
- [任务入口](tasks/README.md)：当前交付、下一步任务和跨 agent 接手提示。
- [初始基线](docs/BASELINE.md)：前端 8/8、Rust 95 项通过；格式检查失败，未运行范围明确记录。

## 功能范围

核心必须保留：文章、社交、画廊、播客、通知五布局；面向 FreshRSS / Miniflux 的 Google Reader / Fever 同步；AI 摘要和翻译。

用户已确认保留 OPT-001～003、OPT-005～010：直连抓取、全文提取、OPML、音视频播放、桌面集成、搜索与快捷操作、图片与富媒体、个性化和缓存/去重能力。OPT-004 也保留，但 Gist/WebDAV 同步范围收敛为订阅源与客户端设置数据；允许同步服务地址、模型等非敏感配置，不同步 API Key、密码等敏感凭据。详见 [功能清单](docs/FEATURES.md)。

协议客户端存在不等于所有服务端、版本和操作均已通过验证，具体记录见 [接口与兼容性矩阵](docs/API.md)。

## 代码与文档

| 路径 | 职责 |
| --- | --- |
| src/ | React 组件、Zustand 状态、IPC 封装与样式 |
| src-tauri/src/ | Tauri 入口、Rust 业务、协议客户端与 SQLite 数据访问 |
| src-tauri/tests/ | Rust 集成、迁移、回归及服务测试 |
| tools/ | 现有前端逻辑回归和 mock 工具 |
| docs/ | 产品边界、现状架构、基线、问题和流程 |
| tasks/ | 项目状态、任务定义、看板与运行证据 |
| .github/workflows/ | 当前 CI 和发布配置；本次没有修改 |

[当前架构](docs/ARCHITECTURE.md)、[数据模型](docs/DATA-MODEL.md) 与 [关键流程](docs/USER-FLOWS.md) 对应已记录的源码基准，不冒充目标设计或运行验证。

## 开发与检查入口

以下是项目已有入口，由管理 agent 在任务范围内选择执行；不要把测试构建与真实应用/服务验收混淆：

```text
npm ci
npm run tauri dev
npm run build
npm run lint
npm run test:frontend
npm run tauri build
```

npm run build 包含 TypeScript 项目构建检查和 Vite 构建。npm run test:frontend 是 Node 中的前端状态回归，不是桌面 UI E2E。

Rust 与完整基线命令、哪些测试需要本地服务或真实账号，见 [TEST-PLAN](docs/TEST-PLAN.md)。不要将全部 ignored 测试作为无外部依赖的统一门禁。

当前 CI 使用 Node 22；本机环境和 Rust 版本记录在 BASELINE。发布需要用户单独确认，见 [发布与回滚](docs/RELEASE-ROLLBACK.md)。

## License

MIT，见 [LICENSE](LICENSE)。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发，保留其版权声明。
