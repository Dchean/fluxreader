# 未做事项与已知限制

这份清单收的是**当前仍然成立**的未完成项与已知边界，按「是否影响用户」分层。已收口的项目不再列在这里（例如 SQL 索引加固已完成：`idx_articles_feed_published`、`idx_articles_read_published`、`idx_articles_published_id`、`idx_sync_queue_article`、`idx_sync_queue_created` 已在迁移里建立）。

## 一、用户可见的缺口

| 缺口 | 现状 | 影响 |
| --- | --- | --- |
| 关于页「检查更新」必然失败 | `src/components/settings/AboutTab.tsx` 仍 `fetch('https://api.github.com/...')`，而 `src-tauri/tauri.conf.json` 的 `connect-src` 只允许 `'self' ipc: http://ipc.localhost` | 用户只看到「检查更新失败，请稍后重试」，网络层没有请求发出 |
| `refreshInterval` 值域前后端不一致 | 前端校验 `[5, 120]`（`src/store/settingsValidation.ts`），Rust 接受 `5..=720`（`src-tauri/src/scheduler.rs`） | 经配置同步或手改库写入越界值时「设置保存了但下次打开变回默认」 |
| 「滚动出列表即标为已读」的前置条件未声明 | 只在「显示: 未读」模式下生效（`Timeline.tsx` 里的 `if (timelineFilter !== 'unread') return;`），设置页与文案都没说 | 用户以为这个开关一直有效 |
| 分类级 AI 开关是死设置 | `src/store/selectors.ts` 只读 `binding.feed.autoSummary`，从不读 `binding.cat`；落库与回显正常但零消费 | 用户开了分类级开关没有效果；需要产品口径（继承语义 or 明确「feed 级优先」） |
| 配置同步的订阅/分类删除语义未实现 | `src-tauri/src/config_sync.rs` 里没有任何 `DELETE FROM` / `delete_folder` / `delete_feed`；`commands/folders.rs::delete_folder` 只调 `record_folder_delete`，不调 `unsubscribe_remote` | 「本地删除 ≠ 远端删除」两个缺口都还开着 |
| `closeAsk` 未进 `OVERLAY_SOURCES` | 清单仍是 8 项（`src/components/shortcutYield.ts`），不含确认框；回归网以 `length === 8` 等号钉死 | 确认框打开时单键快捷键仍会生效。注意：补进清单这个正确修复会让门禁失败，需同时改断言 |
| `articles_fts` 与三个触发器零消费 | 搜索已明确不用 FTS5 `MATCH`（中文分词原因，记录在 `db/articles.rs`），但 `migrations.rs` 仍建表并维护触发器 | 无用户可见影响；占用迁移与触发器维护面 |
| 封面覆盖缺口 | miniflux 源文章不参与封面补全（后端只取 `source='direct'`，约 16% 文章无封面）；`first_image` 无尺寸/角色启发式，会把头像、emoji、favicon 当封面 | 部分文章无封面或封面不正确 |
| `--text-tertiary` 深色下对比度约 4:1 | 处于 WCAG AA 边缘，属于全局视觉 token | 低视力用户在深色模式下阅读辅助文字吃力 |

## 二、代码卫生与瞬态问题

- **`loadMoreArticles` 不被「reload 在途」拦截**：布局/范围切换的缓存未命中窗口内，续拉可能把新口径的一页追加到旧口径 entries 后。有 `paginationStale` 与代际守卫兜底，属瞬态错排。
- **`reader.ts` 详情成功路径不清 `hydrationErrors[id]`**：滞留的错误在分支重排后不可见，纯卫生问题。
- **`sync/entries.rs` 仍有一处 `let _ =`**：`backfill_article_content` 的调用错误被吞掉（同族的其余吞错已按发现清理）。
- **`scheduler.rs` 封面回填仍 `unwrap_or(0)`**：DB 写失败被吞成「无变化」，与 `conflict_policy` 那一族已修的吞错同类。
- **`toast` 上限会挤掉带重试项**：`src/store/slices/ui.ts` 的 `.slice(-4)` 在连续失败时会先丢掉最早（往往带重试入口）的那条。
- **重复实现**：`numericId`（`store/selectors.ts`）与 `scopeNumericId`（`store/internals.ts`）仍是两份逐字相同的实现。
- **只定义不用**：`greader.rs` 的 `KEPT_UNREAD`、`client_login_url` 各只剩定义行；`close-resolved` 事件只 emit 无 listen；`feeds.origin_was_local` 无生产写入方；`sync/phases.rs::sync_now` 无生产调用方（前端封装 `api.syncNow` 已删）。

## 三、流程与基建

- **`ci.yml` 没有 tag 触发**：release 门禁要求「tagged commit 上 CI 全绿」，而 CI 只在 main/dev 的 push 与 PR→main 上跑。tag 指向非 main tip 的提交时会空等 45 分钟超时。修法二选一：给 `ci.yml` 加 `push: tags: ["v*"]`，或让门禁在找不到运行时用 `workflow_dispatch` 补发。
- **CI 覆盖缺口**：`ci.yml` 只跑 6 个受约束的 mock 集成套件；`ingestion_e2e`、`scheduler_e2e` 这两个只需本地 `http.server` 的用例仍未纳入。
- **回归网按领域拆分**：`tools/frontend-regression.mjs` 已约 7600 行、集中在一个文件里。拆分是候选微任务。
- **真实挂载组件测试通道**：纯函数与 SSR 无法覆盖 effect 生命周期（React #185 那次就是这样漏掉的）。评估涉及 devDependency 决策，尚未开始。
- **测量电池补 `componentStack` 捕获**：错误钩子目前只记 `appHealth.crashes`，不落盘 `console.error` 的次参数，导致 NotifCard #185 复发仍不可归因。
- **未修的用户可见文案**：`tools/phase4_checklist.md`（存档分支）记录了几处真机观察项，未逐条处置。

## 四、观察项

- **NotifCard #185 待观察**：浏览器 mock 模式下点「通知」布局曾崩溃渲染树（`getSnapshot should be cached` → React #185，报错组件 `<NotifCard>`）。发生在一次测量烟雾中，`appHealth.crashes` 两条、无组件栈。后续复现尝试全部阴性（完整电池两次、快速五布局切换六轮、GC + 布局循环、阅读器打开/AI 按钮/Escape、命令面板输入，均 0 错误）；`src/` 下 17 处非平凡订阅已逐一核查（均为标量或稳定引用）。真机 Tauri 下是否复现尚未验证。详见 [docs/performance.md](performance.md)。

## 五、已评估并明确不修的边界

这两条**不是遗漏**，改动它们需要新的决策：

- **流式翻译的 delta 不经过 sanitizer**。逐 delta 过 sanitizer 可能切断跨 delta 的 HTML 标签，破坏流式渲染。行为折中是「流式期间按纯文本渲染，`done` 后切 HTML」，靠 CSP `script-src 'self'` 作为纵深防御。**CSP 一旦放宽，这个缺口立刻变成真实漏洞。**
- **`get_setting` 对敏感键解密后可经通用 IPC 出 webview**。加密（DPAPI）保护的是磁盘上的静态数据，不是 webview 运行时；改法需要给敏感键单独开命令并做调用方校验，收益与改动量不成比例。

## 六、未实现的路线项

- **「同一篇内容」与「各订阅源条目」分开建模**（核心一致性重构路线第四块）：未实现。因此「跨源同文只广播 read 不广播 star」这类问题会持续存在，同文副本的读状态传播政策也仍是未经产品确认的默认（见 [docs/sync-compat-matrix.md](sync-compat-matrix.md) §3）。
- **Fever `max_id` 历史回溯**：协议与当前 Miniflux 服务端都支持，本客户端未实现；首同步仅最近 50 条作种子。见兼容矩阵 L2。
- **服务端 × 版本兼容实测**：兼容矩阵的「已验证」列目前只覆盖测试替身 `mock_greader.rs`，Miniflux / FreshRSS 的具体部署版本未逐一实机验收。
- **反向索引**：`src/` 与 `src-tauri/src/` 的注释里保留了历史上的工作流编号（`TASK-xxx`、`REQ-0xx`）与界面契约编号。这些编号离开工作流后不可反查；相关结论已按主题整理进 `.agents/notes/`，但注释里的编号未逐处改写。

## 七、历史材料的位置

- **存档分支 `archive/tooling-and-reports`**：一次性探针与测量工具、整理前的调研报告原文（审计、决定、发现、界面契约、自检）、以及这些报告引用的原始测量数据与审查包。
- **`BASELINE.md` 里的提交号对照表**（在上述存档分支的 `archive/reports/`）：2026-09-16 对仓库历史做过一次全量重写，旧提交号在本仓库已无法解析，该表是按提交信息逐条核对得出的唯一回溯线索。
