# FluxReader

本地优先的 Windows 桌面 RSS 客户端，使用 Tauri 2、Rust、React、TypeScript、Zustand 与 SQLite。核心能力：文章 / 社交 / 画廊 / 播客 / 通知五种内容布局，面向 FreshRSS / Miniflux 的 Google Reader / Fever 协议同步，AI 摘要与翻译，全文提取，OPML 导入导出，播客播放与桌面集成。

## 分支与发布

- `main`：纯项目代码主分支，不含工作流内容；版本 bump 与发布 tag 都在这里进行。
- `dev`：开发分支，承载 workflow-kit 工作流（状态、任务、用户决定见其 `.workflow-kit/`）与全部开发记录。
- 一切调整先在 `dev` 提交、推送并验证，通过后再合并进 `main`；合并不得把工作流内容带入 `main`，步骤见 [AGENTS.md](AGENTS.md)。
- 安装包：GitHub [Releases](https://github.com/Dchean/fluxreader/releases) 提供 Windows x64 `setup.exe` 与 `.msi`。发布序列：dev 验证 → 合并进 main → bump 提交单独推送 → CI 绿 → annotated tag（`v*`）→ Release 工作流自动构建，Release 正文取 tag 附注。

## 从这里开始

- [执行规则](AGENTS.md)：分支流向约定与各 agent 公共约定入口；dev 分支上同时是 workflow-kit 工作流入口（新会话从那里开始）。
- 旧版工作流系统（旧 docs/、tasks/、.agents/ 笔记体系、旧 workflow-kit 安装）已于 2026-09-15 按用户决定清理，原文可在 git 历史查阅。

## 功能范围

核心必须保留：文章、社交、画廊、播客、通知五布局；面向 FreshRSS / Miniflux 的 Google Reader / Fever 同步；AI 摘要和翻译。

用户已确认保留 OPT-001～003、OPT-005～010：直连抓取、全文提取、OPML、音视频播放、桌面集成、搜索与快捷操作、图片与富媒体、个性化和缓存/去重能力。OPT-004 也保留，但 Gist/WebDAV 同步范围收敛为订阅源与客户端设置数据；白名单内允许同步服务地址等非敏感连接配置，AI 配置（含模型名）不在白名单、不同步，API Key、密码等敏感凭据一律不同步（原功能清单 docs/FEATURES.md 在 git 历史中）。

协议客户端存在不等于所有服务端、版本和操作均已通过验证；旧接口与兼容性矩阵（docs/API.md）在 git 历史中。

## 代码与文档

| 路径 | 职责 |
| --- | --- |
| src/ | React 组件、Zustand 状态、IPC 封装与样式 |
| src-tauri/src/ | Tauri 入口、Rust 业务、协议客户端与 SQLite 数据访问 |
| src-tauri/tests/ | Rust 集成、迁移、回归及服务测试 |
| tools/ | 现有前端逻辑回归和 mock 工具 |
| .workflow-kit/ | 仅 dev 分支：当前工作流状态、任务队列与流程文档 |
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

## 提交与发布文案约定

- 提交信息：`<type>(<scope>): <中文主题>`，type 取 feat / fix / docs / style / refactor / perf / test / build / ci / chore；主题一行说清改了什么，细节与动机写正文，破坏性变更以 `BREAKING CHANGE:` 开头说明。
- 业务提交与工作流记录提交分开：`chore(workflow)` 仅用于 `.workflow-kit/` 记录，且只出现在 dev 分支。
- 发布说明（v0.16.0 起）：annotated tag 附注即 GitHub Release 正文（release.yml 自动提取），模板为「标题行 → 距上版提交数与主题 → 行为修复 / 健壮性 / 一致性统一 / 测试与基建 分节 → 门禁数字与独立审查结论 → 已知限制（沿承上版）」。

## License

MIT，见 [LICENSE](LICENSE)。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发，保留其版权声明。
