<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-102 · 设置页协议控件对齐修复与冗余文案清理（含删除性精简）

**状态**：done

**目标**：owner 反馈 TASK-101 后两类问题：①同步协议卡的下拉控件左移错位——根因已由主控定位：.setting-card 为 space-between 双列布局，TASK-101 把 48 字 API 密码提示与 FluxDropdown 包进同一裸 div，提示文本撑宽包裹层使下拉离开控件列右缘（src/components/settings/SyncTab.tsx:199-216）。修法：API 密码提示移入卡片 desc 位并 ≤32 字（替换低价值的「两种协议共用 Miniflux 集成凭据，切换不丢数据」花絮句），下拉恢复为 SettingCard 直接子元素。②文案仍太长：动作区底部常驻提示（74 字两句）精简为单行 ≤40 字、只保留两按钮语义对齐（如「测试连接」仅验证登录；「保存并同步」确认后拉取订阅与文章）；「断开会移除同步拉取的内容」从常驻文案删除——断开确认框（SyncTab.tsx:304）已完整承载该破坏性语义；「已读/收藏约 1 秒回传」删除（解释性冗余）。③全设置页冗余文案清理审计（GeneralTab/ReadingTab/AppearanceTab/AboutTab/FeedsTab/AiTab/ConfigSyncSection/CacheCleanupSection/SyncTab）：判据升级——复述标题或陈述与操作无关花絮的 desc 直接删除；保留的解释 ≤48 字；纯 restating 控件行为的提示不写；每处「删除/压缩/保留」决定入 progress 对照表。回归断言：frontend-regression 新增 t102-* 源级扫描断言（X1 结构断言 + X2 文案断言 + X3 全页扫描守卫），t101-* 中与新文案冲突的断言同步更新。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：.workflow-kit/docs/UI-CONTRACT-TASK-102-SETTING-COPY.md
**界面检查**：X1.protocol-control-aligned, X2.sync-copy-concise, X3.setting-copy-scan
**修改范围**：src/**, tools/frontend-regression.mjs

## 验收标准

- ① 协议下拉与其他设置卡控件列对齐（X1：结构断言 FluxDropdown 为直接子元素 + 真机截图）
- ② 动作区提示单行 ≤40 字、两按钮语义对齐；断开删除语义仅由确认框承载；API 密码提示 ≤32 字在 desc 位（X2）
- ③ X3 全页扫描守卫上线：设置页 desc/hint 无 >48 字、无复述性冗余；删除性精简逐条对照入 tmp/task-102/frontend-progress.md
- ④ 门禁全绿不回退：frontend ≥516 全过（t102-* 新增、t101-* 冲突项更新）、lint/build exit 0、cargo 不回退（无 Rust 改动）
- ⑤ 独立审查（全新子代理）PASS findings=0，V/X 真机取证（SyncTab 浅/深截图 + DOM 断言）入 tasks/evidence/

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@8b9efe4（v0.16.1 发布后）：frontend 516/516、cargo 244/0/9、lint/build/fmt/clippy 全绿。本卡为 TASK-101 文案改动的 owner 返工（对齐控件 + 更激进的删除性精简），无行为语义变化，纯 UI 布局与文案。
- 基线证据：.workflow-kit/tasks/evidence/RUN-c9ad8ebf914647dcac48fa9823a4b93e-frontend.stdout.txt
- 需求决定：DEC-task102-copy-trim-20260930
- 补充：t102-* 源级扫描断言（X1 对齐结构 + X2 文案 + X3 全页长度/冗余扫描）；owner 返工项需防回退；验证：frontend
- 适配：t101-* 中与新文案冲突的断言；文案再精简后原锚点文本变化；验证：frontend
- 保留：既有 cargo/frontend 断言、fmt/clippy/lint/build；不回退证据；验证：cargo_test, cargo_fmt, cargo_clippy, lint, build, frontend

## 执行与恢复

- 首次开始：2026-09-30T02:31:03.031828Z
- 原截止时间：2026-09-30T06:31:03.031828Z
- 当前截止时间：2026-09-30T06:31:03.031828Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 48 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-30T02:31:03.197213Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-09-30T03:19:52.168140Z：编码结果已记录，差异范围已核对：src/components/settings/AboutTab.tsx, src/components/settings/CacheCleanupSection.tsx, src/components/settings/ConfigSyncSection.tsx, src/components/settings/GeneralTab.tsx, src/components/settings/ReadingTab.tsx, src/components/settings/SyncTab.tsx, tools/frontend-regression.mjs；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-09-30T03:24:57.475482Z：Verification timed out/cancelled; original logs and deadline retained；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-30T03:30:33.105652Z：Verification timed out/cancelled; original logs and deadline retained；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-30T03:36:28.346561Z：Verification timed out/cancelled; original logs and deadline retained；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-30T03:41:59.099198Z：Verification timed out/cancelled; original logs and deadline retained；下一步：先核对已有文件及原始日志，再处理 interrupted；不要新建任务或重置预算
- 2026-09-30T03:45:40.809179Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-30T04:02:59.940727Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-102.json)

- [RUN-e7dd6826cc6b4bb39b2a6c18c611ec3d](../runs/RUN-e7dd6826cc6b4bb39b2a6c18c611ec3d.json)
- [RUN-c3056390ff774b85acd7f02132c5e616](../runs/RUN-c3056390ff774b85acd7f02132c5e616.json)
- [RUN-e3792951156e42cca86c59125b5606ce](../runs/RUN-e3792951156e42cca86c59125b5606ce.json)
- [RUN-c3f1102bc67246b6a42a58afe6e76a55](../runs/RUN-c3f1102bc67246b6a42a58afe6e76a55.json)
- [RUN-b475337863904cf18a550539f7a04127](../runs/RUN-b475337863904cf18a550539f7a04127.json)
- [RUN-e39d2c3cfbd44bd6ad10be139a699d05](../runs/RUN-e39d2c3cfbd44bd6ad10be139a699d05.json)
- [RUN-d8b81cd9331a411d9bd516e0eedcfe42](../runs/RUN-d8b81cd9331a411d9bd516e0eedcfe42.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
