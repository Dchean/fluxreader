# FluxReader

本地优先（Local-First）的 Windows 桌面 RSS 阅读器。Tauri 2 + Rust 后端 + React 19 前端，全部数据存于本机 SQLite，直连抓取订阅源，可选连接 Miniflux 服务端做双向同步，并支持 GitHub Gist / WebDAV 跨设备同步阅读状态。基于 [Papr](https://github.com/l0ng-ai/papr)（MIT）二次开发。

> 本项目代码由纯 AI 古法生成——没有一滴人工代码，人工仅负责需求定义与验收。

## 功能特性

- **五种内容布局**：文章 / 社交 / 画廊 / 播客 / 通知，按订阅源或分类绑定布局
- **直连抓取 + Miniflux 双轨**：本机直连源站（第一优先级），连接 Miniflux 后双向同步订阅关系与已读/收藏状态；「本机抓取 / 跟随服务端」两种同步模式
- **跨设备状态同步**：GitHub Gist / WebDAV 同步已读/收藏状态（OR-合并，多端对齐）
- **AI 增强**：OpenAI 兼容端点（DeepSeek/OpenAI/GLM 预设 + 自定义）流式摘要与翻译，结果缓存本地
- **全文提取**：dom_smoothie Readability 智能全文 + og:image 首图，摘要型源自动提取
- **虚拟滚动**：@tanstack/react-virtual，海量文章流畅滚动；可拖动调整列表/正文分栏宽度
- **自定义右键菜单**：按上下文（文章卡片/订阅源/正文/空白）提供收藏/已读/复制/编辑/删除等操作
- **图片防盗链代理**：少数派、豆瓣等白名单式防盗链图床走后端 fetch 转 data: URL
- **播客播放**：内嵌音频/视频播放 + Windows SMTC 系统媒体控制
- **OPML 导入导出**、全文搜索、键盘流（J/K/S/M）、开机自启、系统托盘、新文章通知

## 技术栈

| 层 | 技术 |
|----|------|
| 应用壳 | Tauri 2.11（无边框窗口、WebView2、tray-icon、log / opener / autostart / window-state / single-instance / notification 插件） |
| 后端 | Rust：rusqlite（bundled SQLite + WAL + FTS5）、feed-rs、ammonia、dom_smoothie、reqwest（rustls）、tokio |
| 前端 | React 19 + TypeScript + Zustand（store 拆分 selector/types 层）+ @tanstack/react-virtual、Vite |
| IPC | Tauri commands + `ipc::Channel`（AI 流式增量推送） |

## 系统结构

```
src-tauri/src/
  db.rs                # 数据层：rusqlite_migration 迁移链 v1→v12、FTS5 外部内容表 + 触发器同步、
                       #   参数化查询、智能去重（跨源 URL 查重）、缓存清理、账号数据隔离
  ingestion.rs         # RSS/Atom 抓取解析（feed-rs）、Conditional GET（etag/last-modified/304）、
                       #   失败退避状态机（fail_count → 5/5/30/120 分钟 next_retry_at）、无时间戳兜底
  scheduler.rs         # 后台刷新（Semaphore 并发抓取）+ Miniflux 自动同步 + 文章状态自动同步
  sync.rs              # Miniflux 双向同步：URL 碰撞合并、sync_queue 出队推送、feeds/states 两阶段、
                       #   全量对账（未读数漂移收敛）、批量匹配映射（消除 N+1）、多源并发拉取
  config_sync.rs       # 配置同步（GitHub Gist / WebDAV）：分类/订阅源/设置的上传下载
  article_state_sync.rs# 文章状态同步（已读/收藏，OR-合并），复用 config_sync 的 Gist/WebDAV 通道
  ai.rs                # OpenAI 兼容 SSE 流式消费：预设端点 + 自定义 baseUrl、逐 delta 抽取、8MiB 上限
  opml.rs              # OPML 解析（tidy 修复裸 &）与构建、按 xml_url 去重
  extraction.rs        # dom_smoothie Readability 全文提取 + og:image 首图（spawn_blocking 隔离）
  sanitize.rs          # ammonia 白名单消毒 + 相对 URL 重写 + 惰性图片恢复 + 正文富媒体放行
  credentials.rs       # 敏感凭据 DPAPI 加密（Windows CryptProtectData，非 Windows 回落明文）
  github_auth.rs       # GitHub 设备流登录（配置同步的网页授权）
  media.rs             # Windows SMTC 系统媒体控制线程（非 Windows 降级 inactive）
  miniflux.rs          # Miniflux REST 客户端（对象/裸数组两种响应、enclosure 附件）
  commands.rs          # 全部 IPC 命令 + 后端抓图（Referer 候选链防盗链兼容）
  state.rs             # AppState：Arc<Mutex<Connection>>（锁不跨 .await）+ 共享 reqwest Client

src/
  store.ts             # 全局状态机：导航/筛选/已读保留快照/播放器/AI 打字机/搜索锚定
  store/selectors.ts   # 派生 selector（selectVisibleEntries 等）+ 布局/视图命名常量
  store/types.ts       # AppState / SettingsState / ToastMessage 类型
  components/          # Sidebar（订阅树+角标+失败源警示）/ Timeline（五布局卡片+虚拟滚动）/
                       #   Reader（顶栏+阅读工具栏+正文灯箱+图片代理）/ PlayerBar / Overlays /
                       #   ContextMenu（全局右键菜单，替换 WebView2 默认菜单）
  lib/api.ts           # invoke 封装：Tauri 环境 → 后端；浏览器环境 → 回退演示数据
  lib/imageProxy.ts    # 图片防盗链代理（HTML 内 img + 单张封面 URL）
  lib/external.ts      # 外链统一拦截：仅放行 http(s) → 系统浏览器
```

## 关键设计

- **单写连接**：`Arc<Mutex<Connection>>` 串行化全部 SQL；HTTP 等待期间不持锁（三段式：锁内读 → 锁外网络 → 锁内写；同步/抓取/推送全部遵循）
- **安全边界**：外部 HTML 入库即消毒（ammonia 白名单 + URL 重写 + iframe 域名白名单降级），前端 `dangerouslySetInnerHTML` 只渲染已消毒内容；AI 翻译产物入库前二次消毒；外链点击只放行 http(s)
- **列表性能**：虚拟滚动（只渲染视口 + overscan 缓冲）、列表快照不含正文（选中时懒加载水合）、搜索锚定（`article_index` 窗口函数定位 + 代际守卫防竞态）
- **已读语义**：打开 / 滚动到底 / 滚出列表三种触发；未读筛选下已读卡片原地变灰（会话级快照），切视图才移除
- **AI 流式**：Rust 消费 SSE → `Channel<AiEvent>` 逐 delta 推前端 → 打字机渲染；完成后写缓存列，重复打开零重算；未配置时源级自动触发静默跳过
- **同步一致性**：Miniflux 批量匹配映射（`sync_match_maps`）消除 N+1 查询；多源并发拉取；read-anywhere-wins 多端已读收敛；文章状态同步用 OR-合并（任一端已读即已读，不覆盖回未读）
- **播放器**：store ↔ 单 `<audio>` 元素双向同步（store→element: play/rate/seek；element→store: timeupdate/metadata/ended）
- **正文媒体**：直连源内嵌 video/audio 可播；YouTube/B 站等白名单 iframe 嵌入播放，其余嵌入内容降级为「在浏览器打开」外链

## 数据模型

SQLite（`%APPDATA%\com.fluxreader.app\fluxreader.db`，WAL）：
- `folders` / `feeds`（layout 绑定、auto_summary/translate 开关、退避字段、miniflux_id 映射、origin 来源标记）
- `articles`（guid 唯一约束、enclosure、AI 产物列、url_norm 去重键、部分索引 `idx_articles_unread`）
- `articles_fts`（FTS5 external content 表，靠触发器与 articles 同步；搜索用 LIKE 子串，此表保留作备用索引）
- `settings`（键值：app_settings JSON / ai_config / miniflux 凭据 / 配置同步凭据）
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
