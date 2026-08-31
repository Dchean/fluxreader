# FluxReader

本地优先（Local-First）的 Windows 桌面 RSS 阅读器。Tauri 2 + Rust 后端 + React 19 前端，全部数据存于本机 SQLite，直连抓取订阅源，可选连接 Miniflux 服务端做双向同步。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发。

> 本项目代码由纯 AI 古法生成——没有一滴人工代码，人工仅负责需求定义与验收。

## 技术栈

| 层 | 技术 |
|----|------|
| 应用壳 | Tauri 2.11（无边框窗口、WebView2、tauri-plugin-log / opener / autostart / window-state） |
| 后端 | Rust：rusqlite（bundled SQLite + WAL + FTS5）、feed-rs、ammonia、dom_smoothie、reqwest（rustls）、tokio |
| 前端 | React 19 + TypeScript + Zustand（单 store + useShallow 派生 selector）、Vite |
| IPC | Tauri commands + `ipc::Channel`（AI 流式增量推送） |

## 系统结构

```
src-tauri/src/
  db.rs          # 数据层：rusqlite_migration 迁移链 v1→v4、FTS5 外部内容表 + 触发器同步、
                 #   全部参数化查询、智能去重（跨源 URL 查重）
  ingestion.rs   # RSS/Atom 抓取解析（feed-rs）、Conditional GET（etag/last-modified/304 短路）、
                 #   失败退避状态机（fail_count → 5/5/30/120 分钟 next_retry_at）
  scheduler.rs   # 后台刷新：60s tick 读 app_settings（间隔/开关实时生效）、
                 #   Semaphore 4 并发抓取、新文章 emit 事件 → 前端重载 + toast
  ai.rs          # OpenAI 兼容 SSE 流式消费：预设端点（DeepSeek/OpenAI/GLM）+ 自定义 baseUrl、
                 #   逐 delta 抽取、错误对象检测、8MiB 缓冲上限
  sync.rs        # Miniflux 双向同步：拉取合并（icon/title 补全、缺失源入库）+
                 #   sync_queue 出队推送（已读/收藏/增删源），离线操作不丢
  opml.rs        # OPML 解析（tidy 修复裸 & 的真实世界导出）与构建、按 xml_url 去重
  extraction.rs  # dom_smoothie Readability 全文提取 + og:image 首图（spawn_blocking，非 Send 隔离）
  sanitize.rs    # ammonia 白名单消毒 + 相对 URL 以 feed base 重写 + 惰性图片恢复
  commands.rs    # 全部 IPC 命令（列表/设置/AI 流式/OPML/同步/全文提取）
  state.rs       # AppState：Arc<Mutex<Connection>>（锁不跨 .await）+ 共享 reqwest Client
  miniflux.rs    # Miniflux REST 客户端（兼容对象/裸数组两种响应形态）

src/
  store.ts       # 全局状态机：导航/筛选/已读保留快照（openedReadIds）/播放器/AI 流式打字机累积
  components/    # Sidebar（订阅树+角标）/ Timeline（五布局卡片流+滚动标读）/
                 #   Reader（懒加载水合+工具栏）/ PlayerBar（单 audio 元素双向同步）/
                 #   Overlays（命令面板式搜索：文章/订阅源/命令三组）/ SettingsModal
  lib/api.ts     # invoke 封装：Tauri 环境 → 后端；浏览器环境 → 自动回退演示数据
  lib/external.ts# 外链统一拦截：仅放行 http(s) → 系统浏览器（opener 插件）
```

## 关键设计

- **单写连接**：`Arc<Mutex<Connection>>` 串行化全部 SQL；HTTP 等待期间不持锁（先快照后网络）
- **安全边界**：外部 HTML 入库即消毒（ammonia 白名单 + URL 重写），前端 `dangerouslySetInnerHTML` 只渲染已消毒内容；外链点击只放行 http(s)
- **列表性能**：列表快照不含正文（`content_html` 选中时懒加载水合）；FTS5 外部内容表靠触发器与 articles 行级同步，搜索覆盖标题/正文/作者/AI 摘要/译文
- **已读语义**：打开/滚动到底/滚出列表三种触发；未读筛选下已读卡片原地变灰（会话级快照），切视图才移除
- **AI 流式**：Rust 消费 SSE → `Channel<AiEvent>` 逐 delta 推前端 → 打字机渲染；完成后写 `ai_summary`/`translated_content` 缓存列，重复打开零重算；未配置时源级自动触发静默跳过
- **播放器**：store ↔ 单 `<audio>` 元素双向同步（store→element: play/rate/seek；element→store: timeupdate/metadata/ended）

## 数据模型

SQLite（`%APPDATA%\com.fluxreader.app\fluxreader.db`，WAL）：
- `folders` / `feeds`（layout 绑定、auto_summary/translate 开关、退避字段、miniflux_id 映射）
- `articles`（guid 唯一约束、enclosure、AI 产物列、部分索引 `idx_articles_unread`）
- `articles_fts`（FTS5 external content，unicode61 分词：中文按字、英文按词）
- `settings`（键值：app_settings JSON / ai_config / miniflux 凭据）
- `sync_queue`（离线变更队列，同步成功后出队）

## 开发

```bash
npm install           # 前端依赖
npm run tauri dev     # 开发模式（Vite 固定 5173 + cargo 热重建）
npm run tauri build   # 生产构建 + 打包
```

环境：Node 20+、Rust stable、Windows 10/11（WebView2）。

## 测试

```bash
cd src-tauri
cargo test                # 单元测试 + 迁移测试
cargo test -- --ignored   # e2e（内置 mock HTTP/Miniflux/AI 服务，真实 TCP listener）

npx tsc --noEmit          # 前端类型检查
```

AI 链路可脱离真实 Key 端到端验证：

```bash
python tools/mock_ai_server.py 8123    # OpenAI 兼容 mock（/v1/models + SSE chat）
# 设置 → AI服务 → 自定义，BaseURL 填 http://127.0.0.1:8123/v1
```

## License

MIT —— 见 [LICENSE](LICENSE)。衍生自 [Papr](https://github.com/l0ng-ai/papr)，保留其版权声明。
