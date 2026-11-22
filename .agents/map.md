# 模块地图（真实代码现状）

改代码前先在这里定位；契约级「唯一出处」仍以 [docs/contracts.md](../docs/contracts.md) 为准，本篇只回答"东西在哪、谁能复用"。

## 进程与依赖方向

Tauri 2 Windows 桌面应用：React/TS 前端经 IPC 调用 Rust 命令层，Rust 独占 SQLite、HTTP 与系统媒体控制。

```text
src/ (store+components)
  → invoke → src/lib/api.ts（前端唯一 IPC 封装）
    → src-tauri/src/commands/（薄命令层）
      → db/            schema、迁移、领域查询、事务
      → sync/          RSS 服务双向同步（Content/State/Push 三通道）
      → local_sync/    纯本地 WebDAV 持续同步 + 便携导出/恢复
      → ingestion/     直连抓取、解析、分段落库
      → scheduler.rs   定时刷新、封面补全、保留维护
      → ai.rs / ai_segments.rs / sanitize.rs / media.rs / image_requests.rs（单文件模块）
```

## 五个统一收口点（改前必看）

| 事实 | 唯一入口 |
| --- | --- |
| 前端 IPC | `src/lib/api.ts`（内部 `getInvoke`；全 `src/` 无第二处 `invoke(`） |
| 后端命令注册 | `src-tauri/src/lib.rs` 的 `generate_handler![]`（76 条，唯一注册点） |
| 数据库写事务 | `db::write_transaction`（`BEGIN IMMEDIATE`）+ `db/writer.rs` 的 FIFO 准入 |
| 同步冲突方向 | `src-tauri/src/sync/conflict_policy.rs` |
| AI 服务配置 | `src-tauri/src/ai.rs::AiConfig`；命令层 `commands/ai.rs::AiConfigSnapshot` 复用同次解析 |
| 凭据存取 | `src-tauri/src/credentials.rs`（DPAPI，敏感键白名单 `SENSITIVE_KEYS`） |

## 前端 src/

- 入口装配：`src/main.tsx` → `src/App.tsx`（根布局，装配 Sidebar/Timeline/Reader/PlayerBar 与浮层）。
- 状态：`src/store.ts` 只做组合；按领域 slice 在 `src/store/slices/`（ai、bootstrap、feeds、localLibrary、nav、player、reader、settings、sync、ui，键集两两不相交）。
- 状态内核：`src/store/internals.ts`（查询派生、窗口缓存、乐观写入版本与写前沿）、`src/store/bodyCache.ts`（正文 + AI 产物唯一真值源）、`src/store/selectors.ts`、`src/store/types.ts`。
- 纯逻辑/封装 `src/lib/`：`api.ts`（IPC）、`readingPosition.ts`（返回位置锚）、`syncPill.ts`（同步指示器真值表）、`imageProxy.ts`（图片预算）、`copyPolicy.ts`（文案口径）、`external.ts`（外链）、`jkNavigation.ts`、`lru.ts`（通用淘汰循环 `evictOldestEntries`）。
- UI `src/components/`：主结构 `Sidebar` / `Timeline`（虚拟滚动主列表）/ `Reader` / `ReaderProse` / `PlayerBar` / `SettingsModal`；浮层 `Overlays` / `ContextMenu` / `ErrorCenter` / `ErrorBoundary`；布局纯函数 `GalleryLayout` / `scrollAwayRead` / `shortcutYield` / `timelineRefill` / `timelineSentinel` / `anchorScopeNav`；设置页在 `src/components/settings/`。
- 样式 `src/styles/`：`tokens.css` 是设计变量单点，`base.css` 是主样式表。

### 直接复用的公共件

- UI 基件：`src/components/primitives.tsx`（`FluxDropdown`、`ModalOverlay`、`SwitchInline`、`AiModeSelect`、`SettingCard` 等）、`src/components/icons.tsx`。
- hooks：`src/components/use*.ts`（`useFocusTrap`、`useProseImages`、`useImageResource`、`useAutomaticAi`、`useTauriEvent`、`useEnteringClass`）——**没有**独立 `src/hooks/` 目录。
- 工具：`src/lib/`（无 `src/utils/`）。

## 后端 src-tauri/src/

- 命令层 `commands/`：`ai.rs`、`sync.rs`、`folders.rs`、`settings.rs`、`articles.rs`、`opml.rs`、`backup.rs`，经 `commands/mod.rs` 的 `pub use` 汇总。命令层短小：取状态锁 → 调 `db::` 或协调器 → 返回可序列化行。
- 数据层 `db/`：`schema.sql`（权威结构）、`migrations.rs`（格式识别与拒绝）、`articles.rs`（列表/搜索/读藏/计数/清理）、`feeds.rs`/`folders.rs`、`contents.rs`、`ai_segments.rs`、`identity.rs`/`url_norm.rs`、`settings.rs`、`sync_queue.rs`/`sync_map.rs`/`outbox.rs`、`backup.rs`/`body_retention.rs`、`writer.rs`。
- 同步 `sync/`：`runtime.rs`（运行与会话）、`phases.rs`、`greader_pull.rs`、`fever_pull.rs`、`push.rs`、`subscriptions.rs`、`entries.rs`、`session.rs`、`conflict_policy.rs`、`credentials.rs`（`account_key`）。
- 本地同步 `local_sync/`：`logical.rs`（字段寄存器与戳合并）、`runtime.rs`（工作队列/GC 编排）、`database.rs`（`flx_*` SQL 函数）、`storage.rs`、`backup.rs`（便携 v4）、`body_archive.rs`、`fast.rs`、`capture.rs`、`model.rs`。
- 抓取与协议：`ingestion/`（`http.rs`/`parse.rs`/`staged.rs`/`favicon.rs`）、`greader.rs`、`fever.rs`、`webdav_transport.rs`（不依赖应用库的独立传输层）、`sync_http.rs`、`extraction.rs`。
- 其他：`scheduler.rs`、`ai.rs`、`ai_segments.rs`、`sanitize.rs`、`media.rs`、`image_requests.rs`、`credentials.rs`、`state.rs`、`window_startup.rs`、`diagnostics.rs`、`endpoint_resolve.rs`、`navigation.rs`、`connection_suggestion.rs`、`error.rs`、`lib.rs`。

## 专题细节的唯一入口

- 数据库归属、同步边界、前端数据流、并发与写预留 → [docs/architecture.md](../docs/architecture.md)
- 契约单点与改动影响面 → [docs/contracts.md](../docs/contracts.md)
- 协议能力差异与真实服务端证据 → [docs/sync-compat-matrix.md](../docs/sync-compat-matrix.md)
- 备份/便携/WebDAV → [docs/backup.md](../docs/backup.md)
- 性能基线与预算 → [docs/performance.md](../docs/performance.md)
- 前端组件与令牌台账 → [docs/frontend.md](../docs/frontend.md)
- 门禁、测试分层、常见故障 → [docs/development.md](../docs/development.md)
