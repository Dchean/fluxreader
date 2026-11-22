# FluxReader 顶层总控交接 · 2026-11-20

此包记录一次交接切面。会话在途进度以对应会话和最新原件为准。旧顶层 `01a1f222-1cb4-7371-b4e2-f1f1d86e6807` 在此边界冻结顶层派发与入库操作。S3d 在途任务由在任阶段总控和实现者继续。接任者先只读签收，收到明确移交后取得唯一顶层调度权，旧顶层停止调度。建新会话参数和实际 `turn_context` 均核对 `xingchiapi/gpt-6.1-sol` / `ultra`，末端继承同配置。

## 授权与来源

- 项目 `D:\soft\fluxreader`。先读 `AGENTS.md` 项目执行规则，再按需使用 `docs/contracts.md`、`docs/README.md` 和 `docs/maintenance.md` 路由；历史开局已按 `tmp/handoff-2026-11-17.md` 接手，里面的 beta、旧 HEAD 和待裁决是历史状态。看 `tmp/audit-route-2026-11-14/路线方案.md` 的 §2–§5、§7 和末尾最新记录。
- D3–D9 以 `tmp/decisions-2026-11-20.md` 为准，SHA256 `39998834b385ff284b1dc31469792b6995459d7e7bdf5ee0669c971868d1c06c`。尤其 D6 无默认模型，Base URL、Key、模型由用户填写，支持协议不假定官方服务，不改现有已保存配置；S4a 实现并同步权威说明。该裁决不覆盖交接内列出的另外几项文档事实差异。
- 始终三层。顶层新开阶段总控对话，阶段总控用子代理调查、实施、修复和自测，独审全新只读。总控只核原件、登记和收口。每批一个实现者，同一文件单写者，审计问题交原实施与原审复审。顶层和阶段总控都根据上下文完整性、余量择安全点新开交接，保持唯一调度权。
- 用户已授权合格阶段分批提交并推送 `origin/dev`。不跑 Actions。提交消息含 `[skip ci]`，只按受审清单显式暂存，`git -c push.followTags=false push origin HEAD:refs/heads/dev`，以 `git ls-remote --exit-code origin refs/heads/dev` 核远端。合并、标签、发布需另权。不要为通过删有效测试、加 ignore 或放宽断言。

## 接手切面与所有权

- `dev` HEAD 和本地 `origin/dev` 为 `d40c4f11130e72ada23b1ac99ea040125a727f6d`（已推 S3c，见 `tmp/s3c-2026-11-20/top-push-verification.json`）。暂存为空。当前七个工作树改动按两批划界，S3d 进展可在此快照之后增加正式登记。
- **S3d 在任** `01a1f955-5f20-7d11-b1c9-f61d11c2d894`。旧任 `01a1f7ef-6e11-7d23-b310-503ab98908d5` 已停。授权和交接核对见 `tmp/s3d-2026-11-20/stage-successor-receipt.md`。唯一实现者 Peirce `01a1f7f4-68a4-7ba3-b5a2-04efa08681e9`，按新版证据继续，无需重派。S3d 阶段握有 `docs/audits.md`、`docs/README.md` 的本批写权，记完并冻结后再安排规则批登记；顶层只在此后接手这两个共享文件。S3d 不改 `tools/frontend-tests/ui-contracts.mjs`、`src/lib/api.ts` 或 `src/App.tsx`。
- **规则批正文**已冻结。阶段 `01a1f953-2aea-7d32-9e63-96b18b2033fe` 已完成。唯一改动 `AGENTS.md`，SHA256 `020353fd1651c42cb8966c20a58917b78adabcb42ee0bc0fe4133aa5a73333a1`。`tmp/orchestration-clarify-2026-11-20/delivery-evidence.json` 含基线、命令、输入、结果和性质；`implementation-original.md` 已有完整正文，`audit-original.md` SHA256 `ad7bb334117566d941159b03e02d9836fe8da48021309552ed435577c49357b0`。`npm.cmd run docs:check`（12 篇）和 `git diff --check -- AGENTS.md` 均通过，文本独审通过、三档问题均 0。尚待顶层单写登记、检查、单独提交推送。证据仅适用规则文本；实际 AI 行为归 S4a。
- **S3d 六个候选文件 SHA256**（22:35 与隔离候选一致）：
  - `docs/README.md` `829cbffe3755829f526e77e5c8f4fcf0cf7a2e28b98e94726550adc102791c87`
  - `docs/frontend.md` `7b2c48b2075b2f6a87eec315716dc872c69f1a8dfc806bb458b334ff990ebe98`
  - `src/components/Timeline.tsx` `cd9dae6cf5f238c9fc926ffedadc80b6723681b15bdec8ed607b8469cb99ddf0`
  - `src/styles/base.css` `e825fc3bccce24f07bc7dfc81dff7e4522f6e37b2664e1c168d9ba9be76e959a`
  - `tools/component-tests/ai-cache-signature.test.mjs` `a6aad45ddeaa972a622c63bb666a4da80e771b3ce3fdf88f336a922f2ce4bd20`
  - `tools/component-tests/hydrate.test.mjs` `2a95a9214c3bd022e62512be829139c5c2b60c6d78a9ed688f895984e56467e9`

## 在途验证与下一步

- S3d 范围为卡片翻译状态行复用、通知复开译文时验证 B1、hydrate 样式重复收口、N5 既有媒体 DOM 回归、N22 注释。`tmp/s3d-2026-11-20/implementation/evidence/` 有旧源码配新增测试 16/17（Notif B1 真红），候选定向组件 36/36、前端 494/494、tsc exit 0。正式门禁原件位于同目录 `formal-gates-20261120/`。`02-ui-layout.result.json` 在隔离候选以 `npm run test:ui-layout` 跑正式 Edge，22:34:24 退出 0，日志 SHA256 `c3f7c7c75acbc268a69bbf9476ded742a6fa0c30f218ed7672745561aaefa0df`。`01-ui-layout` 为环境参数预检失败，原样保留。隔离副本 lint 的无文件记录与根目录 lint 补跑分别保留。其余正式门禁由 Peirce 串行结束，实时看原件和阶段报告，不另起 Edge、Cargo 或组件进程抢资源。
- `tmp/s3d-2026-11-20/run-001/` 和更早被覆盖的浏览器探针只作失败记录，不充验收。后继阶段负责处理正式门禁、只读独审、必要修复复审、登记和冻结清单，一次核对即可。迁移核包审计只证交接，不能代替 S3d 产品审计。正式门禁与独审齐备后顶层按清单单独提交 S3d，再登记、检查与单独提交规则批。未跑的 Tauri/WebView2、读屏、真实 AI、Rust 全量等按实际范围如实登记，不用既有 CI 代称当前 SHA 的 CI。
- 紧接 S3e（D4/D9），再按路线推进 S4a、S4b、S4c、S4d、S5。`src/lib/api.ts` 先 S3e 后 S4c，`ui-contracts.mjs` 和 `harness.mjs` 先 S4c 后 S4d，路线 §3 其余写者顺序照守。S4a、S4b 的 Rust 改动由阶段安排适用本地门禁。无发布授权，S6 仅保留路线。
- Node 24，Edge `TEMP`/`TMP` 留系统外部目录，`UI_LAYOUT_CHECK_GROUP` 不用时 unset。Cargo 用 `tmp/rust-env`、PATH 前置 `C:\Users\Wade\.cargo\bin`、`TEMP`/`TMP` 指向 `tmp/test-temp`，不与组件全量并发。只由末端启后台验证，长日志留 `tmp/`。单次冷启动崩先核状态并唤醒原代理；同类失败持续有证据再换法。

独立核包只检查授权、原件、版本、所有权、执行连续性，不重跑门禁。接任最短动作是读此包、裁决和版本证据，用 `git branch --show-current`、`git rev-parse HEAD refs/remotes/origin/dev`、`git status --porcelain` 核当前工作树，检查两个阶段状态，回报只读签收。正式移交后继续原路线并登记接任，原顶层不再派发。
