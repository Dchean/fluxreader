<!-- project-workflow: generated view; edit task JSON instead -->
# TASK-056 · 修 pull 建订阅的外键违约 + 静默吞错：FreshRSS 登录成功却拉不到订阅

**状态**：done

**目标**：用户实测：用 `https://demo.freshrss.org/api/greader.php` 登录**成功**，但**一条订阅都没拉进来**。根因经实证定位为两处叠加：**(1) 外键违约**——`sync/subscriptions.rs` 在 pull 建本地订阅时用 `db::get_first_folder_id(&conn).ok().flatten()).unwrap_or(1)` 兜底 folder_id，而**新装应用的 folders 表为空**（用户真实库复制件实测 `folders: 0`），于是硬编码的 `1` 指向不存在的目录；`feeds.folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE` 且连接开启了 `PRAGMA foreign_keys=ON`，插入必然抛 `FOREIGN KEY constraint failed`（已实测复现）。**(2) 错误被静默吞掉**——`if let Ok(fid) = inserted { ... }` **没有 else 分支**，插入失败既不记入 `report.errors` 也不上抛，`feeds_phase` 照常返回 Ok，前端因此弹出『已拉取订阅源』『后端同步完成』，用户看到的是成功、实际零订阅。仓库**已有**正确兜底函数 `db::ensure_uncategorized_folder()`（不存在则创建『未分类』并返回其 id；`add_feed` 路径的 `commands/folders.rs:152` 正在使用），pull 路径却未使用它。本任务改用该既有函数，并把失败写入 `report.errors` 让前端既有 toast 能真实报错。

**依赖**：TASK-055
**参考方案**：见 ../RESEARCH.md
**界面约定**：不涉及界面
**界面检查**：不适用
**修改范围**：.workflow-kit/tasks/evidence/baseline-2026-09-18-task056.md, src-tauri/src/**, src-tauri/tests/**

## 验收标准

- **复现测试先证缺陷成立**：新增一个**零 folder 的空库**场景测试（这是此前测试网的整体盲区——所有既有 e2e 都先 `create_folder` 再插 feed），断言「远端有订阅时，pull 后本地应确实建立该订阅」，并给出**修复前的失败证据**（修复前该断言必须失败）
- **修复后转正必过 + 证明捕获性**：同一测试修复后通过；并用变异测试（把兜底改回 `unwrap_or(1)`）证明回退后会失败——须确认出现 `Compiling app` 才采信结果（cargo mtime 指纹可能复用陈旧 rlib）
- **兜底改用既有函数**：不再硬编码 `unwrap_or(1)`，改用 `db::ensure_uncategorized_folder()`（或等效的『确保存在』语义）；报告中说明为何这是正确兜底（目录必须真实存在才满足外键）
- **静默吞错必须消除**：pull 建订阅失败时**必须**写入 `report.errors`（含失败 URL 与错误原因），不得再用无 else 的 `if let Ok(...)` 吞掉。报告中给出修复前「report.errors 为空却零订阅」与修复后「report.errors 含具体原因」的对照证据
- **逐一排查同类吞错**：全仓搜索 `subscriptions.rs` / `entries.rs` 及其它 sync 路径中**插入/写入失败被静默忽略**的位置（`if let Ok(...)` 无 else、`let _ =` 忽略有意义返回值等），逐条判断应否上报；未改的须说明理由。**特别核对 `entries.rs` 的 pull 条目插入路径是否有同类问题**
- **不得只修一半**：验证时须覆盖「零 folder 新库」与「已有 folder 的库」两种情形，确认后者行为不回归
- 既有 Rust 测试不得回退：`cargo test` 通过（基线 **137 passed / 0 failed / 9 ignored**，即 TASK-055 之后的值；通过数可增不可减，ignored 不得增加）
- 四门禁全绿：`cargo test`、`npm run lint`、`npm run build`、`npm run test:frontend`（前端 241/241）
- 不引入新依赖；`Cargo.toml`/`Cargo.lock` 零改动；不改前端；不改表结构
- 文本文件必须 LF 行尾；台账改动须在 begin 之前完成；行数以 `splitlines()` 口径报告（不得用 `Measure-Object -Line`）
- **用户真实数据库不得写入**：`%APPDATA%\com.fluxreader.app` 下任何文件只读使用（如需诊断，复制到临时目录后操作）；不得在其中创建/修改数据

## 测试适用性

- 既有行为：按已确认需求变化
- 原始基线：PASS；2026-09-18 基线（TASK-055 之后）：`npm run lint` exit 0（0 warnings/0 errors）、`npm run build` exit 0、`npm run test:frontend` exit 0 且 241/241、`cargo test` **137 passed / 0 failed / 9 ignored**（+1 为 TASK-055 新增的墓碑复现测试）。**本任务有意改变行为**：pull 建订阅的目录兜底由『硬编码 1』改为『确保『未分类』存在』，且插入失败由**静默**改为**写入 report.errors 上报**。前者修复外键违约（功能从『零订阅』变为『正常拉取』），后者是**可观测性契约变化**（失败的同步不再对外表现为成功），已由 DEC-user-testing-bugs-20260918 覆盖（owner 明确选择『一并修复静默吞错』）。**基线盲区须如实记录**：现有 Rust e2e 与前端回归**全部**在插入 feed 前先 `create_folder`，**没有任何测试覆盖『零 folder 的新库』**——这正是本例逃过既有测试网的结构性原因，本任务必须补上该场景。
- 基线证据：.workflow-kit/tasks/evidence/baseline-2026-09-18-task056.md
- 需求决定：DEC-user-testing-bugs-20260918
- 补充：『零 folder 空库 + 远端有订阅 ⇒ pull 后本地确实建立订阅』的复现测试（含修复前失败证据）；该场景是既有测试网的整体盲区（所有 e2e 都先建目录），缺陷恰在此逃逸；必须有可复跑且经变异证明具备捕获力的证据。；验证：cargo_test
- 补充：『pull 建订阅失败 ⇒ report.errors 非空且含原因』的可观测性测试；静默吞错是本缺陷难排障的根源；owner 明确要求一并修复，须有断言锁定『失败必须可见』这一新契约，防止回退成静默。；验证：cargo_test
- 适配：`sync/subscriptions.rs` pull 建订阅分支的目录兜底与错误处理（含相关文档注释）；兜底值从非法常量改为确保存在的目录；错误处理从吞掉改为上报。这是本任务的核心行为变更，注释须与新语义一致。；验证：cargo_test
- 保留：既有 sync_gap_repro_e2e.rs 全部 15 个测试（含 TASK-055 新增的墓碑复现测试与 A-1 防复活保护）；这些是 REQ-002/003 的直接回归网；本任务只改 pull 建订阅的兜底与错误处理，不得削弱防复活、离线入队、对账防误判等既有保证。；验证：cargo_test
- 保留：其余全部 Rust 测试（合计 137 passed）与前端 241 项断言；本任务改同步拉取路径，须证明推送顺序、对账口径、状态保护与前端契约均未被波及。；验证：cargo_test, frontend

## 执行与恢复

- 首次开始：2026-09-18T08:27:39.988860Z
- 原截止时间：2026-09-18T12:27:39.988860Z
- 当前截止时间：2026-09-18T12:27:39.988860Z
- 时钟：按墙钟计：额度 240 分钟，写入阶段已用约 17 分钟
- 已用修复轮：1
- 阻塞：无
- 下一步：继续已授权任务；所属功能完成后请用户验收

## 最近检查点

- 2026-09-18T08:27:40.096800Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T08:45:03.732882Z：Worker requests manager action; inspect the result；下一步：先核对已有文件及原始日志，再处理 action_required；不要新建任务或重置预算
- 2026-09-18T08:54:47.972300Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T09:39:48.149020Z：Review requires changes; inspect the findings；下一步：先核对已有文件及原始日志，再处理 review_failure；不要新建任务或重置预算
- 2026-09-18T10:17:09.484218Z：开始执行，保留原任务身份和截止时间；下一步：完成当前修改后调用 finish，再运行 verify
- 2026-09-18T10:17:18.905227Z：Worker changed_files does not match the observed project diff；下一步：先核对已有文件及原始日志，再处理 protocol；不要新建任务或重置预算
- 2026-09-18T10:22:02.652785Z：预先定义的必需测试全部通过，日志已保存；下一步：审查当前候选；独立审查使用没有参与编码的新上下文
- 2026-09-18T10:58:19.588519Z：当前候选的测试与审查通过；下一步：继续已授权任务；所属功能完成后请用户验收

## 原始证据

[唯一状态记录](../items/TASK-056.json)

- [RUN-7a6709f6a502438095cb1336d784e8b0](../runs/RUN-7a6709f6a502438095cb1336d784e8b0.json)
- [RUN-cc94765bf5dc402bb66910f51e752071](../runs/RUN-cc94765bf5dc402bb66910f51e752071.json)
- [RUN-5ea21cd8294949a3b6961747c1d9650a](../runs/RUN-5ea21cd8294949a3b6961747c1d9650a.json)
- [RUN-3409f1a1d0ae42efb51ef197b4d0a386](../runs/RUN-3409f1a1d0ae42efb51ef197b4d0a386.json)
- [RUN-9d76e4a7a5b8409cb0ccfe38b337c8da](../runs/RUN-9d76e4a7a5b8409cb0ccfe38b337c8da.json)
- [RUN-d2a1d3663f0247239682eb94ba4ed412](../runs/RUN-d2a1d3663f0247239682eb94ba4ed412.json)

卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。
