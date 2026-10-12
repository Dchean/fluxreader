# 未做事项与已知限制

这份清单收的是**当前仍然成立**的未完成项与已知边界，按「是否影响用户」分层。已收口的项目不再列在这里（例如 SQL 索引加固已完成：`idx_articles_feed_published`、`idx_articles_read_published`、`idx_articles_published_id`、`idx_sync_queue_article`、`idx_sync_queue_created` 已在迁移里建立）。

本轮进展与逐项证据见 [优化线路](optimization/README.md)。已提交且通过 CI 的 outbox 身份、正文净化、Google Reader 适配、源码/浮层、凭据/更新检查、Fever有界历史恢复和受控HTTP CI不再列为未修；其他任务未验收前保留，不凭卡片计划关闭。

## 一、用户可见的缺口

| 缺口 | 现状 | 影响 |
| --- | --- | --- |
| 「滚动出列表即标为已读」的前置条件未声明 | 只在「显示: 未读」模式下生效（`Timeline.tsx` 里的 `if (timelineFilter !== 'unread') return;`），设置页与文案都没说 | 用户以为这个开关一直有效 |
| 配置同步的订阅/分类删除语义未实现 | `src-tauri/src/config_sync.rs` 里没有任何 `DELETE FROM` / `delete_folder` / `delete_feed`；`commands/folders.rs::delete_folder` 只调 `record_folder_delete`，不调 `unsubscribe_remote` | 「本地删除 ≠ 远端删除」两个缺口都还开着 |
| `articles_fts` 与三个触发器零消费 | 搜索已明确不用 FTS5 `MATCH`（中文分词原因，记录在 `db/articles.rs`），但 `migrations.rs` 仍建表并维护触发器 | 无用户可见影响；占用迁移与触发器维护面 |
| 封面覆盖缺口 | miniflux 源文章不参与封面补全（后端只取 `source='direct'`，约 16% 文章无封面）；`first_image` 无尺寸/角色启发式，会把头像、emoji、favicon 当封面 | 部分文章无封面或封面不正确 |
| `--text-tertiary` 深色下对比度约 4:1 | 处于 WCAG AA 边缘，属于全局视觉 token | 低视力用户在深色模式下阅读辅助文字吃力 |

## 二、代码卫生与瞬态问题

- **`reader.ts` 详情成功路径不清 `hydrationErrors[id]`**：滞留的错误在分支重排后不可见，纯卫生问题。
- **`scheduler.rs` 封面回填仍 `unwrap_or(0)`**：DB 写失败被吞成「无变化」，与 `conflict_policy` 那一族已修的吞错同类。
- **`toast` 上限会挤掉带重试项**：`src/store/slices/ui.ts` 的 `.slice(-4)` 在连续失败时会先丢掉最早（往往带重试入口）的那条。
- **重复实现**：`numericId`（`store/selectors.ts`）与 `scopeNumericId`（`store/internals.ts`）仍是两份逐字相同的实现。
- **只定义不用**：`greader.rs` 的 `KEPT_UNREAD`、`client_login_url` 各只剩定义行；`close-resolved` 事件只 emit 无 listen；`feeds.origin_was_local` 无生产写入方；`sync/phases.rs::sync_now` 无生产调用方（前端封装 `api.syncNow` 已删）。

## 三、流程与基建

- **测量电池补 `componentStack` 捕获**：错误钩子目前只记 `appHealth.crashes`，不落盘 `console.error` 的次参数，导致 NotifCard #185 复发仍不可归因。
- **未修的用户可见文案**：`tools/phase4_checklist.md`（存档分支）记录了几处真机观察项，未逐条处置。

## 四、观察项

- **NotifCard #185 待观察**：浏览器 mock 模式下点「通知」布局曾崩溃渲染树（`getSnapshot should be cached` → React #185，报错组件 `<NotifCard>`）。发生在一次测量烟雾中，`appHealth.crashes` 两条、无组件栈。后续复现尝试全部阴性（完整电池两次、快速五布局切换六轮、GC + 布局循环、阅读器打开/AI 按钮/Escape、命令面板输入，均 0 错误）；`src/` 下 17 处非平凡订阅已逐一核查（均为标量或稳定引用）。真机 Tauri 下是否复现尚未验证。详见 [docs/performance.md](performance.md)。

## 五、已评估并明确不修的边界

这两条**不是遗漏**，改动它们需要新的决策：

- **流式翻译的 delta 不经过 sanitizer**。逐 delta 过 sanitizer 可能切断跨 delta 的 HTML 标签，破坏流式渲染。行为折中是「流式期间按纯文本渲染，`done` 后切 HTML」，靠 CSP `script-src 'self'` 作为纵深防御。当前原始 delta 使用 React 纯文本插值，完成后回读净化版；不能把“未逐段清洗”单独等同 XSS，也不能据此放宽 CSP。
- **`get_setting` 对敏感键解密后可经通用 IPC 出 webview**。加密（DPAPI）保护的是磁盘上的静态数据，不是 webview 运行时；改法需要给敏感键单独开命令并做调用方校验，收益与改动量不成比例。

## 六、未实现的路线项

- **「同一篇内容」与「各订阅源条目」分开建模**（核心一致性重构路线第四块）：未实现。因此「跨源同文只广播 read 不广播 star」这类问题会持续存在，同文副本的读状态传播政策也仍是未经产品确认的默认（见 [docs/sync-compat-matrix.md](sync-compat-matrix.md) §3）。
- **服务端 × 版本兼容实测**：兼容矩阵的「已验证」列目前只覆盖测试替身 `mock_greader.rs`，Miniflux / FreshRSS 的具体部署版本未逐一实机验收。
- **反向索引**：`src/` 与 `src-tauri/src/` 的注释里保留了历史上的工作流编号（`TASK-xxx`、`REQ-0xx`）与界面契约编号。这些编号离开工作流后不可反查；相关结论已按主题整理进 `.agents/notes/`，但注释里的编号未逐处改写。

## 七、历史材料的位置

- **存档分支 `archive/tooling-and-reports`**：一次性探针与测量工具、整理前的调研报告原文（审计、决定、发现、界面契约、自检）、以及这些报告引用的原始测量数据与审查包。
- **`BASELINE.md` 里的提交号对照表**（在上述存档分支的 `archive/reports/`）：2026-09-16 对仓库历史做过一次全量重写，旧提交号在本仓库已无法解析，该表是按提交信息逐条核对得出的唯一回溯线索。
