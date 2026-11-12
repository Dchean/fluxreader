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

| 信息 | 归属 |
| --- | --- |
| 难以从代码/测试恢复、未来仍需重访的重要理由与取舍 | [`.agents/notes/`](.agents/notes/)，一篇一个决定；普通局部修复不自动新增 |
| 需随产品长期维护的事实、协议矩阵、性能基线与发布操作 | [`docs/`](docs/README.md) |
| 仍存在的缺口、接受的边界和未完成验证 | [docs/roadmap.md](docs/roadmap.md) |
| 当轮范围、计数、提交与检查状态 | 既有交接/验收入口或当次交付，不复制进每篇主题 Note |

沿用现有路径 `{lifecycle}/{class}/yyyy-mm-dd-topic.md`；lifecycle 为 proposed / implemented / rejected / archived，class 为 feature / bug-fix / simplification / architecture / process / testing。日期是首次记录决定的日期，不随整理更新。活跃篇前三行是 `# Agent Note: <标题>`、空行、`Status: <状态>`，正文从 `## Problem` 开始，含 `## Alternatives considered`。implemented 保留 Decision 与 Consequences；备选只记真实考虑过的方案，无真实备选可明确写无，不凑数。

决定理由未变时更新归属篇事实；重要决定变化须保留原由和后继关系。部分取代明确各自仍有效的范围，不能把旧决定重写成从未作过的反面。`implemented` 不代表测试、审计或发布通过；历史验证需附日期/基线/范围，未验证如实保留。更新应通读相关收益、代价和验证边界，不只在顶部追加补丁。

仅当独特有效约束已由活跃记录承接或确已不适用，才归档。归档第 3 行保留 `Status: implemented`、第 4 行 `Archived: YYYY-MM-DD`，其余原正文保全；新封印只追加，旧正文和 manifest 条目冻结。旧 WCAG 档案的历史头块例外及首次封印原因见[现有记录](.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md)。不以重算哈希掩盖历史改写。

关键实现点保留能回到决定的反向注释，链接变化时同步必要注释，不为整理改产品逻辑：

```ts
// Note: 分页谓词必须与 ORDER BY 同构 — 见 .agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md
```

清理旧执行材料须先保全独特理由、当前约束及引用入口。已清出的历史审计/任务卡通过固定 Git 提交恢复，定位见 [docs/README.md](docs/README.md)；不能称作已存入一个实际没有这些文件的归档分支。记录技能不承担审批、调度或提交，执行仍遵守当前适用的协作规范与授权。

## 门禁

| 检查 | 在哪跑 |
| --- | --- |
| `npm run lint`（oxlint）、`npm run build`（tsc + vite）、`npm run test:frontend`（前端状态回归）、`npm run test:components`（组件真实挂载回归） | 本地与 CI |
| `npm run test:ui-layout`（Windows Edge headless + CDP：五布局 footer、七类社交正文（长文、data URI 图、HTTP 延迟图、HTTP 失败图、视频、短文、空正文）的固定外高/空态/展开往返/裁切/横向溢出、视频 `readyState=0 → metadata 自然尺寸 → poster 撑高`、1440×900 与 980×640；画廊两视口分波异步自然比例图片与指定可见锚漂移；另加 1536×900 / 1280×800 / 980×640 三档 J/K、Tab/Space、源行 padding 鼠标热区、AX 与响应式回归） | 本地与 CI（windows-latest `ui-layout` job；失败证据由 artifact 上传） |
| `cargo fmt --all -- --check`（在 `src-tauri/` 下执行） | 本地与 CI |
| `cargo clippy --all-targets -- -D warnings`、`cargo test`、`cargo check`（在 `src-tauri/` 下执行） | CI（windows-latest）；当前工作区的 `tmp/rust-env/` 隔离环境亦可本地执行 |

2026-11-03 起，当前工作区的隔离 Rust/MSVC 环境（`tmp/rust-env/`，rustc/cargo `1.98.1` + clang-cl/lld-link）已可真实执行 `cargo test` / `cargo check` / `cargo clippy`。这是当前工作区的证据，不外推到其他机器；CI 的 rust job（windows-latest）仍是最终跨环境门禁。本地必须跑 `cargo fmt --all -- --check` 与前端门禁，Rust 全量检查可由隔离环境执行或由 CI 承担——**每一项都必须有承担者**，不能出现「本地不跑、CI 也不跑」的检查。CI 执行一次 `cargo test --no-fail-fast`，覆盖全部未忽略的单元与集成测试（含 6 个受约束的 mock 套件及 ingestion/scheduler 自托管测试）；测试目标失败后仍执行其余目标，最终退出仍非零。`#[ignore]` 的 live 测试（真实服务端）需在具体任务里单独约定读写边界后执行，不作为统一门禁。

CI 固定在 Windows 上跑 Rust：这是纯 Windows 客户端，DPAPI 凭据加密与 SMTC 媒体控制等 Windows 专有功能只在 Windows 编译；在 Linux 上检查的是 `#[cfg(not(windows))]` 降级分支，真正的生产代码路径反而被跳过。

## 仓库卫生

- 不要提交 `node_modules/`、`dist/`、`src-tauri/target/`、`*.db`、`tmp/`（都已在 `.gitignore` 中；`.zcodeignore` 为工具自生成的本机可选配置，不入库）。
- 一次性探针与完整调研输出不进主线；既有材料位于存档分支 `archive/tooling-and-reports`，本机新审计证据保留在忽略的 `tmp/`。历史审计/任务卡另按固定提交追溯，见下方索引。
- 历史材料的定位（含 2026-09-16 历史重写前后的提交号对照）见 [docs/README.md](docs/README.md) 的「历史材料」节。
