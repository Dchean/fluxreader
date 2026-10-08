<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-128 · 性能测量纠正与媒体/并发负载补测工具——先保证测量正确（审计 P2-7）

**状态**：done

**目标**：修复第三方审计 P2-7（tmp/audit-20261007/REVIEW.md 第 7 节）对四阶段性能测量可信度提出的六点质疑，并补上媒体与同步并发负载的测量能力。具体：(1) 切换完成判据 document.querySelectorAll('[data-card-index]').length>0 会在旧卡片尚未换掉时成立，63-78ms 主要是 50ms 轮询与自动化开销——改为「新结果识别 + 渲染稳定」双重判据：点击前记录首屏卡片 id 基线，点击后等待 (a) 卡片 id 集合变化（新查询结果已落地）且 (b) 连续 3 帧 rAF 无 DOM 变更（渲染稳定），报告 firstResultMs 与 stableMs 两个值以及 cardsBefore/cardsAfter。(2) 搜索计时在打开面板并等 500ms 后才开始、「包含打开浮层」说法不实、完成判据搜整个 document.body 可能命中背景列表已有文本、超时仍输出普通耗时——改为分段计时：openMs（Ctrl+K → 搜索输入框出现）与 resultMs（键入 → 结果出现）；结果归属必须断言：命中节点位于搜索浮层内、其文本包含本次唯一 token、且结果集与基线不同；超时写 timedOut:true，不输出误导性毫秒数。(3) 50k 数据中社交/画廊/播客/通知四布局卡片数为 0、社交数据为纯文本无真实图片/音频/解码压力——phase4_seed.py 增加 --media（为种子行写入 image_url/audio_url/duration_sec/enclosure_url，指向稳定可用的真实媒体 URL）与 --layouts（把布局分配到全部五种，保证社交/画廊/播客/通知都有卡），--info 输出按布局的可见卡数。(4) JS 堆不等于 Rust + WebView2 + 图片解码的进程内存；长会话缓存是否有界未被测——测量电池同时采集进程 RSS（应用进程 + WebView2 进程组，Node 侧读进程内存并明确标注三者口径不同），并新增长会话测量：8 次视图往返 + 反复深页滚动，报告首末 JS 堆/RSS 增量而非单点值。(5) 程序化滚动只测了 3 秒顶部——新增深页滚动测量（滚动到列表中段/尾部再采样帧间隔），并在报告中写明测量条件。(6) 独立 SQL 查询没有制造后台同步持锁竞争——新增锁等待代理测量：对 __TAURI_INTERNALS__.invoke('sync_queue_stats') 采样 N 次，分别在空闲态与同步在飞态（触发手动同步后立即采样）报告 p50/p95/p99，并明确标注这是「IPC 往返 + DB 单连接锁等待」的代理指标而非直接锁计时。全部修正后的报告输出必须自带测量条件（加载实体数、媒体是否启用、网络状态、同步是否在飞），结论只能限定在受测条件内。另产出面向 owner 的实机测量清单 tools/phase4_checklist.md（启动参数、注入、跑电池、还原、每项数字的含义与判定线、异常处置）。

**依赖**：无
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：tools/t059_cdp.mjs, tools/phase4_measure.mjs, tools/phase4_seed.py, tools/phase4_checklist.md

## 验收标准

- ① 切换延迟判据改为「新结果识别（卡片 id 集合变化）+ 连续 3 帧渲染稳定」，输出 firstResultMs/stableMs/cardsBefore/cardsAfter，不再用「存在任意卡片」作完成条件
- ② 搜索分段计时 openMs/resultMs，结果归属断言（浮层内、含唯一 token、与基线不同），超时 timedOut:true 且不输出普通耗时数字
- ③ phase4_seed.py 支持 --media（真实图片/音频/时长/enclosure URL）与 --layouts（五布局均有数据），--info 输出按布局卡数；备份/还原纪律不变
- ④ 内存测量同时采集 app 进程与 WebView2 进程组 RSS，与 JS 堆分列并标注口径差异；新增长会话测量（8 次视图往返 + 反复深页滚动）报告首末增量
- ⑤ 新增深页滚动帧测量（列表中部/尾部），不再只测顶部 3 秒
- ⑥ 新增锁等待代理测量：sync_queue_stats IPC 往返 p50/p95/p99，空闲态与同步在飞态各一组，标注为代理指标
- ⑦ 报告输出携带测量条件（加载实体数/媒体开关/网络/同步在飞），结论限定受测条件（工具输出中给出同一口径的结论行）
- ⑧ tools/phase4_checklist.md 提供 owner 可执行的实机清单（启动参数、注入、跑电池、还原、数字含义与判定线、异常处置）
- ⑨ 本地烟雾验证：用 Vite dev + 无头 Chrome（CDP）跑通修正后的测量电池（mock 模式数据量小，验证的是工具不崩、各测量段都产出结构化字段、超时/空数据分支如实标记）
- ⑩ 门禁全绿：lint/build/frontend/cargo_fmt 不回退（本卡零前端与零 Rust 业务代码改动）
- ⑪ 独立审查（全新子代理）PASS findings=0

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；dev@0cc84e6（第五阶段 117/118/119/121/122/123/124/127 全部验收后）：frontend 759/759、CI 全绿。本卡只改测量工具（Node/Python），不改业务行为；审计 P2-7 要求先纠正测量再决定优化。
- 基线证据：.workflow-kit/docs/DEC-gate-adjust-20261007.md
- 需求决定：DEC-gate-adjust-20261007
- 补充：测量工具自校验通道：本地 Vite dev + 无头 Chrome（CDP）烟雾跑通修正后的电池，产出结构化字段与如实的分支标记；工具修正本身需要可执行的验证证据（审计 P2-7.1/7.2/7.6 指出上一版工具与 CDP 契约不符且判据不实——工具正确性必须被验证而不是被声明）；验证：lint, build, frontend
- 保留：既有 frontend 759/759、lint、build、cargo_fmt 门禁；本卡零前端/零 Rust 业务改动，全部既有断言保持；验证：cargo_fmt, lint, build, frontend

## 执行与恢复

- 首次开始：2026-10-08T01:46:49.056155Z
- 原截止时间：2026-10-08T05:46:49.056155Z
- 当前截止时间：2026-10-08T05:46:49.056155Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 66 分钟
- 已用修复轮：0
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-10-08T02:33:52.477750Z：编码结果已记录，差异范围已核对：tools/phase4_checklist.md, tools/phase4_measure.mjs, tools/phase4_seed.py；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-08T02:34:33.182188Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T02:45:13.168902Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-10-08T02:45:42.453113Z：阻塞已处置（review_failure）：R1 修复范围：F1 搜索正例限定文章分组+overlayText 打开后读取+键入前 token 硬断言；F2 每段自带 conditions 快照并据此生成结论行；F3 清单与 seed 脚本加『还原前退出应用』保护；F4 修正『只读』声明并单列真实同步警告；F5 统一清单文件名与长会话判定线；F6 崩溃布局堆值标失效。；下一步：begin 重新实现
- 2026-10-08T02:46:35.032394Z：开始执行，保留原任务身份和截止时间；沿用本任务先前的范围基线，changed_files 为本任务累计改动；下一步：完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify
- 2026-10-08T03:07:54.095620Z：编码结果已记录，差异范围已核对：tools/phase4_checklist.md, tools/phase4_measure.mjs, tools/phase4_seed.py；下一步：运行 verify；代码完成尚未等于验收通过
- 2026-10-08T03:08:36.579407Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-10-08T03:19:13.651168Z：当前候选的测试与审查通过（independent）；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-128.json)

- [RUN-e21dfa27982d4d8d9ebf349ee67d9939](../runs/RUN-e21dfa27982d4d8d9ebf349ee67d9939.json)
- [RUN-0353573413df42efa91ac2d7c4235031](../runs/RUN-0353573413df42efa91ac2d7c4235031.json)
- [RUN-798c246ba58940a087bb6248e6e7365d](../runs/RUN-798c246ba58940a087bb6248e6e7365d.json)
- [RUN-f3b7d9bf4e9949568982eace7b31cdbb](../runs/RUN-f3b7d9bf4e9949568982eace7b31cdbb.json)
- [RUN-aebd4d720c7a41069b8abb20a372d1cf](../runs/RUN-aebd4d720c7a41069b8abb20a372d1cf.json)
- [RUN-b269c104e7a744c1a0d2c883de4b2bea](../runs/RUN-b269c104e7a744c1a0d2c883de4b2bea.json)
- [RUN-999aa4d5487b45738d2de39266416a33](../runs/RUN-999aa4d5487b45738d2de39266416a33.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
