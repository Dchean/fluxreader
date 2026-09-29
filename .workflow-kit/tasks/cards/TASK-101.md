<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-101 · Fever 协议对 FreshRSS 认证修复（api_key 必须走 POST form body）与设置页冗长文案精简

**状态**：done

**目标**：两项独立修复。① Rust（REQ-002）：用户报告 Fever 协议登 FreshRSS 报「Fever认证失败(api_key不正确)」而 GReader 正常——根因已由主控取证（tmp/task-101/fever-analysis.md，含 FreshRSS 官方源码与文档逐行证据）：FreshRSS p/api/fever.php:172 只读 $_POST['api_key']（form body），不读 query；本客户端把 api_key 拼在 URL query 上 POST 空 body，故恒 auth:0。公式 md5(username:api密码) 经官方文档示例逐字验证无误，不改。修法：src-tauri/src/fever.rs 的 call_probe 与 mark_items 把 api_key 从 query 挪到 POST form body（reqwest .form），query 上不再出现 api_key（顺带消除其进服务器访问日志的泄露面）；action/mark/as/id/with_ids/since_id 等参数留在 query（FreshRSS 全部走 $_REQUEST，Miniflux 实证 query 可用，不动）。endpoint_resolve.rs 的 fever_candidates 增补第三候选 {base}/p/api/fever.php（新 FreshRSS 布局，404 后才尝试，顺序在 /api/fever.php 之后）。测试：公式单测保留；新增 mock Fever 服务器集成测试模拟 FreshRSS 行为（api_key 只认 POST body、query 里带了一律当无效），含修前红证据（把 mock 对旧代码跑一遍留档）；断言请求 query 中不含 api_key（防回退）；第三候选顺延有测试。错误文案补充 FreshRSS API 密码指引（「Fever 认证失败（api_key 不正确；FreshRSS 请使用个人设置里的「API 密码」）」口径可微调）。② 前端（REQ-006）：SyncTab 被点名的长提示（「测试连接」只验证连通性（秒级）；「保存并同步」会立即在后台拉取订阅与文章状态。已读/收藏等变更约1秒内推送到服务端；断开连接会移除服务端拉取的订阅与文章。）精简为一两句短文案，语义全保留；对设置页同类冗长提示（多分号长句、>约60字的解释性段落）做一轮精简审计：GeneralTab/ReadingTab/AppearanceTab/AboutTab/FeedsTab/AiTab/ConfigSyncSection/CacheCleanupSection/SyncTab 逐个过，每处给前后对照；Fever 协议入口处补一行简短提示（FreshRSS 的 Fever 使用「API 密码」，在个人设置中设置/重置）；回归断言沿用 frontend-regression 既有 fix-* 风格补 t101-*。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-101-SYNC-COPY.md
**界面检查**：V1.sync-hint-concise, V2.fever-api-password-hint
**修改范围**：src/**, src-tauri/src/**, src-tauri/tests/**, tools/frontend-regression.mjs

## 验收标准

- ① Fever 修复：mock FreshRSS 行为（api_key 只认 POST body）的集成测试修后绿，且修前红证据（旧代码对同一 mock 失败）留档 .workflow-kit/tasks/evidence/TASK-101-*；请求 query 不含 api_key 有断言
- ② Miniflux 兼容不回退：action 在 query、api_key 在 form body 的请求形状被测试锁定（公式单测保留）
- ③ fever_candidates 第三候选 p/api/fever.php 有 404 顺延测试
- ④ SyncTab 精简文案上线且语义保留断言；设置页冗长文案逐条前后对照（progress 文档），断言总数 ≥501 且全过
- ⑤ V1/V2 两条 ui_checks 有 harness 真机取证（SyncTab 截图 + DOM 文案断言），证据入 tasks/evidence/
- ⑥ 门禁全绿且不回退：cargo test ≥239 passed/0 failed/9 ignored 不增（新增除外）、fmt/clippy 0、lint/build exit 0、frontend ≥501 全过
- ⑦ 独立审查（未参与实现的全新子代理）PASS findings=0 后由主控验收

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@a5598cb（v0.16.0 后）：cargo 239/0/9、frontend 501/501、fmt/clippy/lint/build 全绿。Fever+FRESHess 认证为协议兼容缺陷（api_key 传参位置），文案为 REQ-006 范畴的表述冗长，均为缺陷修复而非行为变更。
- 基线证据：.workflow-kit/tasks/evidence/TASK-101-fever-analysis.md
- 需求决定：DEC-task101-fever-copy-20260929
- 补充：Rust：mock FreshRSS 行为的 Fever 认证集成测试（api_key 只认 POST body）+ query 无 api_key 断言 + p/api 候选顺延测试；REQ-002 双向同步在 FreshRSS+Fever 下不可用，需成对证据；验证：cargo_test
- 补充：前端：SyncTab 精简文案断言 + Fever API 密码提示断言（t101-* 系列）；REQ-006 文案精简需防回退；验证：frontend
- 保留：既有 cargo/frontend 断言、fmt/clippy/lint/build；不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-29T14:03:52.635880Z
- 原截止时间：2026-09-29T18:03:52.635880Z
- 当前截止时间：2026-09-29T18:03:52.635880Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 29 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-29T14:31:51.037116Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-29T14:32:26.922300Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-29T14:50:03.449815Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-29T14:50:19.948087Z：阻塞已处置（review_failure）：已核对：审查报告真实（红证据独立复现、候选摘要吻合、V1/V2 真机取证 13/13）；两条 findings 均已在案，F1 修复属注释级改动；下一步：begin 重新实现
- 2026-09-29T14:50:31.682315Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-29T14:53:04.112853Z：编码结果已记录，差异范围已核对：src-tauri/src/endpoint_resolve.rs, src-tauri/src/fever.rs, src-tauri/tests/endpoint_autodetect_e2e.rs, src-tauri/tests/fever_freshrss_e2e.rs, src-tauri/tests/mock_greader.rs, src/components/settings/ConfigSyncSection.tsx, src/components/settings/SyncTab.tsx, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-29T14:58:56.811399Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-29T15:06:47.310053Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-101.json)

- [RUN-6b30abdf58d34f7f84ba0b64d8d298a0](../runs/RUN-6b30abdf58d34f7f84ba0b64d8d298a0.json)
- [RUN-f27f5c93291241e19ce97050d6486c33](../runs/RUN-f27f5c93291241e19ce97050d6486c33.json)
- [RUN-9669a13e2dc64e7fb3e89833f2eb3946](../runs/RUN-9669a13e2dc64e7fb3e89833f2eb3946.json)
- [RUN-db6aef84c1974520b40380c2cae190b9](../runs/RUN-db6aef84c1974520b40380c2cae190b9.json)
- [RUN-08cfeb9730274efb9e2709bbef5ac3ef](../runs/RUN-08cfeb9730274efb9e2709bbef5ac3ef.json)
- [RUN-227e86740cba4cceae1461854f4f0314](../runs/RUN-227e86740cba4cceae1461854f4f0314.json)
- [RUN-0a68a7ee5ec345e2ba93000983e9e4fa](../runs/RUN-0a68a7ee5ec345e2ba93000983e9e4fa.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
