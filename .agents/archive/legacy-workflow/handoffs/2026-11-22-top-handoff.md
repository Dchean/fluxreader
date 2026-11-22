# FluxReader 顶层安全点交接（2026-11-22）

此包用于独立核包和候任签收。它不授予候任立即调度权，不代表 S4b 产品、门禁、独审或发布通过。

## 当前目标与唯一调度

- 仓库：`D:/soft/fluxreader`，`dev`，HEAD 与 origin/dev 为 `902f4b9e86691755c4021687743ca87a08194a23`，暂存为空。
- 当前出任顶层：`01a1fd74-b635-7631-b666-3e1c4f85668a`。本包生成后停止新增产品/检查/阶段调度，只办理交接核验和一次正式交权。
- 在任 S4b 阶段：Ohm `01a1fe8c-8d57-77d3-9a67-5e1d9769bde0`；原阶段 `01a1fdfc-73b0-7000-aba3-18930fd3600f` 已结束，不再恢复调度。
- 最近唯一实施末端：Poincare `01a1fdfe-2475-7261-bdbb-06b1cabb9080`，现 idle。阶段与末端均已到原子安全点；当前无 Cargo/rustc 或本任务验证后台进程。
- 用户要求阶段新建对话采用非显性独立对话。两级总控常态只读；调查、业务实现、测试/QA 必须实际派末端，末端不能递归派发。独审另派全新只读上下文，不得自检冒充独审。

## 最新直接授权与上下文交接要求

用户本轮原文：“接受建议，不过不管是顶层总控还是阶段总控，达到上下文腐坏长度都必须进行交接”。前轮已给出的建议为：解除 `webdav_transport.rs` 和 `local_sync/runtime.rs` 对 D8/X25 必需范围的工作树冻结；先独立快照，最小清退旧链并修正内部 session/lease 归属，保留 D7 历史签收原件，随后定向验证、阶段适用门禁与独立只读审计。该建议现已获接受，不重复请批。

- 仅上述已批 D8/X25 工作范围可修改两个重叠生产文件；必要的 `local_sync/mod.rs`、测试及权威说明同步沿用 S4b 原授权。十个 D7 文件中的其他路径不因本次有限解冻自动获得新改动范围。
- D7 manifest、receipt、threshold audit、事件记录和既有 D8 原件仍不可改写、追加、重算收据以掩盖变化。后继产品版本另建原件和哈希，不能再声称变更后的工作树与旧 D7 全文件哈希相同。
- 保留 D7 已验收可见行为：单目录 256、XML 256 KiB、actor 64 数量/身份区分及准确提示；不得扩容、指导删除真实远端或伪称重连能降远端数量。
- X25 仅内部错误归属，既有 IPC code/message 形状不变；禁止 schema、持久格式、真实数据、远端 DELETE、未经批准的外部契约变更。
- 顶层与阶段达到交接条件必须在安全点换到新开的独立对话；压缩或 resume 不算换任。独立只读核包完成并由后继签收后，方可明确移交唯一调度权；前后任不能同时调度。
- 本顶层已有 7 次 `compacted` 记录，且前轮出现重复核验和调度反复，现主动交接。采样本地 `token_usage_record` 为 390；这不是供应商余量、硬请求上限或腐坏的统一数值阈值。
- 阶段 Ohm 已被要求评估自身及末端上下文能否承接下一批；达到条件也必须另开独立阶段接替。本次交接不以待审计为由授权旧阶段继续产品写入。

## 全路线与权限来源

唯一路线为 `tmp/audit-route-2026-11-14/路线方案.md`，先读 §2–5 与最新尾节；原用户产品裁决见 `tmp/decisions-2026-11-20.md`。不要按旧表重复做已完成条目。

- S1–S3 历史收口按路线与提交追溯。S4b/X10 已于 `4c5d8a2` 独立收口，排除在余项外。
- S4a 已独审并提交推送为当前 `902f4b9`；原件 `tmp/s4a-2026-11-21/top-git-closure.json` SHA256 `e7099b497e1b5932be0eacae5d2ab8c7a1d3f1e853c29c07bd702e451b2dcac4`。不复验 S4a，不把它的证据冒充本批新运行。
- S4b 原释放令 `S4B-WRITE-RELEASE-REMAINDER-20261121`：D7/N1、D8/N2/X11、N9/X23、X8/X12/X25/X26。顺序为传输容量/旧链/X25 → fast N9/X23 → storage X8/X12/X26，同文件单写者、共享验证资源串行。当前活动子批为 D8/X25；后继 fast/storage 不提前启动，D8 子批闭合后按原路线推进，无须新增用户许可。
- N9 仅摘要、JSON、actor 与路径不符这三种坏对象错误返回前消费名单头；GET None 已消费；传输/会话/DB/逻辑失败保留重试。X23 只过滤通过传输安全校验但不合 intent 名称语法者并计数，不改/移/删远端，不记成功 ACK；危险路径/非法 XML 保持原错误。
- X8 为 peer_frontiers 无用 current 参数；X12 仅现有格式边界，可按准备保留并说明；X26 只记 generation 读写/GC 边界，不实现 GC/远端 DELETE。
- S4c、S4d 和 S5 尚未获得产品写权/运行资源；只读准备在各自 tmp 目录，等 S4b 阶段终审和精确资源释放。D3/D4/D9 等生效裁决以原决定文件及路线后继记录为准，旧“待拍板”不能外推。
- 阶段和末端不得 commit/push/Actions/merge/tag/release。顶层历史已有“合格阶段提交推送 dev”的用户授权，其原文需从历史会话和路线所指原件核实，执行时只选受验受审文件，含 `[skip ci]`、关闭 followTags、禁止 Actions。此包不自行执行或扩大 Git 写权；合并、tag、发布仍无授权。

## 当前产物、原件与验证边界

当前 12 个受管改动的原始状态、字节副本和哈希见同目录 `scope-snapshot.json`，SHA256 `e6debbdf9ef54745ebf4dd6f0dd0cfe6141faa6f206b1d8894472d1b2052e664`。`worktree-bytes/` 是本次独立接续快照，不是旧 D7 事故之前的原始副本；每项复制前后与副本 SHA 均相等。全部 10 个 D7 文件在此安全点仍匹配旧清单。禁止恢复这些副本覆盖工作区；仅用于核对与必要的获权恢复评估。

- D7 签收 manifest：`tmp/s4b-2026-11-21/remainder/d7-freeze-manifest.json`，16,062 字节，SHA256 `92b1d316c6168f000357e6a17322da2f58e70aaff487d32c8b49ac1baffe5d0c`；receipt 匹配。
- D7 冻结件发生过未经授权的临时改写与回写，mtime 为 `2026-11-21T21:40:22.2113596+08:00`，无独立事前原始副本；SHA 相同不抹除事故。独立事件 `stage-incident-20261121-d7-freeze-write.json` SHA256 `0f1b155f919088b17880174a06d45e704fac2c4689368722327e0465fb4294b8`；threshold audit 是额外证据，不纳入原签收。
- D7 格式化后三组目录/XML/actor 共五个最终 filter、最终 fmt、docs 检查记录在 manifest 内。D7 用户签收仅为切片，不是 S4b 全部产品独审/门禁通过。
- 新 `src-tauri/tests/webdav_library_e2e.rs` SHA256 `b4eb629560f62302f7f22e9dcbef474ec5b49f0ecc844764458ea055f9a08991`。覆盖 PUT 丢响应 OutcomeUnknown、精确字节重试无第二次 PUT、异字节 Conflict/不覆盖，以及活动 library_list XML 256 KiB 的 Content-Length/EOF 超限与恰好上限接受。
- D8 迁移原件 `d8-migration-evidence.json` SHA256 `23a968e14f866063a79825ff489c8caabbb969f65e731ff18f7d3698ce5f5960`；现行计划 `d8-x25-write-plan.json` SHA256 `9166c1919cd9d9d81fd95ab7cedd349d2e7a677ef557ede08d06b1cca087fda3`。计划里的旧冻结限制已被本轮用户有限解冻取代，原件仍只读，不改写旧计划。
- 旧 `src-tauri/tests/webdav_transport_e2e.rs` 仅删除已承接的两个 mock 和其独占 helper，未整文件删除；当前 SHA256 `106f1f21d7a622594d5d6d9480ce9b242cefb632dddc9699f2b9531dbcd964bf`。
- 清理分类 `d8-webdav-transport-e2e-classification.json` SHA256 `d32a03ce668490619093c6c1d0e8e14101d79a3e501df00086c13bd266c76b59`；清理收据 `d8-webdav-transport-e2e-cleanup-result.json` SHA256 `5dc6c1ea0bdd9bf453ad4fa57a4bffe0e89bbe9b82a3b3efd468cf7266d81e0d`。精确 diff 原件及 SHA 在收据中。
- 清理后新 run：`d8-legacy-e2e-cleanup-fmt-check`、`d8-legacy-e2e-new-put-filter`、`d8-legacy-e2e-new-xml-length-filter`、`d8-legacy-e2e-new-xml-eof-filter`，均 exit0/inputStable=true。实际 fmt argv 为 `cargo fmt --all --check`；不要把报告写法冒充带独立 `--` 的实际 argv。
- 首次 D8 编译失败和第二次夹具路径 panic、rustfmt 前失败、Windows 换行比对失配及手工抄错预期哈希均保留；不是产品真红。不要把没有业务断言运行的失败当防复发红证据。

## 尚待完成与首个可执行步骤

1. 全新只读审计者先核本交接包、现场、原始运行结果和授权冲突；候任顶层只读签收。出任顶层发正式交权记录后停止调度。
2. 后继顶层核阶段健康；触发交接就先按规则更换阶段，且由阶段评估是否更换长上下文 Poincare。原末端已安全 idle，无需强断或杀进程。
3. 新任正式授权阶段按本轮有限解冻继续 D8/X25。实施前核独立快照、基线/暂存/写者/进程。保留生产 PreparedBatch、Batch/Checkpoint/Intent 格式、serde、预算及有效 Replica/dominates 模型/SQL 对照。
4. 旧 probe/lease/MOVE API 没有当前业务生产调用者，但保留的 ignored WsgiDAV 测试仍是实在的测试消费者。不能称全仓零消费者，也不能简单删除该测试丢失有效断言。先核原 D8 裁决，按真实在用入口承接有效保护，或说明保留部分的边界；不要为旧链测试复活已批准清退的生产协议面。
5. X25 精确现状：runtime::transport_guard 将 ensure_session 的 stop/generation/库/DB 失败包装为 LeaseExpired；webdav_transport::ErrorKind 同时被旧 LOCK 路径使用。候选方案需在正式实现中核真实调用和外部映射，不能把旧链内部存在 parse_lock/check_lease 当成有当前业务生产消费者的证明。保留外部 code/message 及 stop/generation 语义，新增防复发先在隔离旧源码真实断言红，再修后绿。
6. 定向开发验证后集中适用门禁：lint/build/test:frontend/test:components/test:ui-layout/docs:check/Cargo fmt，以及本地隔离 Cargo check/clippy/test（禁止 Actions，不能留给未运行 CI）。输入未变复用 S4a 有效证据，输入改变明确重评，不无故重复。Node24、既有 tmp/rust-env、进程局部短 PATH/隔离 TEMP/APPDATA/DB/profile；不改全局配置/权限/UAC。真实窗口未授权，Edge headless 生命周期按本任务精确监督。
7. 冻结产品/测试/说明后派全新只读产品独审；自检、交接核包和用户 D7 签收均不能替代。阶段只写 audits/README 登记，其他权威说明由末端同步。最后终审后再按原路线续 fast/storage。

## 核验性质与恢复边界

本包只做只读原件核对、工作树字节保存与必要调度记录；未写业务代码、未运行产品门禁、未改真实配置/数据库/远端服务。S4b 全阶段仍未审结、未体验验收、未提交或推送本批；无发布结论。后继只补缺口与矛盾，不重做已有效部分。此包若与现场冲突，保留现场并给出精确差异，不自动恢复或重算旧证据。
