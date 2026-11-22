# 执行规则（FluxReader）

Tauri 2 + React 19 + TS + Zustand + SQLite 的 Windows RSS 阅读器。本文件只放**每次都要遵守**的东西；模块地图看 `.agents/map.md`，拍板过的死规矩看 `.agents/decisions.md`，坑看 `.agents/LESSONS.md`，当前停点看 `.agents/state.md`。

## 常用命令

- 前端：`npm run dev` / `npm run build`（tsc + vite）/ `npm run lint`（oxlint）
- 前端回归：`npm run test:frontend`、`npm run test:components`、`npm run test:ui-layout`（Windows Edge headless）
- 文档门禁：`npm run docs:check`（索引覆盖、相对链接与锚点、docs/ 内禁止行号）
- Rust：在 `src-tauri/` 下 `cargo fmt --all -- --check`；全量 `cargo clippy --all-targets -- -D warnings`、`cargo check --all-targets`、`cargo test --no-fail-fast`（本地可用隔离环境 `tmp/rust-env/`，CI 由 windows-latest 承担）
- 发布脚本回归：`node --test tools/release/release.test.mjs`

## 日志与数据隔离

- **日志真实通道**：本项目**没有** `.dev-logs/`。debug 构建由 `tauri-plugin-log` 输出 stdout + 应用日志目录；同步诊断是自建环形日志，写在 `%APPDATA%\com.fluxreader.app\diagnostics\`（每片 256 KiB × 3）。前端日志只有 console。
- **数据落盘**：SQLite 在 `%APPDATA%\com.fluxreader.app\fluxreader-v2.db`（旧 `fluxreader.db` 不迁移）；备份在 `backups\`。**没有** `.dev-data/` 目录约定——开发要用隔离数据时，用环境变量把测试根指到仓库外，别写用户真实目录。
- `tmp/`、`*.log`、`*.db`、`.dev-logs/`、`.dev-data/`、`.env*`（除 `.env.example`）、`.agents/tmp/`、`.pre-v6-backup/`、`*.local.md`、`*.local.json` 一律不入库。

## 全局红线（≤10 条）

1. **不得为变绿删测试、加 `#[ignore]` 或放宽断言。** 改门禁标准要先有依据、经确认与独立审计。
2. **可渲染 HTML 只走 `sanitize()` 一个口径**，写入口与读边界共用；不把属性串拼成标签。
3. **凭据失败关闭**：DPAPI 加解密失败一律 Err，绝不回退明文；读取错误 ≠ 未配置。
4. **AI 接入只用项目支持的协议**：Base URL / Key / 模型由用户配置，不预填默认模型、不假定官方端点、不静默覆盖已有配置。
5. **写事务只走 `db::write_transaction`**；锁序固定 业务锁 → writer，不反向获取。
6. **同步冲突方向只在 `sync/conflict_policy.rs` 定义**；"失败"不许折成"空集合"。
7. **外链只经 `openExternalWithFeedback`，仅 HTTP/HTTPS**；主页面只放行应用 origin。
8. **不为未验证的极端假设加兼容层、开关或检查流程**；只记录为提示。
9. **一次性探针与完整调研输出不进主线**，证据放忽略的 `tmp/`。
10. **不扩大推送 / 合并 / 打标签 / 发布权限**；每次合并与发布单独确认。

## 动作触发式路由表

| 你要做的事 | 先读 |
| --- | --- |
| 定位"改这个会影响谁" | `docs/contracts.md`（契约单点与影响面） |
| 找模块、公共组件、收口点 | `.agents/map.md` |
| 确认某条规矩是不是用户拍板的 | `.agents/decisions.md`（**优先于任何半成品代码**） |
| 排查"这坑以前踩过没" | `.agents/LESSONS.md` |
| 接手正在做的切片 | `.agents/state.md` + `.agents/specs/` |
| 改数据库结构 / 迁移 / 查询分页 | `docs/architecture.md`、`docs/performance.md` |
| 改同步协议 / 冲突政策 | `docs/sync-compat-matrix.md` |
| 改备份 / 便携 / WebDAV | `docs/backup.md` |
| 改前端组件 / 类名 / 令牌 | `docs/frontend.md` |
| 改门禁 / 测试 / 工具链 | `docs/development.md` |
| 做发布 | `docs/release.md` |
| 查已知缺口与未验证范围 | `docs/roadmap.md` |
| 查历史决定原由 | `.agents/decisions.md` 的对应条目 → 必要时才读归档 |

**日常严禁读取 `.agents/archive/`。** 那里是换轨前的旧工作流长文与过期交接快照，只用于事后追溯；读到的任何内容都**不是当前事实**，引用时必须注明"来自归档"。

## 分支与提交

- `main` 主分支（版本 bump 与发布 tag）；`dev` 开发分支（改动先在这里提交、推送、验证，通过后 `git merge --no-ff dev` 进 `main`）。两分支项目内容保持一致。
- 提交信息：`<type>(<scope>): <中文主题>`，type 取 feat / fix / docs / style / refactor / perf / test / build / ci / chore；细节与动机写正文，破坏性变更以 `BREAKING CHANGE:` 开头。

## 证据与文档纪律

- 证据五要素：基线 SHA、命令、输入、结果、性质（运行 / 静态 / 未验证）。缺一只是线索。
- 独立审计者只读、只交报告；被推翻的结论留档。
- 文档只写稳定事实与边界，**不写行号**（用"文件 + 符号名"或 `<a id>` 锚）。
- 代码、测试、契约表、文档、索引同一批完成；做不到就登记到 `docs/roadmap.md` 的「文档待办」，不许口头欠账。
- 文档与代码不一致且分不清哪边对时：报告差异与影响，由用户裁决，不许静默当成"文档笔误"。
