# 测试与构建基线

记录日期：2026-09-13（Asia/Shanghai）。执行任务：TASK-002。状态：初始测量已完成，等待审阅；这不表示整个 M1 的测试保护工作已经完成。

用户已明确批准本次依赖准备、指定检查和桌面构建。完整记录见 [本次报告](../tasks/runs/TASK-002-20260913T041438Z.json)，当前授权见 [PROJECT](../tasks/PROJECT.json)。TEST-PLAN 保留审批前制定的测量方案，命令没有因运行结果而修改。

## 结果

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| npm ci | PASS | 安装 37 个包；未修改依赖清单或锁文件 |
| npm run lint | PASS，6 条警告 | 当前规则允许警告，不等于零问题 |
| npm run build | PASS | tsc -b 与 Vite 构建通过；首轮沙箱 EPERM，获准重跑后通过 |
| npm run test:frontend | PASS | Node 中的 8/8 项状态逻辑检查；不是桌面 UI E2E |
| cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check | FAIL | 38 个 Rust 文件、532 处格式差异；本项是新增的测量检查，当前 CI 未配置 |
| cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings | PASS | 在 Windows 上完成现有配置的静态/编译检查 |
| cargo test --manifest-path src-tauri/Cargo.toml --locked | PASS | 81 项通过、23 项 ignored |
| 指定 6 个 target 的 --ignored 测试 | PASS | 补充执行 14 项；dual_client_e2e 在此命令中为 0 项，其 4 项已由普通测试运行 |
| npm run tauri build | PASS | 生成 Windows x64 MSI 与 NSIS 安装包；未安装、未启动、未发布 |

完整参数和每次运行的退出码、起止时间、耗时、日志位置保存在报告中。计划 9 项检查中 8 项返回成功，格式检查返回失败；没有删除测试、降低规则或修业务代码让结果变绿。

Rust 总共执行 95 项并通过；8 项前端断言单独计数，不混成同一种测试数量。

## 未运行与覆盖边界

- 7 项真实 Miniflux 测试没有运行；本次不使用真实 FreshRSS/Miniflux 账号，不能据此保证服务端兼容性。
- 2 项依赖单独启动的本地 feed 服务的 ignored 测试没有运行；后续需要 fixture 任务。
- 普通测试跳过的 23 项中，14 项已由第二条 mock 命令补跑，最终仍未执行的是上述 9 项。
- 没有安装/启动应用，没有完整 Windows 桌面 UI E2E、人工交互验收、真实 AI 内容质量评估或性能验收。
- 当前测试证明其实际覆盖的路径。历史 Bug 是否被有效防住，仍需核对失败入口与修复前后的证据。
- OPT-004 的新要求尚未实现。现有配置同步测试通过，不代表它已经满足排除敏感凭据的新字段契约。

## 源码与输入快照

- 源码提交：6550a223d3e5fb4cae66b07af31fc35c5f1fd6a9。
- 项目版本：0.12.0。
- 2026-09-12 首轮盘点时工作树干净；本次输入还包括此前未提交的治理文档和用户新增的 AGENTS 规则。
- 已保存 130 个输入文件、约 1.96 MB 的副本和 SHA-256 清单：本地目录 .cache/baseline/TASK-002-20260913T041438Z/input/，清单为 .cache/baseline/TASK-002-20260913T041438Z/input-manifest.json。
- 输入清单 SHA-256：4b301a7988eca7acc1ca2ae4729d540da97c80239cb3c974c9ab3c91d72e26c8。
- 本次测量未修改产品源码、测试、CI、依赖清单或锁文件；未创建 Git 提交、标签，未合并或发布。

| 锁文件 | SHA-256 |
| --- | --- |
| package-lock.json | 2902f509f0c891d277ee1a3d5b0962951800619bc310d743f5c9ca744ebe15e3 |
| src-tauri/Cargo.lock | c93c394d32c1ca23e625adeb5160cb0028dfb2e45c3d4e3e4fac70bb79acef60 |

## 环境与执行条件

| 项目 | 本次实际环境 |
| --- | --- |
| 操作系统 | Windows build 26200，X64 |
| Node / npm | v24.19.0 / 11.17.0 |
| Rust / Cargo | 1.98.1 / 1.98.1，stable-x86_64-pc-windows-msvc |
| Rust 组件 | clippy、rustfmt 等已安装 |
| C++ 工具 | Visual Studio 2022 BuildTools 可用 |
| 桌面打包工具 | Tauri 构建流程取得 WiX 3.14.1、NSIS 3.11 及相关插件 |

CI 前端配置为 Ubuntu + Node 22，Rust 为 Windows + stable；本次是本机 Windows + Node 24 的测量，不是实际 CI 运行，也未验证声明的最低 Rust 1.80。

原始日志还包含 Node 的 experimental-loader 提示、MSVC 生成导入库信息触发的 linker_messages 警告，以及 Tauri 对 .app 结尾标识的 macOS 提示。这些没有导致本次 Windows 检查失败；本轮未改加载器、应用标识或工具配置。

每条命令的 TEMP/TMP 仅在其进程内指向本次运行专用目录 .cache/baseline/TASK-002-20260913T041438Z/tmp。测试使用本地 mock 和临时数据库，不访问真实应用数据库。没有修改全局工具链设置或启动无人值守控制器。

首次输入快照的 Git 子进程和首次 Vite 构建遇沙箱 EPERM，取得所需环境权限后成功。失败与重跑均保留。桌面构建沿用已确认需要的运行权限，没有把权限限制当成源码缺陷修复。

## 产物

| 产物 | 大小（字节） | SHA-256 |
| --- | --- | --- |
| [MSI](../src-tauri/target/release/bundle/msi/FluxReader_0.12.0_x64_en-US.msi) | 8359936 | 5e18b3ae096406a35ec1027a94cc99470def17b1d2e20aef3740de4b63f68958 |
| [NSIS](../src-tauri/target/release/bundle/nsis/FluxReader_0.12.0_x64-setup.exe) | 5801156 | 11ef552228ecb91eee8ec858f316ae7da329395c1ecdfb2d596f830d15da2ab2 |

最终 release/app.exe 为 23100416 字节，SHA-256 为 651cd7c0d4c6401240f698f15d5c9434c721c4f499f9bf45595dd111b84bf4f8。产物存在与打包成功不等于安装或运行验收通过。

原始日志位于 .cache/baseline/TASK-002-20260913T041438Z/logs/。输入副本、原始日志和安装包受 .gitignore 排除，不随 Git 自动移交；换环境时需要携带产物或重新测量。仓库中的 JSON 报告保留索引和关键结果。

## 后续处理

详见 [ISSUES](ISSUES.md) 与报告的 followup_proposals：

1. 将已经可运行的前端回归接入 CI，并核对既定 CI 环境。
2. 单独处理格式差异，逐条判断 lint 告警；不要与业务重构混在一次变更中。
3. 为 OPT-004 的新数据边界定义字段清单与有效测试，再提出实现任务。
4. 补齐真实桌面与服务兼容性验证，以及项目内决策技能依赖。

以上是测量时提出的后续建议；测量阶段没有修改代码、测试或 CI。后续已按用户交接要求进入风险受控的管理 agent / Claude 模式，具体授权与任务见 HANDOFF 和 EXECUTION-POLICY。M0 的历史报告保留原始 NOT_RUN 状态；当前测量结果以本文件及本次报告为准。
