# TASK-100 UI 取证报告（U1-U8 · 独立审查）

- 审查人：TASK-100 独立审查 worker（未参与实现，全新上下文）
- 日期：2026-09-29
- 取证方式：tmp/audit-r3/harness（Chrome headless CDP + 忠实假后端 mock_backend.py，注入 `window.__TAURI_INTERNALS__` shim，前端零改动走真实 IPC 代码路径）；浅色/深色各一套截图 + DOM 断言采集
- 被测产物：仓库工作区候选（candidate_digest 15bd751d…，dist/ 由 `npm run build` 现场重建后取证）
- 原始采集数据：tmp/task-100/review/t100-ui-results.json、t100-ui2-results.json、t100-ui3-results.json、t100-ui5-results.json
- 截图：TASK-100-ui-dark.png（深色，默认 fixture themeMode=dark）、TASK-100-ui-light.png（浅色，patchSettings themeMode=light 后重载）

## 逐条结论

### U1.shortcut-hint-format — PASS
- DOM 采集：侧栏搜索入口 kbd-tag = `Ctrl+K`、设置中心入口 = `Ctrl+,`；搜索浮层 kbd-tag 序列含 `Esc`（页脚 `Esc 关闭`），无 `ESC 关闭`、无小写 `<kbd>esc</kbd>`。
- 快捷键设置页行文本逐行采集：`Ctrl+K 打开全局搜索 / Ctrl+, 打开设置中心 / J / K 上下切换文章·文章布局 / Space 播放·暂停（播放器激活时）·播放器 / S / M / Esc`。禁止形态 `Ctrl + K`、`Ctrl + ,` 全文不存在。
- 浅色主题下侧栏两处提示复测同值。

### U2.add-feed-wording — PASS
- 右键菜单（全局菜单，真实 contextmenu 事件触发）条目采集：`添加订阅源 / 刷新全部订阅源 / 搜索 / 设置`——`新建订阅源` 不存在。
- 设置页「订阅」页签（FeedsTab）：5 个分类的添加按钮文本全部为 `添加订阅源`（title=在该分类下添加订阅源），面板文本无「新建订阅源」「添加源」。

### U3.star-visual — PASS
- ArticleCard 页脚：收藏视图下 `.card-starred-flag` 8 处，全部含 `<svg>`（Icons.starFilled，`title="已收藏"`），无 `★ 已收藏` 文字形态（深色截图中页脚金色星标即该 SVG）。
- SocialCard：12 张卡 12 个收藏按钮全部 `svg`（收藏态 starFilled），卡内无 `★`/`☆` 字符。
- GalleryCard：5 张卡 5 个收藏按钮全部 `svg`，无 `★`/`☆` 字符。

### U4.player-icons — PASS
- 播放迷你条（真实点击播客卡播放钮激活）：6 控件 = 后退15(svg)/播放暂停(svg)/快进30(svg)/展开全屏(svg)/倍速(纯文本 1.0x)/关闭(svg)，`/[⏸▶✕↺↻⛶]/` 全文零命中。
- 全屏播放器（`.player-full-overlay.open`，z-index 140）：后退15/播放/快进30 均 svg，收起与关闭播放器为文字按钮（非字符图标），字符图标零命中。
- AppearanceTab 主题三按钮：3/3 含 svg（sun/moon/monitor），label `浅色模式/深色模式/跟随系统`，emoji（☀️🌙💻）零命中。
- currentColor：外观页 svg computed color = 按钮色（深色 rgb(241,243,247)）；播放器 svg 深色 rgb(149,159,174) / 浅色 rgb(87,98,116)——随主题前景变化，无硬编码色相。

### U5.truncate-title — PASS
- 文章卡：card-title 5/5、card-snippet 5/5 `title` 属性存在且 === textContent（未截断全文）。
- 画廊卡：gallery-title 5/5 存在且相等（样本：脑吧评测室 #3002 等）。
- 播客卡：podcast-title 5/5 存在且相等。

### U6.favicon-fallback — PASS
- 注入：将可见源（feed id=10「少数派」）favicon_url 指向假后端 404 路径后重载。结果：`img.feed-favicon` 消失、`.feed-favicon-fallback` dot 占位出现且内含 svg——失败回退与「无 favicon」同形态，行首不留空槽。
- 对照组（无 favicon 源「小众软件」）：同为 `.feed-favicon-fallback` dot，形态一致。
- 说明：fixture 各分类默认折叠且侧栏按内容布局过滤分类（image 布局的「豆瓣小组」在文章布局下不显示），故取证对象选文章布局下的可见源。

### U7.close-ask-esc — PASS
- CloseAskDialog 可 Esc 关：`window.__fire('close-ask')` → `.modal-overlay.open`（z-index 150）内出现「关闭 FluxReader」弹窗；按 Esc 后弹窗消失且 IPC 日志出现 `resolve_close`。
- 关序与视觉层级一致（真机序列取证）：全屏播放器（z-index 140 实测）+ 关闭询问（z-index 150 实测）同时打开 → Esc 第 1 次只关 closeAsk（弹窗消失、播放器仍在、resolve_close 已调用）→ Esc 第 2 次关播放器。
- 确认框（FeedsTab 删除源触发）：`.confirm-overlay.open` z-index 实测 3000 → Esc 只关确认框、设置弹窗（150）仍开——3000 > 150 > 140 阶梯与 Esc 关序一致（确认框为捕获级 Esc 并 stopPropagation，App.tsx Esc 链首支为 closeAskVisible）。

### U8.theme-color — PASS
- `meta[name=theme-color]` content = `#14161a`（两主题下恒定）。
- 深色：`data-theme=dark`、`--bg-base` computed = `#14161a` —— 与 meta 逐字一致，启动不闪色。
- 浅色：`data-theme=light`、`--bg-base` = `#f0f3f7`（浅色盘底色，meta 恒为深色值属契约预期：U8 只约束与深色 --bg-base 一致）。

## 汇总

| 检查 | 结论 |
| --- | --- |
| U1.shortcut-hint-format | PASS |
| U2.add-feed-wording | PASS |
| U3.star-visual | PASS |
| U4.player-icons | PASS |
| U5.truncate-title | PASS |
| U6.favicon-fallback | PASS |
| U7.close-ask-esc | PASS |
| U8.theme-color | PASS |

8/8 PASS，无阻塞项。
