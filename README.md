# FluxReader

本地优先的 Windows 桌面 RSS 客户端，使用 Tauri 2、Rust、React、TypeScript、Zustand 与 SQLite。核心能力：文章 / 社交 / 画廊 / 播客 / 通知五种内容布局，面向 FreshRSS / Miniflux 的 Google Reader / Fever 协议同步，AI 摘要与翻译，全文提取，OPML 导入导出，播客播放与桌面集成。

## 从这里开始

- [执行规则](AGENTS.md)：分支流向、提交信息、文档约定与门禁。
- [文档索引](docs/README.md)：`.agents/notes/`（决定与取舍）与 `docs/`（协议矩阵、性能基线、发布流程）的分工。
- [未做事项与已知限制](docs/roadmap.md)。
- 安装包：GitHub [Releases](https://github.com/Dchean/fluxreader/releases) 提供 Windows x64 `setup.exe` 与 `.msi`。

## 功能范围

核心必须保留：文章、社交、画廊、播客、通知五布局；面向 FreshRSS / Miniflux 的 Google Reader / Fever 同步；AI 摘要和翻译。

用户已确认保留：直连抓取、全文提取、OPML、音视频播放、桌面集成、搜索与快捷操作、图片与富媒体、个性化和缓存/去重能力。Gist/WebDAV 配置同步的范围收敛为订阅源与客户端设置数据；白名单内允许同步服务地址等非敏感连接配置，AI 配置（含模型名）不在白名单、不同步，API Key、密码等敏感凭据一律不同步。

协议客户端存在不等于所有服务端、版本和操作均已通过验证——哪些能力被实测覆盖、哪些只是协议支持，见[兼容矩阵](docs/sync-compat-matrix.md)。

## 代码与文档

| 路径 | 职责 |
| --- | --- |
| `src/` | React 组件、Zustand 状态、IPC 封装与样式 |
| `src-tauri/src/` | Tauri 入口、Rust 业务、协议客户端与 SQLite 数据访问 |
| `src-tauri/tests/` | Rust 集成、迁移、回归及服务测试 |
| `tools/` | 前端状态回归（`npm run test:frontend`）与 mock 服务工具 |
| `.agents/notes/` | 决定记录：为什么这样改、放弃了哪些方案 |
| `docs/` | 需要跟代码同步维护的事实：兼容矩阵、性能基线、发布流程、待办 |
| `.github/workflows/` | CI 与发布配置 |

## 开发与检查入口

```text
npm ci
npm run tauri dev
npm run build            # tsc -b + vite build
npm run lint             # oxlint
npm run test:frontend    # Node 中的前端状态回归，不是桌面 UI E2E
npm run tauri build
```

Rust 侧：`cargo fmt --all -- --check` 可在本地跑；`cargo clippy --all-targets -- -D warnings` 与 `cargo test` 需要 MSVC 链接器，由 CI 的 windows-latest rust job 承担。`#[ignore]` 的 live 测试需要真实同步后端，不作为统一门禁。

当前 CI 使用 Node 22。发布需要单独确认，流程见 [docs/release.md](docs/release.md)。

## 已知限制

摘要见 [docs/roadmap.md](docs/roadmap.md)：用户可见缺口（关于页检查更新、`refreshInterval` 值域、滚动标读的前置条件等）、流程与基建待办（CI 覆盖、回归网拆分、组件测试通道）、以及两条**已评估并明确不修**的边界。

## License

MIT，见 [LICENSE](LICENSE)。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发，保留其版权声明。
