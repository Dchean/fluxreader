# 执行规则（FluxReader）

## 分支流向

- `main`：主分支。版本 bump 与发布 tag 都在这里。
- `dev`：开发分支。一切调整先在 `dev` 提交、推送并验证，通过后合并进 `main`。
- 两个分支的项目内容保持一致（代码、文档、`.agents/notes/`）；合并就是常规合并，没有需要剔除的目录。

```text
# 在 dev 上完成改动并推送，等 CI 绿
git merge --no-ff dev          # 在 main 上执行
git push origin main
```

发布序列、tag 附注结构与存量版本说明见 [docs/release.md](docs/release.md)。

## 提交信息

`<type>(<scope>): <中文主题>`，type 取 `feat` / `fix` / `docs` / `style` / `refactor` / `perf` / `test` / `build` / `ci` / `chore`。主题一行说清改了什么，细节与动机写正文；破坏性变更以 `BREAKING CHANGE:` 开头说明。

## 文档约定

| 想写什么 | 写在哪 |
| --- | --- |
| 为什么这样改、当时放弃了哪些方案 | [`.agents/notes/`](.agents/notes/)，一篇一个决定 |
| 需要长期与代码同步维护的事实（协议矩阵、性能基线、发布流程） | [`docs/`](docs/README.md) |
| 还没做的事、已知限制 | [docs/roadmap.md](docs/roadmap.md) |

Note 的路径即分类：`{lifecycle}/{class}/yyyy-mm-dd-topic.md`，lifecycle 取 `proposed` / `implemented` / `rejected` / `archived`，class 取 `feature` / `bug-fix` / `simplification` / `architecture` / `process` / `testing`。每篇前三行是 `# Agent Note: <标题>` + 空行 + `Status: <状态>`，正文以 `## Problem` 开头，必含 `## Alternatives considered`。

**关键实现点要留反向注释**，从代码能走回决定：

```ts
// Note: 分页谓词必须与 ORDER BY 逐字同构 — 见 .agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md
```

改代码时若发现某篇 Note 的事实已过时，**就地更新那一篇**；方案被完全取代时才新建一篇并把旧的移到 `archived/`。

## 门禁

| 检查 | 在哪跑 |
| --- | --- |
| `npm run lint`（oxlint）、`npm run build`（tsc + vite）、`npm run test:frontend`（前端状态回归） | 本地与 CI |
| `cargo fmt --all -- --check` | 本地与 CI |
| `cargo clippy --all-targets -- -D warnings`、`cargo test` | CI（windows-latest） |

本机没有 MSVC 链接器，`cargo test` 与 clippy 无法链接，因此由 CI 的 rust job 承担——**每一项都必须有承担者**，不能出现「本地不跑、CI 也不跑」的检查。CI 只跑 6 个受约束的 mock 集成套件与默认测试集；`#[ignore]` 的 live 测试（真实服务端）需在具体任务里单独约定读写边界后执行，不作为统一门禁。

CI 固定在 Windows 上跑 Rust：这是纯 Windows 客户端，DPAPI 凭据加密与 SMTC 媒体控制等 Windows 专有功能只在 Windows 编译；在 Linux 上检查的是 `#[cfg(not(windows))]` 降级分支，真正的生产代码路径反而被跳过。

## 仓库卫生

- 不要提交 `node_modules/`、`dist/`、`src-tauri/target/`、`*.db`、`tmp/`（都已在 `.gitignore` / `.zcodeignore` 中）。
- 一次性探针与调研报告放在存档分支 `archive/tooling-and-reports`，不进主线。
- 历史材料的定位（含 2026-09-16 历史重写前后的提交号对照）见 [docs/roadmap.md](docs/roadmap.md) 第七节。
