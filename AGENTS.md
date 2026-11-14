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

本段只规定流向，**不授予推送、合并或发布权限**；每次合并与发布按上位规则单独确认。

## 提交信息

`<type>(<scope>): <中文主题>`，type 取 `feat` / `fix` / `docs` / `style` / `refactor` / `perf` / `test` / `build` / `ci` / `chore`。主题一行说清改了什么，细节与动机写正文；破坏性变更以 `BREAKING CHANGE:` 开头说明。

## 文档约定

| 信息 | 归属 |
| --- | --- |
| 难以从代码/测试恢复、未来仍需重访的重要理由与取舍 | [`.agents/notes/`](.agents/notes/)，一篇一个决定；普通局部修复不自动新增 |
| 需随产品长期维护的事实、协议矩阵、性能基线与发布操作 | [`docs/`](docs/README.md) |
| 仍存在的缺口、接受的边界和未完成验证 | [docs/roadmap.md](docs/roadmap.md) |
| 契约单点、改动影响面与同步面 | [docs/contracts.md](docs/contracts.md) |
| 维护流程、证据标准与文档写作规则 | [docs/maintenance.md](docs/maintenance.md) |
| 当轮范围、计数、提交与检查状态 | 既有交接/验收入口或当次交付，不复制进每篇主题 Note |

沿用现有路径 `{lifecycle}/{class}/yyyy-mm-dd-topic.md`；lifecycle 为 proposed / implemented / rejected / archived，class 为 feature / bug-fix / simplification / architecture / process / testing。日期是首次记录决定的日期，不随整理更新。活跃篇前三行是 `# Agent Note: <标题>`、空行、`Status: <状态>`，正文从 `## Problem` 开始，含 `## Alternatives considered`。implemented 保留 Decision 与 Consequences；备选只记真实考虑过的方案，无真实备选可明确写无，不凑数。

决定理由未变时更新归属篇事实；重要决定变化须保留原由和后继关系。部分取代明确各自仍有效的范围，不能把旧决定重写成从未作过的反面。`implemented` 不代表测试、审计或发布通过；历史验证需附日期/基线/范围，未验证如实保留。更新应通读相关收益、代价和验证边界，不只在顶部追加补丁。

仅当独特有效约束已由活跃记录承接或确已不适用，才归档。归档第 3 行保留 `Status: implemented`、第 4 行 `Archived: YYYY-MM-DD`，其余原正文保全；新封印只追加，旧正文和 manifest 条目冻结。旧 WCAG 档案的历史头块例外及首次封印原因见[现有记录](.agents/notes/implemented/process/2026-11-03-修复波次的分批、证伪审计与证据门槛.md)。不以重算哈希掩盖历史改写。

关键实现点保留能回到决定的反向注释，链接变化时同步必要注释，不为整理改产品逻辑：

```ts
// Note: 分页谓词必须与 ORDER BY 同构 — 见 .agents/notes/implemented/architecture/2026-10-05-查询与分页契约.md
```

清理旧执行材料须先保全独特理由、当前约束及引用入口。已清出的历史审计/任务卡通过固定 Git 提交恢复，定位见 [docs/README.md](docs/README.md)；不能称作已存入一个实际没有这些文件的归档分支。记录技能不承担审批、调度或提交，执行仍遵守当前适用的协作规范与授权。

## 维护与文档纪律

- 定位先查 [docs/contracts.md](docs/contracts.md)（契约单点与影响面），再按 [docs/README.md](docs/README.md) 的"任务 → 必读"挑篇目；不要通读 `docs/`。
- **文档处置分三种**：①代码改动使文档过时 → 同批直接更新，不请批；②文档与代码不一致且无法判断哪边对 → 报告差异与影响，由用户决定改代码还是改文档；③错字、格式、失效链接 → 直接修。**不许把②静默当成①**，否则会把真 bug 洗成`文档笔误`。暂不更新的登记到 [docs/roadmap.md](docs/roadmap.md) 的「文档待办」，写明下次触碰该模块时必须处理。
- 代码、测试、契约表、文档、索引在**同一批**完成；做不到就当场登记到 [docs/roadmap.md](docs/roadmap.md)，不许口头欠账。
- 文档只写稳定事实与边界：**不写行号**（用"文件 + 符号名"或 `<a id>` 锚），能由类型或测试表达的不重复写进文档。
- 审计与证伪：新发现必须由另一个独立上下文再证伪一次；被推翻的结论留档。**审计者只交报告与摘要，不写文件**；由实现者把摘要登记到 [docs/audits.md](docs/audits.md)、原始输出留 `tmp/`。
- 证据要可复现：基线 SHA、命令、输入、结果、性质（运行 / 静态 / 未验证）五样齐，细则见 [docs/maintenance.md](docs/maintenance.md)。
- `npm run docs:check` 检查索引覆盖、相对链接与锚点、行号禁令；本地与 CI 都必须跑。

## 门禁

本地必跑 `npm run lint`、`npm run build`、`npm run test:frontend`、`npm run test:components`、`npm run test:ui-layout`（Windows Edge headless）、`npm run docs:check` 与 `cargo fmt --all -- --check`（在 `src-tauri/` 下执行）。Rust 全量 `cargo clippy --all-targets -- -D warnings`、`cargo check`、`cargo test --no-fail-fast` 由 CI 的 windows-latest job 承担，本地可用 `tmp/rust-env/` 隔离环境执行。

每一项检查都必须有承担者，不能出现“本地不跑、CI 也不跑”。`#[ignore]` 的真实服务端与容量测试需在具体任务里单独约定读写边界后执行，不作为统一门禁；不得为变绿删测试、加跳过或放宽断言。命令、脚本与 CI job 的对应、测试分层、证据纪律与常见故障见 [docs/development.md](docs/development.md)。

## 仓库卫生

- 不要提交 `node_modules/`、`dist/`、`src-tauri/target/`、`*.db`、`tmp/`（都已在 `.gitignore` 中；`.zcodeignore` 为工具自生成的本机可选配置，不入库）。
- 一次性探针与完整调研输出不进主线；既有材料位于存档分支 `archive/tooling-and-reports`，本机新审计证据保留在忽略的 `tmp/`。历史审计/任务卡另按固定提交追溯，见下方索引。
- 历史材料的定位（含 2026-09-16 历史重写前后的提交号对照）见 [docs/README.md](docs/README.md) 的「历史材料」节。
