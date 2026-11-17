# 审计记录

每轮审计、证伪与修复基线的结论摘要。**原始输出留在 `tmp/`（不入库）**；本页只登记可追溯的摘要、基线与复跑入口，避免机器清理后断档。规则见 [维护与文档纪律](maintenance.md) 第 2 节。

| 日期 | 范围 | 基线 | 结论 | 条目与缺口 | 证据位置 | 处置状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-11-17 | S3b-ui 半批：N-16（components 域 isTauri）/X-6/P2-17+N-12/P2-13；1 实现者 + 独立审计 1 子代理 | `c1fbb32` | 通过：五项前端门禁审计复现全绿（lint/build/test:frontend 1516/test:components 358+2/test:ui-layout ok）；X-6 四段逐字段等价、P2-13 两份逐字同形、P2-17 五处可达输入行为一致；`canProxyImages` 与其 3 个未转换消费者留 S3b-store 半批 | 次要 3 条：App.tsx 旧式为裸 window 判断（新 isTauri 多 typeof 守卫，无可达行为差）、提交正文文件清单表述不全（非夹带）、P2-17 注释措辞对空串略宽（后续收紧） | 复跑：五门禁（原生 Node 24，串行） | 已审计通过 |
| 2026-11-17 | 文档清账批：matrix 3 项事实 + performance 3 处 + roadmap 待办清账；1 实现者 + 独立审计 1 子代理 | `0a88963` | 通过：6 项验收全成立——429 退避（origin 键/秒数与 RFC2822/缺省 60/下限 1/仅 GR+Fever）、ClientLogin 双格式（JSON 优先，失败或空 Auth 回退文本）、32 MiB 正文对象（primary/rss 分别计）、封面守卫 UPDATE 与弃用原因、45s 首轮可终止、五类回落覆盖六条件；docs:check 12 篇 | 次要 1 条：scheduler.rs attempt_cover 注释仍有 NULLIF 旧措辞（后续触碰时改） | 复跑：docs:check（两次 exit 0） | 已审计通过 |
| 2026-11-15 | 批 F-1（S3a）：P2-14/P2-16/P2-18/P3-15/X-5 死代码删除；1 实现者 + 独立审计 1 子代理 | `55e9e5c` | 通过：五门禁审计复现全绿；12 类删除经父/HEAD 类计数差恰 12 佐证、括号平衡校验；X-5 处置修正——useAutomaticAi.ts 的 AUTO_AI_DWELL_MS 实为文件内活跃消费（审计"零消费"表述与代码不符），总控裁决去 export 化，审计独立核验成立 | 次要：frontend.md 台账未随批更新（366→354）——已即时补同步（本登记提交）；"一批一个模块"经审计接受为前端清理整体批 | 复跑：五门禁（原生 Node 24） | 已审计通过 |
| 2026-11-15 | 批 C（S2c）：P1-3（D2=A 删索引）/N-20/P2-4/N-7/X-9/P3-3/P3-18/P3-19 + P3-20 评估不动 + N-19 Note（总控派发遗漏，补账时一并）；2 实现者 + 独立审计 1 子代理（初判不通过：X-9 测试未入库、P3-20 未留档 → 补账提交 57129c5 + 本登记 → 局部复审） | `94afe80` + `57129c5` | 初审时 S2b（5b870cc）已通过；批 C 门禁复现全绿（cargo 三件套 + docs:check；探针与 schema 36+22 双向比对全量一致）；两个记录缺口修复中 | P3-20 评估留档：articles.rs 双分支 UPDATE 重复约 16 行 SET 阶梯，合一收益=消漂移风险、风险=CASE 守卫/参数重排易错；commands/folders.rs 全表回查为 add-feed 低频小 n，修复需动非本批文件——建议单列小步（只合一 SET 片段生成 + 相关 e2e），不与注释批混做。次要 2 条留后续：ingestion/mod.rs 毗邻注释残留、N-19 触发窗口未实测（Note 已定性） | 复跑：原生 rust-env cargo 三件套 + docs:check；X-9 测试 `bucket_index_survives_short_or_invalid_digest` 已入库（57129c5） | 已审计通过（局部复审闭合） |
| 2026-11-15 | 批 B（S2a）：P2-5/P2-6/N-3/P3-1/P3-4/P3-5/P3-7/P3-8/P3-9/P3-10/X-14/B6；3 实现者 + 独立审计 1 子代理 | `c8d03f8` | 通过：四门禁复现绿（cargo 三件套 + docs:check；lib 676/62 目标）；P2-6 连带清理只写私有字段；P3-6 作废（§6-21）；遗留归口 1 条：greader::VerificationItems.complete 零读者 pub 只写字段（已补账 Note，归口批 G） | 证据：提交正文、tmp/test-temp 日志、审计复现命令随报告 | 已审计通过 |
| 2026-11-15 | 批 A（S1）：P1-2/P2-1/N-5/P2-2 实现 + X-13 评估不引入白名单；3 实现者 + 独立审计 1 子代理（初判不通过：两个记录缺口 → 补账 → 局部复审闭合） | `1a6698d` + `bfa789b` | 最终通过：8 项门禁审计复现全绿（cargo 三件套 rust-env + 前端四门禁 + docs:check；fever_compat 57、lib 676、components 358+2）；缺口为文档级（私网边界未标注、X-13 理由未落档），bfa789b 补齐；次要 2 条留后续：v6 文档段 3fff::/20 口径、discover_favicon 死形参 | 证据：提交正文、favicon/roadmap diff、tmp/test-temp/audit-components-business.log；复跑命令见审计记录 | 已审计通过 |
| 2026-11-15 | Node 基线 22→24 与探针 reporter 固定（N-8 提前完成，用户裁决与 N-8 冲突处以 N-8 为准）；独立审计 1 子代理 | `80783b6` | 审计通过：7 条验收全成立、N-8 三项逐字核对成立；A/B 实证证明 pin 必要且有效；断言零改动 | 限制 2 条：Node 22 腿本机无版本未实测（机制论证 + Node 22 官方 NODE_OPTIONS 清单佐证）；Note 曾有悬空称谓（本次顺手修正） | `tmp/rust-env/cargo-clippy-all-xwin-msvc.log`；审计报告全文在子代理记录；复跑：原生 Node 24 `npm run test:components`、`node --test tools/release/release.test.mjs` | 已审计通过 |
| 2026-11-15 | 文档对照：8 篇既有文档逐篇 vs 代码 + 前端组件/类名台账；6 个调查子代理 + 3 个独立证伪子代理 | `77c1964`（对照）/ `c6a72b8`（台账入库） | 对照通过：8 篇均“保留+局部更新”；24 条声明 22 成立、2 部分成立（表述细节）、1 推翻（tone 实为五值，local 复用基类——已更正台账）；12 个零引用 CSS 类与已知 P2-14 口径互证 | 21 条新登记（9 失实 + 6 缺口 + 2 裁决 + 1 失效锚 + 1 更正）：S0.5 批落笔 19 条（architecture/development/README/backup/roadmap/ci.yml/AGENTS），performance.md 与 sync-compat-matrix.md 按写者顺序留 S2c/S5；两裁决已定（六套 mock 改不枚举表述；clippy 实跑通过补日志） | `tmp/s0-2026-11-15/`（对照底稿、falsify-*.md、class-ledger.json/md、scan 脚本） | S0 完成、S0.5 本批落笔 |
| 2026-11-14 | 全项目：同步协议、数据层、前端、测试工具链、WebDAV/local_sync、AI；7 轮、≥27 个子代理 | `d4a460d`（工作树干净） | **不通过**：9 条重要项待修 | 101 条（重要 9 / 次要 55 / 信息 37）+ 9 决策 + 6 已推迟 + 8 勿动 + 20 条前轮纠正；缺口：`db/outbox.rs`、`commands/sync.rs`、`commands/settings.rs` 未覆盖 | `tmp/audit-2026-11-14/最终审计报告.md`、`修复优化指导.md`、`tmp/audit-falsify-2026-11-14/`（工作底稿与证伪报告） | 待修复 |

## 修复路线

当前路线：`tmp/audit-route-2026-11-14/路线方案.md`（v2，工作文件，不入库）。约束性内容以 [维护与文档纪律](maintenance.md) §3/§9/§11 为准；路线只是排期，不替代规则。

## 复跑入口

- 前端全量状态回归：`npm run test:frontend`（2026-11-14 本机 1516/1516 通过）。
- 组件回归：`npm run test:components`（2026-11-15 起探针 reporter 固定 tap，Node 24 原生通过；项目 Node 基线随 `.node-version` 升至 24）。
- 审计条目复核方式：按报告 §1 的位置列 grep/read；静态结论的触发前提写在条目内。

## 覆盖矩阵（累计表；每轮只更新行，不整体替换）

深度口径：**逐行** = 关键路径逐函数核对；**关键段** = 只核公开入口、错误路径与边界；**结构级** = 只核模块边界与调用关系。空着的格子写"未覆盖"，不留白。

| 域 | 最后审计日期 | 基线 | 深度 | 证据/位置 |
| --- | --- | --- | --- | --- |
| 同步协议（`sync/`、`greader.rs`、`fever.rs`、`sync_http.rs`） | 2026-11-14 | `d4a460d` | 逐行 | 报告 §1.1/§1.2；四轮独立复核 |
| 数据层（`db/` 的 articles/feeds/folders/sync_map/writer/contents/backup） | 2026-11-14 | `d4a460d` | 逐行（核心）/关键段（备份） | 报告 §1.1/§1.2 |
| 数据层（`db/outbox.rs`、`db/body_retention.rs`、`db/url_norm.rs` 全量） | 2026-11-14 | `d4a460d` | 未覆盖 / 抽样 | 仅核过入口与注释 |
| 前端 store 与关键组件（internals、slices、Timeline、Overlays、Reader、api） | 2026-11-14 | `d4a460d` | 逐行 | 报告 §1.2 前端条目 |
| 前端（`PlayerBar`、`GalleryLayout`、`useProseImages`、`imageProxy` 全量） | 2026-11-14 | `d4a460d` | 结构级 | 未逐行 |
| 测试工具链（`tools/` 的 runner、断言框架、夹具；`ui-layout` 中段） | 2026-11-14 | `d4a460d` | 逐行（框架）/结构级（`ui-layout` 中段） | 报告 §1.2/§1.3 |
| WebDAV 与本地同步（`webdav_transport.rs`、`local_sync/`） | 2026-11-14 | `d4a460d` | 关键段（新增一轮） | 报告 §2 N-1；本轮才补 |
| AI 全链路（`ai.rs`、`ai_segments.rs`、`commands/ai.rs`、AI store） | 2026-11-14 | `d4a460d` | 逐行（新增一轮） | 报告 §1.2 的 X 条目 |
| 调度器与抓取（`scheduler.rs`、`ingestion/`） | 2026-11-14 | `d4a460d` | 结构级 | 只核了节奏与收体边界 |
| 命令层（`commands/settings.rs`、`commands/sync.rs` 全量） | 2026-11-14 | `d4a460d` | 未覆盖 | 只核了被引用的函数 |
| 文档与注释一致性 | 2026-11-14 | `d4a460d` | 抽样（发现 20 条纠正） | 报告 §6 |
| 文档逐篇对照（architecture/development/backup/release/根README/roadmap/sync-compat-matrix/performance） | 2026-11-15 | `77c1964` | 逐篇（每条可检验断言核对） | `tmp/s0-2026-11-15/`（8 份对照底稿 + 3 份证伪记录） |
| 前端组件/类名/令牌台账（src/ 全量扫描 + 动态拼接排查 + 全仓死类复核） | 2026-11-15 | `77c1964` | 全量扫描（静态） | `tmp/s0-2026-11-15/class-ledger.json`、`docs/frontend.md` |

**结论**：不是"全覆盖"。下一轮应按"未覆盖 + 最久未碰"排序补齐，而不是重跑已逐行的域。

### 下一轮覆盖优先级

1. `db/outbox.rs`、`commands/sync.rs`、`commands/settings.rs`（未覆盖，且是订阅写回与设置写入的主干）。
2. `ingestion/staged.rs`、`db/body_retention.rs`（正文保留与暂存，和 N-1/X-26 的对象生命周期相关）。
3. 前端 `PlayerBar`、`GalleryLayout`、`imageProxy` 全量（只做过结构级）。
4. 文档全量梳理（路线见 `tmp/audit-route-2026-11-14/`）。