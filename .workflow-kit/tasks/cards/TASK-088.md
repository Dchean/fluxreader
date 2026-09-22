<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-088 · refresh_dedup_e2e mock 服务器线程容错（收口 TASK-087）

**状态**：ready

**目标**：修一处**实测发现的**测试基建脆弱点：`src-tauri/tests/refresh_dedup_e2e.rs` 的 mock HTTP 服务器线程原有两个会**杀死整个服务器线程**的出口——(a) `let mut stream = stream.unwrap();` 在某个连接出错时 panic；(b) `if n == 0 { return; }` 在**正常的客户端断开**（响应头带 `Connection: close`）时 return 掉整个 accept 循环，而不是仅结束这一个连接。两者都会让该测试内后续所有请求连不上，症状伪装成「抓取失败」而非「测试基建故障」。改为：连接出错跳过该连接继续 accept（`let Ok(mut stream) = stream else { continue }`）、读端断开只 `break` 当前连接、写回失败仅忽略该连接。本卡是 TASK-087 的收口卡（该卡因任务记录多次合法修订与旧基线不收敛被取消），改动内容一致，只保留测试文件一项。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：src-tauri/tests/refresh_dedup_e2e.rs

## 验收标准

- ① mock 服务器不再因单个连接出错而 panic 掉服务器线程，也不再因客户端正常断开而退出整个 accept 循环；改后能持续服务后续连接
- ② 该测试的两个既有断言、语义与测试名一行不动
- ③ 四门禁全绿且不回退：cargo test ≥206 且 0 failed、9 ignored 不增；lint 0/0；build exit 0；frontend ≥322/322
- ④ 取证如实：本项为**潜在健壮性改进**，若无法构造出可复现的失败须如实记录（本次实测：把 `n == 0` 改回旧的 `return` 后该测试仍 2 passed，即当前用例观测不到差异），不得声称有「成对证据」

## 测试适用性

- 既有行为：保持
- 原始基线：PASS；2026-09-21 基线（TASK-086 验证 RUN-110241c0，提交 2d50c2b）：cargo test 206 passed / 0 failed / 9 ignored、lint 0 warnings / 0 errors、build exit 0、frontend 322/322。本任务 behavior=preserve：只把测试基建里两处「出错即杀死服务器线程」的出口改为容错，不触碰被测行为、feed 内容与任何断言。既有全部断言必须原样通过。
- 基线证据：.workflow-kit/tasks/runs/RUN-110241c05b914c77af21d9eed6dc012c.json
- 需求决定：沿用既有行为，无新增业务取舍
- 保留：全部既有 206 条 Rust 断言（含 refresh_dedup_e2e 的 2 条）与 322 条前端断言；本卡只改 mock 服务器的错误处理，不改 feed 内容、响应体或断言语义；既有断言即零行为变化的判据。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：None
- 原截止时间：None
- 当前截止时间：None
- 时钟：未开始
- 已用修复轮：0
- 阻塞：无
- 下一步：执行 start/next 获取可继续的动作

## 最近检查点


## 原始证据

[唯一状态记录](../items/TASK-088.json)


卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
