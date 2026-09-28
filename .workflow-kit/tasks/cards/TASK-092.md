<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-092 · 封面图片位统一代理与失败回退 + 失效上报接线（REQ-106 之③，前端）

**状态**：verified

**目标**：修用户报告「文章封面部分获取不到」的前端成因（审计 round-4 §1-C/§1-D；基线 .workflow-kit/tasks/evidence/TASK-092-baseline-images.md；UI 契约 .workflow-kit/docs/UI-CONTRACT-REQ-106-IMAGES.md）。现状：只有画廊卡片走 proxyImageUrl，文章卡（Timeline.tsx:374）、播客卡（:685）、迷你播放条（PlayerBar.tsx:216）、全屏播放器（:332）、灯箱（Overlays.tsx:338）全部直连 + no-referrer 且无 onError ⇒ 白名单式防盗链图床（实测 cdnfile.sspai.com 无 Referer 403×12/12、img*.doubanio.com 418×20/20）必然破图，失效封面（TASK-091 已提供 report_broken_cover 后端命令）也没有上报出口。本卡：① 五处图片位统一复用 lib/imageProxy.ts 的既有判定与 api.fetchImage（抽一个共享的封面图片组件或 hook，不新造第二套判定）；② 失败（代理失败/直连失败/字节非图片）时显示占位，同会话不重复请求；③ 失败时经 api 调用 report_broken_cover(article_id, url) 上报，同条目同 URL 幂等只上报一次；④ 补前端回归断言（修前失败、修后通过）与真机 UI 证据（截图 + 交互报告）。

**依赖**：TASK-091
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-REQ-106-IMAGES.md
**界面检查**：A1.proxy-by-policy, A2.no-overreach, A3.gallery-unchanged, A4.no-layout-shift, B5.fallback-placeholder, B6.theme-light-dark, B7.no-retry-storm, C8.report-idempotent, C9.shared-cover-handling, D10.empty-cover, D11.non-image-bytes, D12.lightbox-failure
**修改范围**：src/**, tools/frontend-regression.mjs

## 验收标准

- ① 五处图片位（文章卡/播客卡/迷你播放条/全屏播放器/灯箱）按 lib/imageProxy.ts 的既有判定走代理或直连；判定规则只在 imageProxy.ts 一处；画廊卡片行为不变
- ② 失败回退：代理失败、直连 onError、字节非图片三种情形都显示占位（无破图图标、无布局跳变），同会话同 URL 不重复请求
- ③ 上报：失败时调用 report_broken_cover(article_id, url)（参数名与 Tauri 命令契约一致），同条目同 URL 只上报一次；无 cover 的条目不渲染 img、不代理、不上报
- ④ 前端回归新增断言修前失败、修后通过（五处走代理、失败回退、上报幂等、空 cover 不上报）；既有 348 条断言无一削弱
- ⑤ 真机 UI 证据：用 tmp/audit-r3/harness 的 Chrome CDP + 忠实假后端核对 ui_checks 全部条目，保存浅色/深色截图与交互报告（DOM src 形态、naturalWidth、占位节点、IPC 计数）到 .workflow-kit/tasks/evidence/TASK-092-*
- ⑥ 门禁全绿且不回退：cargo test ≥213 passed / 0 failed / 9 ignored 不增；cargo fmt --check、cargo clippy --all-targets -- -D warnings、lint、build 全部 exit 0；frontend ≥348 且全部通过

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；修前门禁全绿（cargo 213/0/9、frontend 348/348、lint 0/0、build/fmt/clippy exit 0，2026-09-24 主控复跑），但既有前端回归对五处内容图片位的取图方式与失败表现零覆盖：没有断言要求走代理、没有 onError、没有上报。已知限制（本卡不改）：miniflux 源不补全、first_image 启发式。
- 基线证据：.workflow-kit/tasks/evidence/TASK-092-baseline-images.md
- 需求决定：DEC-next-batch-covers-reachability-20260923
- 保留：前端回归既有 348 项（含画廊卡片代理相关断言）；本卡改变的是另外五处图片位的取图方式，画廊与其余行为保持；验证：frontend
- 补充：tools/frontend-regression.mjs 新增：五处图片位走代理判定 / 失败占位 / 同 URL 不重复请求 / report_broken_cover 幂等上报 / 空 cover 不渲染不上报 / 非图片字节走回退；新行为需要修前失败、修后通过的成对断言；验证：frontend
- 保留：src-tauri 既有 213 项测试、fmt、clippy、lint、build；本卡不改 Rust，作为不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build

## 执行与恢复

- 首次开始：2026-09-24T05:35:16.176680Z
- 原截止时间：2026-09-24T09:35:16.176680Z
- 当前截止时间：2026-09-28T09:06:50.470158Z
- 时钟：按墙钟计：额度 480 分钟，写入阶段已用约 5740 分钟
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-28T05:49:02.437434Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-28T05:53:25.894229Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T05:53:41.051372Z：Worker result must match the complete worker-result contract；下一步：按报错列出的漏报/多报文件修正 worker-result，再 unblock 后 begin；不要新建任务或重置预算
- 2026-09-28T05:54:02.896174Z：阻塞已处置（protocol）：重建后先本地校验键集再 finish。恢复安全。；下一步：begin 重新实现
- 2026-09-28T05:54:09.123195Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-28T05:54:16.915413Z：编码结果已记录，差异范围已核对：无文件变化；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-28T05:54:48.793890Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-28T06:20:49.536530Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-092.json)

- [RUN-53130f3d2605478c958b31e4e68ea4a2](../runs/RUN-53130f3d2605478c958b31e4e68ea4a2.json)
- [RUN-fc5dbd3a3fc4445284b1e5fc66c9794f](../runs/RUN-fc5dbd3a3fc4445284b1e5fc66c9794f.json)
- [RUN-c269afa7f3fd4fa7a0d178992edb577a](../runs/RUN-c269afa7f3fd4fa7a0d178992edb577a.json)
- [RUN-a084b6cd0fd4489fb3c724e842c0bcb7](../runs/RUN-a084b6cd0fd4489fb3c724e842c0bcb7.json)
- [RUN-4d1c8e8b311740e99016319aff092af2](../runs/RUN-4d1c8e8b311740e99016319aff092af2.json)
- [RUN-4e880e9ee0e94faba25a4ae8aecd012c](../runs/RUN-4e880e9ee0e94faba25a4ae8aecd012c.json)
- [RUN-086b2e6bf6444b4fa60146402f6751ee](../runs/RUN-086b2e6bf6444b4fa60146402f6751ee.json)
- [RUN-3ce255de7a9e4a94b590cadf6e1a7bce](../runs/RUN-3ce255de7a9e4a94b590cadf6e1a7bce.json)
- [RUN-3c369789cecf443892e3db18ff8907b5](../runs/RUN-3c369789cecf443892e3db18ff8907b5.json)
- [RUN-248bfad9c0864267b6b98388f2cccb84](../runs/RUN-248bfad9c0864267b6b98388f2cccb84.json)
- [RUN-56dc087f1efd48e5bf6a82f13f30c403](../runs/RUN-56dc087f1efd48e5bf6a82f13f30c403.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
