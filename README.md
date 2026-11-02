# FluxReader

FluxReader 是一款本地优先的 Windows 桌面 RSS 客户端：全部数据存于本机 SQLite，直连抓取订阅源，支持与 FreshRSS / Miniflux 双向同步。基于 Tauri 2 构建，后端为 Rust，前端为 React + TypeScript。

## 功能特性

- **内容浏览**：文章、社交、画廊、播客、通知五种布局，配合全文搜索与个性化设置。
- **双协议同步**：对接 FreshRSS / Miniflux，同时支持 Google Reader 与 Fever 两种协议；读状态与收藏在客户端与服务端之间保持同步，各方向的冲突政策见[同步兼容矩阵](docs/sync-compat-matrix.md)。
- **AI 摘要与翻译**：接入自有 AI 服务，对文章生成摘要、进行翻译。
- **全文提取与 OPML**：对订阅源条目抓取全文，支持 OPML 订阅列表导入导出。
- **播客播放**：内置音视频播放器，直接收听订阅的播客节目。
- **桌面集成**：系统托盘、SMTC 媒体键控制、系统通知、开机自启与窗口状态记忆。
- **配置同步**：通过 Gist 或 WebDAV 在多台设备间同步订阅源与客户端设置；AI 配置（含模型名）以及 API Key、密码等敏感凭据始终保留在本机，不参与同步。

## 安装

前往 GitHub [Releases](https://github.com/Dchean/fluxreader/releases) 下载 Windows x64 安装包：`setup.exe` 或 `.msi`。

注意：安装包尚未进行 Windows 代码签名，首次运行时系统可能弹出未知发布者的提示。

## 快速开始

1. 安装并启动 FluxReader。
2. 在设置中添加同步账号：填入 FreshRSS 或 Miniflux 的服务地址与凭据（也可以先离线使用，手动添加本地订阅源）。
3. 执行一次同步，将远端订阅与文章拉取到本地。
4. 切换五种布局浏览内容，标读、收藏与分类等操作会随同步回写，与服务端保持一致。

## 开发

开发统一使用 Node 22（主版本记录在仓库根目录 [.node-version](.node-version)）：

```bash
npm ci
npm run tauri dev
```

前端门禁（本地与 CI 均执行）：

```bash
npm run lint             # oxlint 静态检查
npm run build            # tsc -b + vite build，类型检查与构建
npm run test:frontend    # Node 中的前端状态回归
npm run test:components  # 组件真实挂载回归
```

Rust 侧命令在 `src-tauri/` 下执行。本机环境不支持链接 Rust 产物，本地只执行 `cargo fmt --all -- --check`；`cargo clippy`、`cargo test` 与 `cargo check` 由 CI 的 windows-latest rust job 承担。

发布流程与版本约定见 [docs/release.md](docs/release.md)。

源码结构：

| 路径 | 职责 |
| --- | --- |
| `src/` | React 组件、Zustand 状态、IPC 封装与样式 |
| `src-tauri/src/` | Tauri 入口、Rust 业务、协议客户端与 SQLite 数据访问 |
| `src-tauri/tests/` | Rust 集成、迁移、回归及服务端协议测试 |
| `tools/` | 前端状态回归与组件挂载回归的运行器 |

## 项目文档

| 文档 | 内容 |
| --- | --- |
| [docs/sync-compat-matrix.md](docs/sync-compat-matrix.md) | Google Reader / Fever 协议与 FreshRSS、Miniflux 的兼容矩阵，及冲突政策与已知实现差异 |
| [docs/roadmap.md](docs/roadmap.md) | 当前缺口与已知限制 |
| [docs/release.md](docs/release.md) | 发布序列、tag 附注结构与提交信息约定 |
| [.agents/notes/](.agents/notes/) | 各项设计决定的记录：为什么这样做、当时放弃了哪些方案 |

## License

本项目以 [MIT](LICENSE) 协议发布。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT 协议）二次开发，保留其版权声明。
