# M2 模块处置方案：逐模块保留/整理/替换/删除分析与目标边界

**任务**: TASK-016  
**运行**: TASK-016-B5  
**基准**: main@443fb86 (BATCH-004 合并后)  
**日期**: 2026-09-13  
**分析范围**: 全部前端与后端模块  

---

## 执行摘要

按 PROCESS.md M2 要求，对前后端全部模块进行处置分析。证据基于代码行数、职责计数、SQL 散布点、测试覆盖与依赖关系。**所有建议保留 FEATURES.md 已确认的核心与全部 OPT-001～010 能力**，不缩减功能。

**关键发现**：
- **ISSUE-008 (SQL 边界)**：commands.rs 13 处、sync.rs 17 处直接 SQL，db.rs 集中承诺未兑现
- **ISSUE-009 (职责聚合)**：store.ts 1494 行含 22+ 职责、commands.rs 1370 行混杂 IPC/业务/SQL
- 同步领域与协议类型耦合中等，已通过 Backend 枚举封装大部分差异
- 测试对全局状态依赖低（19 测试文件，无共享 DB 冲突记录）

**试点建议**：SQL 边界收敛（db.rs），范围最小、可自动验证、能代表目标模式。

---

## 一、前端模块处置

### 1.1 核心状态管理（src/store.ts）

**现状量化**：
- 行数：1494 行
- 职责数：22+ 个（导航、筛选、水合、AI 流、同步、播放器、订阅管理、设置、弹层、GitHub 流、搜索锚定等）
- 导出函数：91 个 action + selector
- 外部依赖：api.ts (IPC)、selectors.ts、types.ts、mockData

**处置建议**：**整理** — 按领域拆分 slice，保留 Zustand 单 store

**理由**：
1. 职责过度聚合（22+ 个独立领域混在一个文件）是 ISSUE-009 的典型体现
2. 文件长度（1494 行）超出单屏可读性阈值，变更风险高
3. 但核心架构（Zustand 单 store + selector 派生）**运作良好**，无需替换技术栈
4. 不做理由的最强论据：Zustand 单 store 是当前技术选型，功能完整且测试覆盖，重写为 Redux Toolkit 或其他方案成本高、风险大、收益不明确

**目标边界**：
- 按领域拆分为 store/navigation.ts、store/ai.ts、store/sync.ts、store/player.ts、store/feeds.ts、store/settings.ts
- 主文件仅组合 slice + 导出统一 hook
- 每个 slice 独立可测，互不污染
- 保持现有 API 契约，前端组件零改动

**依赖风险**：
- 低：拆分是纯内部重构，对外 API (useAppStore) 不变
- 需同步更新 store/types.ts 的类型定义

---

### 1.2 API 层（src/lib/api.ts）

**现状量化**：
- 行数：569 行
- 职责数：6 个（IPC 封装、类型映射、mock 回退、错误提取、行转换函数、批量导出）
- 导出函数：50+ IPC 命令 + 3 转换函数
- 外部依赖：Tauri invoke、types.ts

**处置建议**：**保留** — 职责单一且清晰

**理由**：
1. 职责明确：IPC 封装 + Rust/前端数据映射，无业务逻辑泄漏
2. 行数合理（569 行），对应 50+ 后端命令的类型安全封装
3. 不做理由：IPC 层是前后端边界的必要抽象，删除或合并会让 store.ts 直接调用 Tauri invoke，降低可测试性

**目标边界**：
- 维持现状：IPC 命令 1:1 映射后端 commands
- 类型安全：TypeScript 类型与 Rust Serialize 结构严格对齐

**依赖风险**：无（边界清晰，变更隔离）

---

### 1.3 Selector 层（src/store/selectors.ts）

**现状量化**：
- 行数：192 行
- 职责数：8 个派生计算（布局解析、视图筛选、可见条目、计数聚合、树角标、feed 配置、数字 id 提取）
- 导出函数：12 个
- 外部依赖：types.ts、format.ts

**处置建议**：**保留** — 职责单一，性能关键

**理由**：
1. 派生数据计算与 store 分离是 Zustand 最佳实践
2. 选择器带引用缓存（visibleEntriesCache），避免重复计算
3. 不做理由：合并回 store.ts 会让 store 职责更混乱，且失去计算优化机会

**目标边界**：
- 维持现状：派生计算独立于 store 状态
- 按 store 拆分后，selector 也对应拆分（store/navigation-selectors.ts 等）

**依赖风险**：无

---

### 1.4 组件层（src/components/*.tsx）

**现状量化**：
- 文件数：11 个 TSX 文件
- 平均行数：~200 行/文件（Reader.tsx 最大，约 400 行）
- 职责：UI 渲染 + 交互逻辑
- 导出组件：15 个
- 外部依赖：store.ts、api.ts、primitives

**处置建议**：**保留** — 组件粒度合理

**理由**：
1. 组件按功能拆分（Timeline、Reader、Sidebar、SettingsModal 等），职责清晰
2. Reader.tsx 400 行因需处理多种布局（文章/社交/画廊/播客/通知），合理
3. 不做理由：进一步拆分会产生过多文件间跳转，降低可读性；当前粒度在 React 社区属常规

**目标边界**：
- 维持现状：按功能页面/区域划分组件
- 随 store 拆分，组件 import 路径更新（useAppStore → useNavigationStore 等）

**依赖风险**：低（仅 import 路径变更）

---

## 二、后端模块处置

### 2.1 核心 IPC 层（src-tauri/src/commands.rs）

**现状量化**：
- 行数：1370 行
- 职责数：10+ 个（Folders、Feeds、Articles、Settings、刷新、AI、OPML、同步、缓存清理、GitHub 流）
- Tauri 命令数：46 个
- **SQL 散布点**：**13 处**直接 execute/query_row/prepare（违反 db.rs 集中承诺）
- 外部依赖：db、sync、ingestion、ai、opml、scheduler

**处置建议**：**整理** — SQL 全部移至 db.rs，拆分命令组

**理由**：
1. **ISSUE-008 核心证据**：13 处 SQL 散布在 IPC 命令中（add_feed L187-199、update_feed L276-283、mark_all_read L446-475 等）
2. 职责混杂：IPC 参数验证、业务逻辑、直接 SQL、错误处理全在一处
3. 不做理由：全删除会失去 IPC 入口，替换为其他 RPC 框架（gRPC/tRPC）成本极高且无必要

**目标边界**：
- commands.rs 只保留：IPC 参数反序列化、锁管理、调用 db:: 类型化函数、序列化返回
- 所有 SQL 移至 db.rs（如 mark_all_read 的动态 SQL 构建 → db::mark_all_read_scoped）
- 按领域拆分为 commands/folders.rs、commands/articles.rs、commands/sync.rs 等

**依赖风险**：
- 中：需确保 SQL 迁移后语义不变（单元测试覆盖）
- 已有 95 项 Rust 测试通过，为重构提供安全网

---

### 2.2 数据访问层（src-tauri/src/db.rs）

**现状量化**：
- 行数：2227 行
- 职责数：12 个（迁移、Folders、Feeds、Articles、搜索、同步映射、Settings、队列、去重、清理、URL 规范化、测试）
- 公开函数：70+ 个
- **SQL 集中度**：**70%**（大部分在此，但 commands.rs 13 处、sync.rs 17 处仍散布）
- 外部依赖：rusqlite、rusqlite_migration、credentials

**处置建议**：**整理** — 收敛全部 SQL，内部按领域分模块

**理由**：
1. **ISSUE-008 的解决关键**：db.rs 承诺"所有 SQL 集中"未兑现，需补齐 commands/sync 的 SQL
2. 2227 行已较大，收敛后会更大，需内部模块化（db/folders.rs、db/articles.rs、db/sync.rs）
3. 不做理由：用 ORM（Diesel/SeaORM）替换会强制重写全部 SQL，且 SQLite 个人场景下裸 SQL 更直接

**目标边界**：
- 100% SQL 集中：commands.rs 和 sync.rs 的 SQL 全部抽取为 db:: 函数
- 内部拆分为 db/folders.rs、db/articles.rs、db/sync.rs、db/search.rs、db/migrations.rs
- 主文件（db.rs 或 db/mod.rs）仅 re-export

**依赖风险**：
- 低：纯内部重构，对外 API（db:: 函数签名）不变或只新增
- 迁移测试（migration_test.rs）已覆盖 schema 演进，保证 SQL 正确性

---

### 2.3 同步引擎（src-tauri/src/sync.rs）

**现状量化**：
- 行数：1265 行
- 职责数：7 个（凭据读取、Backend 枚举、Push 计划+执行、Pull feeds/entries、状态对账、协议分派）
- **SQL 散布点**：**17 处**直接 execute/query_row/prepare
- 外部依赖：db、greader、fever、error、chrono

**处置建议**：**整理** — SQL 移至 db.rs，内部按 Push/Pull/协议分层

**理由**：
1. **ISSUE-008 的另一重灾区**：17 处 SQL（plan_push L187-194、pull_feeds L403-446、merge 路径等）
2. 协议耦合：Backend 枚举已封装大部分差异，但 pull_entries_greader/pull_entries_fever 仍有重复逻辑
3. 不做理由：同步是核心能力（REQ-SYNC-001），全删除会丢失 Google Reader/Fever 支持，替换协议库（RSS Guard 的 sync 模块）不兼容现有数据模型

**目标边界**：
- SQL 全部移至 db/sync.rs（如 plan_push 的 query_row → db::sync_plan_read_queue）
- 内部拆分为 sync/push.rs、sync/pull.rs、sync/protocol_greader.rs、sync/protocol_fever.rs
- 共享合并逻辑（merge_pulled_entry）抽取为 sync/merge.rs

**依赖风险**：
- 中：同步逻辑复杂（绑定回填、副本记账、状态对账），需完整 E2E 测试覆盖
- 现有 sync_e2e.rs、dual_client_e2e.rs、dedup_sync_e2e.rs 已覆盖主路径，重构后回归

---

### 2.4 抓取管线（src-tauri/src/ingestion.rs）

**现状量化**：
- 行数：约 600 行（估算）
- 职责数：5 个（HTTP 抓取、feed 解析、条目映射、三段式刷新、favicon 发现）
- 公开函数：15 个
- SQL 依赖：仅通过 db:: 函数，无直接 SQL
- 外部依赖：reqwest、feed-rs、db、sanitize

**处置建议**：**保留** — 职责单一，SQL 边界清晰

**理由**：
1. 职责明确：RSS/Atom 抓取 + 解析，不含业务逻辑
2. 已遵守 SQL 边界：全部通过 db::upsert_article_with_feed 等函数，无散布 SQL
3. 不做理由：feed-rs 是标准 RSS 解析库，替换无收益；三段式管线（锁外 HTTP + 锁内写库）是并发架构的核心设计

**目标边界**：
- 维持现状：HTTP + 解析独立于业务
- 确保新增功能仍遵守 SQL 边界

**依赖风险**：无

---

### 2.5 其他后端模块

| 模块 | 行数 | 职责 | SQL 散布 | 处置 | 理由 |
|------|------|------|----------|------|------|
| ai.rs | ~300 | OpenAI 兼容流式调用 | 0 | **保留** | 职责单一，无 SQL |
| extraction.rs | ~200 | Readability 全文提取 | 0 | **保留** | 独立能力，OPT-002 必需 |
| sanitize.rs | ~400 | HTML 清洗（ammonia） | 0 | **保留** | 安全边界，不可删 |
| scheduler.rs | ~300 | 后台刷新调度 | 0 | **保留** | 通过 db:: 读源列表，无散布 SQL |
| config_sync.rs | ~400 | Gist/WebDAV 配置同步 | 0 | **保留** | OPT-004 核心，遵守 SQL 边界 |
| github_auth.rs | ~250 | GitHub 设备流 | 0 | **保留** | OPT-004 依赖，独立模块 |
| opml.rs | ~200 | OPML 导入导出 | 0 | **保留** | OPT-003 必需 |
| credentials.rs | ~150 | Windows DPAPI 加密 | 0 | **保留** | 安全边界，不可删 |
| media.rs | ~200 | Windows SMTC 媒体控制 | 0 | **保留** | OPT-005 播客必需 |
| greader.rs | ~500 | Google Reader 协议客户端 | 0 | **保留** | REQ-SYNC-001 核心 |
| fever.rs | ~400 | Fever 协议客户端 | 0 | **保留** | REQ-SYNC-001 核心 |

**共同特征**：全部模块已遵守 SQL 边界（通过 db:: 函数）或无 SQL 依赖，职责单一，保留。

---

## 三、测试覆盖现状

**测试文件统计**（src-tauri/tests/）：
- 文件数：19 个
- 主要覆盖：迁移、同步（多协议）、AI、抓取、去重、调度、GitHub 流、回归
- **全局状态依赖**：低 — 各测试用独立临时 DB（TASK-002 已隔离 TEMP/TMP），无跨进程碰撞

**ISSUE-009 相关发现**：
- 测试对 Tauri/全局状态依赖低，大部分是单元测试或 mock E2E
- 无需重构测试架构，只需同步更新 import 路径（db:: 函数拆分后）

---

## 四、代表性重构试点

### 试点选择：SQL 边界收敛（db.rs）

**范围**（精确到文件/函数）：
1. **目标**：commands.rs 的 13 处 SQL 全部抽取为 db:: 函数
2. **具体函数**：
   - `add_feed` L187-199 的未分类 folder 查询/创建 → `db::ensure_uncategorized_folder()`
   - `update_feed` L276-283 的 folder 存在性校验 → `db::folder_exists(id)`
   - `mark_all_read` L446-475 的动态 SQL 构建 → `db::mark_all_read_scoped(feed_id, folder_id)` + 预查询未读 id 列表 → `db::list_unread_ids(feed_id, folder_id)`
3. **不改动**：sync.rs 的 17 处 SQL（留给后续 TASK）
4. **验收标准**：
   - commands.rs 中 `grep -E 'execute\(|query_row\(|prepare\('` 返回 0（全部移除）
   - 新增的 db:: 函数有对应单元测试
   - 现有 95 项 Rust 测试全部通过（回归保护）
   - cargo clippy 无新增警告

**为何选此试点**：
1. **范围最小**：只改 commands.rs（1370 行）的 13 处 SQL，约 100 行代码量
2. **可测**：现有测试已覆盖 commands 的主路径（feed_edit_e2e、regression_e2e），自动验证
3. **代表目标边界**：SQL 边界收敛是 ISSUE-008 的核心解决模式，此试点验证可行性
4. **独立**：不依赖 store.ts 拆分或 sync.rs 重构，可单独交付

**验证计划**（自动化）：
```bash
# 静态检查：commands.rs 无直接 SQL
rg -t rust 'execute\(|query_row\(|prepare\(' src-tauri/src/commands.rs
# 预期：无匹配（exit code 1）

# 单元测试：新增 db 函数覆盖
cargo test --lib db::ensure_uncategorized_folder
cargo test --lib db::folder_exists
cargo test --lib db::mark_all_read_scoped
cargo test --lib db::list_unread_ids

# 回归测试：全部通过
cargo test --release

# Lint：无新增警告
cargo clippy -- -D warnings
```

---

## 五、其他候选审查区域的结论

### 5.1 同步领域与协议类型耦合

**现状**：中等耦合，Backend 枚举已封装大部分差异

**评估**：
- Google Reader 与 Fever 的共享逻辑（merge_pulled_entry、状态对账）已抽取
- 协议特定逻辑（item_ids 分页 vs since_id 分页）隔离在 pull_entries_greader/pull_entries_fever
- **不需要单独试点**：协议差异是业务本质，过度抽象会降低可读性

**建议**：维持现状，若新增协议（如 Atom Pub）再评估共享接口

---

### 5.2 测试对全局状态依赖

**现状**：依赖低，已隔离

**评估**：
- TASK-002 已通过 TEMP/TMP 隔离解决跨进程碰撞（ISSUE-011）
- 各测试用独立临时 DB，无共享全局状态
- **不需要单独试点**：当前测试架构健康

**建议**：维持现状，新增测试继续遵守独立 DB 原则

---

## 六、风险与依赖

### 6.1 不缩减功能的保证

所有处置建议已对照 FEATURES.md：
- **核心**（REQ-LAYOUT-001、REQ-SYNC-001、REQ-AI-001/002）：保留，通过重构不影响
- **OPT-001～010**：全部保留，处置为"整理"或"保留"，无"删除"或"替换"

### 6.2 回归风险

| 风险点 | 缓解措施 | 责任方 |
|--------|----------|--------|
| SQL 迁移语义变化 | 95 项 Rust 测试 + 单元测试覆盖 | TASK-017 执行器 |
| store 拆分 import 路径 | TypeScript 编译检查 + 前端 8/8 测试 | TASK-017 执行器 |
| 并发竞态（锁纪律） | 现有三段式模式不变，代码审查 | 管理 agent 审查 |

### 6.3 预算与时间

- **试点**（SQL 边界）：预估 1 任务（3 轮修复预算内）
- **store.ts 拆分**：预估 2 任务
- **db.rs 全量收敛**：预估 2 任务（含 sync.rs 的 17 处 SQL）
- **总计**：5 任务，按 EXECUTION-POLICY 每批最多 3 任务，需 2 批次

---

## 七、后续任务建议

**TASK-017（试点）**：SQL 边界收敛 — commands.rs
- 范围：commands.rs 的 13 处 SQL 移至 db.rs
- 验收：上述自动化验证计划全部通过
- 预算：3 轮修复、90 分钟

**TASK-018**：store.ts 按领域拆分 slice
- 范围：拆分为 store/navigation.ts 等 6 个文件
- 验收：前端 8/8 测试通过 + TypeScript 编译无错误

**TASK-019**：db.rs 全量 SQL 收敛 + 内部模块化
- 范围：sync.rs 的 17 处 SQL 移至 db/sync.rs，db.rs 拆分为 db/mod.rs + 子模块
- 验收：`rg 'execute\(|query_row\(' src-tauri/src/{commands,sync}.rs` 返回 0

**TASK-020**：sync.rs 内部按 Push/Pull/协议分层
- 前置：TASK-019 完成（SQL 已移除）
- 范围：拆分为 sync/push.rs、sync/pull.rs 等

**TASK-021**：commands.rs 按领域拆分命令组
- 前置：TASK-017 完成（SQL 已移除）
- 范围：拆分为 commands/folders.rs、commands/articles.rs 等

---

## 八、决策依据与可追溯性

### 代码证据来源

所有行数、职责数、SQL 散布点均来自：
- 实际文件读取（Read 工具）
- 模式计数（Grep 工具，如 `execute\(|query_row\(` 匹配 SQL）
- 手工职责清单（基于函数签名与注释）

### 架构约束

- ARCHITECTURE.md：不预先决定整体重写或更换技术栈
- FEATURES.md：核心 + OPT-001～010 全部保留
- ISSUE-008/009：SQL 边界与职责聚合是首要改进方向

### 最强反对论据（不做/复用）

每个"保留"或"整理"建议都包含"不做理由"，如：
- store.ts：不替换 Zustand 为 Redux（成本高、收益不明确）
- db.rs：不引入 ORM（SQLite 个人场景下裸 SQL 更直接）
- sync.rs：不删除同步能力（核心功能，FEATURES 明确保留）

---

## 附录：模块规模总览

| 模块 | 行数 | 职责数 | SQL 散布 | 处置 |
|------|------|--------|----------|------|
| **前端** |
| store.ts | 1494 | 22+ | N/A | 整理（拆 slice） |
| api.ts | 569 | 6 | N/A | 保留 |
| selectors.ts | 192 | 8 | N/A | 保留 |
| components/* | ~2000 | 15 组件 | N/A | 保留 |
| **后端** |
| commands.rs | 1370 | 10+ | **13** | 整理（移 SQL + 拆命令组） |
| db.rs | 2227 | 12 | 70% 集中 | 整理（收敛 SQL + 内部模块化） |
| sync.rs | 1265 | 7 | **17** | 整理（移 SQL + 分层） |
| ingestion.rs | ~600 | 5 | 0 | 保留 |
| 其他 10 模块 | ~3000 | 各 1-3 | 0 | 保留 |

**总计**：
- 前端：~4255 行（主要 3 文件）
- 后端：~9862 行（主要 3 文件 + 10 辅助模块）
- **SQL 散布关键指标**：commands 13 处 + sync 17 处 = 30 处需收敛

---

## 结论

M2 模块处置方案已完成。核心发现：
1. SQL 边界（ISSUE-008）是首要改进方向，30 处散布 SQL 需收敛至 db.rs
2. 职责聚合（ISSUE-009）集中在 store.ts 和 commands.rs，需拆分而非重写
3. 试点（SQL 边界收敛）范围最小、可验证、代表目标模式，建议 TASK-017 实施

所有建议保留 FEATURES.md 已确认的核心与 OPT-001～010 能力。
