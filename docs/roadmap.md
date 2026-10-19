# 未做事项与已知限制

这份清单收的是**当前仍然成立**的未完成项与已知边界，按「是否影响用户」分层。已收口的项目不再列在这里（例如 SQL 索引加固已完成：`idx_articles_feed_published`、`idx_articles_read_published`、`idx_articles_published_id`、`idx_sync_queue_article`、`idx_sync_queue_created` 已在迁移里建立）。

本轮进展与逐项证据见 [优化线路](optimization/README.md)。已提交且通过 CI 的 outbox 身份、正文净化、Google Reader 适配、源码/浮层、凭据/更新检查、Fever有界历史恢复和受控HTTP CI不再列为未修；其他任务未验收前保留，不凭卡片计划关闭。

## 一、用户可见的缺口

| 缺口 | 现状 | 影响 |
| --- | --- | --- |
| 「滚动出列表即标为已读」的前置条件未声明 | 只在「显示: 未读」模式下生效（`Timeline.tsx` 里的 `if (timelineFilter !== 'unread') return;`），设置页与文案都没说 | 用户以为这个开关一直有效 |
| 配置同步的镜像删除不产生远端退订意图 | mirror 模式的本地 feed/分类删除已实现（`src-tauri/src/config_sync/plan.rs`）；普通 UI 删除已走 typed outbox 持久退订；镜像删除明确不调用 `enqueue_unsubscribe` | 镜像同步后远端订阅可能残留，需用户手动退订 |
| `articles_fts` 已删除，搜索为 LIKE 权威 + trigram 候选 | v29 已删除旧 FTS 及触发器；当前 `search_articles` 以 LIKE 逐行验证结果，`articles_trigram_idx` 仅对 ≥3 字符词作候选过滤（短词/索引缺失回落纯 LIKE） | 无用户可见影响；候选索引体积与写成本见 [performance.md](performance.md) |
| 封面覆盖缺口 | 服务端正文无图文章的候选排除已由 OPT-017D 移除（候选改为全来源「无封面 + 有 URL」）；`first_image`/og:image 仍无尺寸/角色启发式，会把头像、emoji、favicon 当封面；本地为服务端文章补上的封面不参与失效清理（保守边界） | 部分文章无封面或封面不正确 |
| `--text-tertiary` 深色下对比度约 4:1 | 处于 WCAG AA 边缘，属于全局视觉 token | 低视力用户在深色模式下阅读辅助文字吃力 |

## 二、代码卫生与瞬态问题

- **`reader.ts` 详情成功路径不清 `hydrationErrors[id]`**：滞留的错误在分支重排后不可见，纯卫生问题。
- **`toast` 上限仍会挤掉旧瞬时 toast**：`src/store/slices/ui.ts` 的 `.slice(-4)` 在连续失败时仍会挤掉最旧的瞬时 toast；带重试错误已由 OPT-017E 的错误中心（有界 20 条 + 同文案合并 + 24h TTL，侧栏入口）持久承接，本清单不再把「重试入口丢失」列为缺口。
- **只定义不用**：`feeds.origin_was_local` 无置 1 写入方（`purge_remote_data_in` 里的重置为 no-op），删除列需迁移；`sync/phases.rs::sync_now` 无生产调用方（前端封装 `api.syncNow` 已删；集成测试 `dual_client_e2e` 仍消费）。

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

- **内容/条目分离已实现（v22 `contents` + `articles.content_id`）**：正文单一所有权与最后引用回收已落地。剩余是跨源同文的产品语义——read 广播、star 不广播——尚未由产品最终确认，同文副本的读状态传播政策仍是未经产品确认的默认（见 [docs/sync-compat-matrix.md](sync-compat-matrix.md) §3）。
- **服务端 × 版本兼容实测**：兼容矩阵的「已验证」列覆盖测试替身 `mock_greader.rs` 与 `greader_compat_e2e.rs` / `fever_compat_e2e.rs` 的 Miniflux / FreshRSS 严格夹具，不代表任何具体部署版本；真实服务端未逐一实机验收。
- **存量 identity 惰性收敛**：OPT-008C 的 identity 重算与等价合并只对触达行生效，未触达的旧行不会一次性回填；收敛随读取/写入逐步发生（见 [OPT-008C](optimization/OPT-008C.md)「遗留」）。
- **真实 WebView 性能测量缺口**：OPT-017 的证据是隔离 SQLite / jsdom / 组件回归；真实 WebView 的滚动帧率、图片解码、真实高度分布与同构建端到端前后对照未测（见 [OPT-017](optimization/OPT-017.md) 父卡收口与 [performance.md](performance.md) 的未测边界）。
- **反向索引**：`src/` 与 `src-tauri/src/` 的注释里保留了历史上的工作流编号（`TASK-xxx`、`REQ-0xx`）与界面契约编号。这些编号离开工作流后不可反查；相关结论已按主题整理进 `.agents/notes/`，但注释里的编号未逐处改写。

## 七、历史材料的位置

- **存档分支 `archive/tooling-and-reports`**：一次性探针与测量工具、整理前的调研报告原文（审计、决定、发现、界面契约、自检）、以及这些报告引用的原始测量数据与审查包。
- **`BASELINE.md` 里的提交号对照表**（在上述存档分支的 `archive/reports/`）：2026-09-16 对仓库历史做过一次全量重写，旧提交号在本仓库已无法解析，该表是按提交信息逐条核对得出的唯一回溯线索。
