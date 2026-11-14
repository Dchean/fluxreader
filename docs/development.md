# 开发与门禁

本页记录本地如何运行、每项检查由谁承担、证据如何保留，以及隔离环境里已经踩过的坑。分支流向与提交格式见 [AGENTS.md](../AGENTS.md)，发布步骤见 [release.md](release.md)。

## 本地环境

- Windows x64；Node 版本由 `.node-version` 固定（当前 22），CI 用 `actions/setup-node@v4` 的 `node-version-file` 读取同一文件。不要用本机更新的 Node 大版本直接判定门禁结果。
- 安装依赖与脚本：`npm ci`（或 `npm install`）后使用 `package.json` 中的 `lint`、`build`、`test:frontend`、`test:components`、`test:ui-layout`。
- Edge headless 由 `npm run test:ui-layout` 自动查找；本机路径通常是 `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`。
- Rust 使用工作区隔离环境 `tmp/rust-env/`（rustc/cargo `1.98.1` + clang-cl/lld-link），入口 `activate-rust-env.ps1`；它只服务当前工作区，不外推到其他机器。
- 隔离环境里跑 Cargo 时通常前置 `C:\Users\<user>\.cargo\bin` 到 `PATH`，并设 `CARGO_NET_OFFLINE=true`；不要在没有授权时联网更新依赖。
- 本机临时证据放忽略的 `tmp/`，内容随任务变化，**不在文档里枚举目录清单**（枚举会过期）；需要长期保留的结论摘要登记到 [审计记录](audits.md)。

## 门禁与 CI job 对应

| 检查 | 命令 | 运行位置 | 对应 CI job（`ci.yml`） |
| --- | --- | --- | --- |
| 前端静态检查 | `npm run lint`（oxlint） | 本地与 CI | Lint, build & frontend regressions |
| 构建 | `npm run build`（tsc + vite） | 本地与 CI | Lint, build & frontend regressions |
| 前端状态回归 | `npm run test:frontend` | 本地与 CI | Lint, build & frontend regressions |
| 组件真实挂载回归 | `npm run test:components` | 本地与 CI | Lint, build & frontend regressions |
| 布局/交互真实浏览器 | `npm run test:ui-layout` | 本地与 CI（windows-latest） | UI layout & social hydration (Edge CDP) |
| 文档结构（索引/链接/锚点/行号禁令） | `npm run docs:check` | 本地与 CI | Lint, build & frontend regressions |
| Rust 格式 | `cargo fmt --all -- --check`（`src-tauri/`） | 本地与 CI | Check & test Rust (src-tauri) |
| Rust 全量 | `cargo clippy --all-targets -- -D warnings`、`cargo test --no-fail-fast`、`cargo check --all-targets` | CI（windows-latest）；本地可用隔离环境 | Check & test Rust (src-tauri) |

- `ci.yml` 对 `dev`/`main` push、面向 `main` 的 PR 及 `workflow_call` 触发；`release.yml` 在 tag 上调用同一提交的 `ci.yml`，不另起一套检查。
- 每个检查必须有承担者：本地不跑 Rust 全量时，由 CI 的 Rust job 承担；不能出现“本地不跑、CI 也不跑”。
- `#[ignore]` 的容量与真实服务端测试需在具体任务里单独约定读写边界后执行，不作为统一门禁；不得为变绿加 `#[ignore]` 或删测试。
- 本机结果只代表本机与当前源码；发布前的最终跨环境结论以固定 SHA 的 CI 为准。

- **CI 固定 Windows 跑 Rust**：这是纯 Windows 客户端，DPAPI 凭据加密与 SMTC 媒体控制等 Windows 专有功能只在 Windows 编译；在 Linux 上检查的是 `#[cfg(not(windows))]` 降级分支，真正的生产代码路径反而被跳过。
- **Rust 全量覆盖范围**：CI 执行一次 `cargo test --no-fail-fast`，覆盖全部未忽略的单元与集成测试（含自托管 mock 的集成测试及 ingestion/scheduler 自托管测试）；测试目标失败后仍执行其余目标，最终退出仍非零。

## 测试分层

- **前端状态回归**（`tools/frontend-tests/`）：纯逻辑/状态模块，经 loader 与源码共用模块实例；覆盖 store 投影、查询与分页、同步状态、设置校验、阅读位置等。
- **组件回归**（`tools/component-regression.mjs` + `tools/component-tests/`）：真实 React 挂载、真实 DOM 事件与焦点；分 `business` 与 `selfcheck` 两个子进程，任一失败整体非 0。`selfcheck` 用黑盒探针验证 runner 的看门狗、清理与错误账本本身。
- **布局/交互回归**（`tools/ui-layout-regression.mjs`）：真实 Edge headless + CDP，覆盖五布局、社交流式正文、画廊两视口、J/K 与 Tab/Space、模态焦点、窄窗与 reduced-motion；失败证据由 CI artifact 上传。
- **Rust 单元与集成**：`src-tauri/src/` 内的 `#[cfg(test)]` 模块与 `src-tauri/tests/` 的端到端/受控 HTTP 测试；运行 `cargo test --no-fail-fast` 覆盖全部未忽略目标。
- **单文件人工回归入口**：`tools/ai-lifecycle-regression.mjs`、`tools/ai-policy-regression.mjs`、`tools/gallery-media-regression.mjs` 未接入 npm scripts 与 CI，按需人工运行，不作为统一门禁。
- **容量与真实服务端**：`#[ignore]` 的 10k distinct-body、50k 正文容量与 live 服务端测试按任务单独运行；50k 状态初始分片是普通 `#[test]`，随 CI 全量执行。当前规模验证标准见 [performance.md](performance.md)。

## 证据纪律

- 证据必须带日期、构建方式、语料/输入、测量范围与限制；“测试通过”只说明当时命令的退出码，不外推为 product 可用或已发布。
- 失败运行保留日志与原始输出，不用后续成功覆盖；重跑要说明是环境原因还是代码变化，并保留两次结果。
- 同一 SHA 的 CI 结果才可绑定该提交；本地工作树有未提交改动时，CI 不代表未提交内容。
- 断言与阈值不得为通过而放宽；需要改变门禁标准时先有依据与确认，**并按上位规则过独立审计**，再同步本页与 [AGENTS.md](../AGENTS.md)。
- 可复现的最小集是五样：基线 SHA、命令、输入、结果、性质（运行验证 / 静态推导 / 未验证）。缺任一样只算线索，不算证据；细则见 [维护与文档纪律](maintenance.md)。
- 审计与证伪：新发现必须由另一个独立上下文再证伪一次；被推翻的结论要留档，结论摘要登记到 [审计记录](audits.md)。
- 临时探针放 `tmp/` 并在结束后删除；不能把一次性脚本或完整输出提交进主线。**审计证据不同**：原始输出要保留到该轮摘要复核完成，确认 [审计记录](audits.md) 已含复跑命令与最小结果快照后再清理。

## 常见故障

- **git/npm 代理不可达**：本机可能设置 `HTTP_PROXY=http://127.0.0.1:7897`，代理程序未运行时所有 HTTPS 失败。确认直连可用后，用单次 `git -c http.proxy= -c https.proxy= ...` 或清空当前进程的代理变量；不要擅自改全局 git 配置。
- **文件符号链接权限**：非提权 Windows 会话中，Rust 构建的子进程可能无法创建真实文件符号链接（`CreateSymbolicLinkW` errno 1314）。备份保留测试可用目录 junction 做等价 reparse-point 夹具；真实 symlink 路径的唯一测试 `writer_windows_symlink_alias_resolves_actual_file_identity` 带 `#[ignore]`，CI 也不运行 ignored 用例——本地与 CI 都未覆盖，只在具体任务约定后运行。
- **Node 版本导致 TAP 输出差异**：`.node-version` 是 22；Node 24 的 `node:test` 默认 reporter 输出 `ℹ fail 0`，而项目自检按 Node 22 的 `# pass/# fail` TAP 契约断言。临时用 `NODE_OPTIONS=--test-reporter=tap` 复现 Node 22 契约并在报告中注明；不要把改测试当作修复。
- **cargo 不在 PATH**：隔离环境激活后仍可能需要把 `C:\Users\<user>\.cargo\bin` 前置；命令报 `cargo not found` 时先查 PATH，不要重装工具链。
- **测试临时目录**：容量/Rust 测试需要可清理的 `TEMP` 与测试根；隔离环境构建产物统一进 `tmp/rust-env/target-xwin-msvc/`，不要把构建产物提交。
