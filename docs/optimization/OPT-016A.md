# OPT-016A：CI执行责任、受控HTTP测试与开发依赖补丁

Status: verified-local
父卡OPT016独立子卡；回归网拆分/真实挂载由父卡后续完成，不因本卡关闭父卡。前置：无。

## 写集

- .github/workflows/ci.yml（增加v* tag触发，各Rust门禁独立执行且任一失败job仍红）
- src-tauri/tests/ingestion_e2e.rs
- src-tauri/tests/scheduler_e2e.rs
- src-tauri/tests/common/mod.rs（必要自托管临时HTTP helper，保持现有unique_db_path接口）
- package-lock.json（source-map-js从1.2.1更新至修复版本>=1.2.2的最小兼容补丁，不升级无关依赖）
- package.json仅不得已时需要显式合理override；优先无需新直接依赖
- .agents/notes/implemented/testing/2026-09-16-测试门禁与断言纪律.md（原位改事实：CI独立步骤/自托管套件）
- docs/roadmap.md对应已验收检查条目（主控在更新，执行者只在RESULT列修改建议不要直接写roadmap）
- tmp/optimization-20261008/OPT-016A/RESULT.md

## CI与测试要求

fmt失败不遮住clippy/test；使用明确if(success或failure)且前置checkout/toolchain正常时运行，不continue-on-error取消红灯。Windows Rust平台保持。tag v*有CIrun满足release门禁，不自动发布。当前六suite显式重复运行可暂保留，不删除默认cargo test。

ingestion_e2e/scheduler_e2e中只有依赖127.0.0.1固定外部HTTP服务的ignored测试改自托管：测试自己绑定127.0.0.1:0、生成固定feed内容/条件响应、结束时线程/连接关闭，不串到用户真实账户；去ignore后CI默认真实运行。live_e2e真实服务测试不动，不全局--include-ignored。不要保留固定8765或试图借用用户当前服务。失败/timeout有界，临时DB正常与panic路径清理（仅本测试创建）。

依赖修补必须跑npm ci、npm audit --json并保留返回故障/告警，不假绿。漏洞GHSA-68fv-2mgg-jv7q开发依赖，不把修补当生产RSS漏洞已消失。

## 验收

先证明ignored fixture被执行而非0匹配，新增/调整测试保持原业务断言；cargo test --test ingestion_e2e --test scheduler_e2e真实运行，clippy该targets。前端lint/build/test:frontend保持当前787+，lock diff仅预期包。CI语法与触发条件核查，真正tag触发不创建发布tag（静态验证+下一次正常CI验证）。独立review后由主控提交push。无Git写入/个人memory/代理，禁止修改其他卡文件。

## 验收证据

Heisenberg R2 PASS；主控两个HTTPsuite真实1+4通过/0ignored；cargo clippy --lib --bins --tests -- -D warnings退出0（无-A）；npm audit实时返回0 vulnerabilities。examples原生验收驱动仍WIP，完整all-targets由提交级CI覆盖（该未提交example不进入本提交）。CI工作流未实际打tag，正常devCI会验证语法与各门禁；父卡016回归分层/effect挂载仍未完成。
