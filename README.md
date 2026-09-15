# FluxReader

本地优先的 Windows 桌面 RSS 客户端，使用 Tauri 2、Rust、React、TypeScript、Zustand 与 SQLite。现有核心能力包括五种内容布局、Google Reader / Fever 协议同步，以及 AI 摘要和翻译。

当前工作流为 workflow-kit（2026-09-15 接入）：新会话从 [WORKFLOW-KIT.md](WORKFLOW-KIT.md) 开始；当前主会话是总控，目标、权限、预算与用户决定以 `.workflow-kit/` 下的状态文件为准。

## 从这里开始

- [工作流入口](WORKFLOW-KIT.md)：新会话从这里开始，运行 `python .workflow-kit/scripts/project_workflow.py start --root .` 查看阶段与任务状态。
- [执行规则](AGENTS.md)：各 agent 公共约定入口。
- [项目状态](.workflow-kit/tasks/PROJECT.json)、[用户决定](.workflow-kit/tasks/DECISIONS.json)、[权限与分工](.workflow-kit/tasks/POLICY.json)、[需求确认](.workflow-kit/tasks/BRIEF.json)。
- 旧版工作流系统（旧 docs/、tasks/、.agents/ 笔记体系、旧 workflow-kit 安装）已于 2026-09-15 按用户决定清理，原文可在 git 历史查阅。

## 功能范围

核心必须保留：文章、社交、画廊、播客、通知五布局；面向 FreshRSS / Miniflux 的 Google Reader / Fever 同步；AI 摘要和翻译。

用户已确认保留 OPT-001～003、OPT-005～010：直连抓取、全文提取、OPML、音视频播放、桌面集成、搜索与快捷操作、图片与富媒体、个性化和缓存/去重能力。OPT-004 也保留，但 Gist/WebDAV 同步范围收敛为订阅源与客户端设置数据；允许同步服务地址、模型等非敏感配置，不同步 API Key、密码等敏感凭据（原功能清单 docs/FEATURES.md 在 git 历史中）。

协议客户端存在不等于所有服务端、版本和操作均已通过验证；旧接口与兼容性矩阵（docs/API.md）在 git 历史中。

## 代码与文档

| 路径 | 职责 |
| --- | --- |
| src/ | React 组件、Zustand 状态、IPC 封装与样式 |
| src-tauri/src/ | Tauri 入口、Rust 业务、协议客户端与 SQLite 数据访问 |
| src-tauri/tests/ | Rust 集成、迁移、回归及服务测试 |
| tools/ | 现有前端逻辑回归和 mock 工具 |
| .workflow-kit/ | 当前工作流状态、任务队列与流程文档 |
| workflow-kit/ | workflow-kit 启动包（接入工具来源，维护时处理） |
| .github/workflows/ | 当前 CI 和发布配置 |

旧架构、数据模型与用户流程文档（docs/ARCHITECTURE.md、DATA-MODEL.md、USER-FLOWS.md）在 git 历史中，对应已记录的源码基准，不冒充目标设计或运行验证。

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

Rust 完整基线命令与哪些测试需要本地服务或真实账号，见 git 历史中的 docs/TEST-PLAN.md。不要将全部 ignored 测试作为无外部依赖的统一门禁。

当前 CI 使用 Node 22；发布需要用户单独确认。

## License

MIT，见 [LICENSE](LICENSE)。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发，保留其版权声明。
