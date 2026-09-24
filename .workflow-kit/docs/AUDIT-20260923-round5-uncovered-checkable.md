# fluxreader 审计第五轮（未覆盖但可检查的部分）

- 基线：`git HEAD = d6234e5`（业务代码零改动；仅 `tmp/` 脚本与 `.workflow-kit/` 文档）
- 授权与口径（用户指令）：**继续审计「未覆盖但能检查」的部分**；模拟数据须与真实数据一致；不跑门禁
- 前序：[第一轮](AUDIT-20260922-shells-and-regressions.md) · [第三轮 真机 DOM](AUDIT-20260922-round3-dom.md) · [第四轮 封面与全功能矩阵](AUDIT-20260923-round4-covers-and-feature-matrix.md)
- 本轮范围：把第四轮 §2.5 列为「未覆盖」的项目里**实际可在浏览器 + 忠实后端下验证**的那些全部跑掉
  （脚本 `tmp/audit-r3/harness/s12…s18`，逐个可重跑）

---

# 一、本轮新增验证（全部通过）

## 1.1 阅读器 / AI / 全文（s12，8/8 PASS）

| 编号 | 检查项 | 证据 |
| --- | --- | --- |
| E1 | 视觉设置真的应用到正文（不只落库） | `.article-prose` computed `fontSize=16px`、`lineHeight=28.8px`（= 设置 180% × 16px）、`fontFamily` 生效 |
| E2 | 预计阅读时间 | byline「…·1 分钟阅读」 |
| E3 | 手动「摘要」→ 流式生成并展示 | IPC `ai_summarize`×1，摘要卡 open 且文本为模型输出 |
| E4 | 手动「翻译」（成功）→ **按 HTML 渲染** | 正文 `html="<p>已消毒译文：第 3004 篇</p>"`、`hasP=true`、无字面标签（对照组） |
| E5 | 「提取全文」 | IPC `extract_fulltext`×1；正文替换为提取内容；按钮文案「提取全文 → RSS 原文」 |
| E6 | 「源码」按钮 | 只加 `raw-render-mode` 类，正文**仍是 HTML 节点**（`<p>` 仍是元素）⇒ round-1 **P3-15「文案承诺显示源码、实际只是换排版」真机确认** |
| E7 | 正文内 sspai 图片代理 | `fetch_image`×1；`<img src="data:image/png;base64,…">` ⇒ `proxyImagesInHtml` 真机验证 |
| E8 | 翻译失败 → 错误可见 | toast「翻译失败：AI 服务未配置或不可达」 |

## 1.2 系统级与插件路径（s13/s14/s15/s17）

| 编号 | 检查项 | 结论与证据 |
| --- | --- | --- |
| F1 | 关闭询问弹窗 → `resolve_close` | 点「最小化到托盘」→ IPC 实参 `{action:'tray', remember:true}`（实测） |
| F2 | 开机自启动开关 | 切换 → `plugin:autostart|enable` IPC×1 且设置镜像落库（false→true） |
| F3 | 自启动**失败**回滚 | 注入 `plugin:autostart|disable` 拒绝 → toast「自启动设置失败」且镜像回滚到原值（不留与注册表不符的值） |
| F4 | 新文章通知开关 | 前端只写设置（`notifyOnNewArticles` 落库），**权限申请在 Rust 侧**，浏览器无法验证（如实登记） |
| **F5** | 关于页「检查更新」 | **round-1 的 CSP 疑点已确证为事实**：页面确实发起 `fetch('https://api.github.com/…')`，Chrome 报 `violates the following Content Security Policy directive: "connect-src 'self' ipc…"`，网络层无请求 ⇒ **该功能在真实应用中必然失败**，用户只会看到「检查更新失败，请稍后重试」。CSP 字符串逐字取自 `tauri.conf.json:28` |
| F6 | OPML **导入**（真实文件） | 用 CDP 给 `<input type=file>` 塞真实 `.opml` → 真实 change 事件 → IPC `opml_import`×1 → feeds 16→18 + toast「OPML 导入完成：新增 2 个源」 |
| J1 | 音频**真实播放** | 后端提供真实 WAV（3s）→ 真实鼠标点击后 `<audio>` 播到结束（`currentTime 2.48→3.00`）；**注意**：合成的 `.click()` 会被浏览器 autoplay 策略拦住而不播放（harness 局限，非产品缺陷——用 CDP `Input.dispatchMouseEvent` 的真实手势即可播放） |
| I3/I4 | 倍速与暂停 | `playbackRate 1.0→1.25`；暂停 `paused=true` |
| I2 | SMTC 前端侧 | `media_update_full` 被调用（实参含 title/show/durationSec/positionSec/playing）——**系统侧效果仍未验证** |
| I5 | Space（播放器激活时） | 真实按键 → 播放/暂停翻转 |
| I6 | M 键 | `is_read 1→0`（切换生效） |

## 1.3 两个「设置项」的真实前置条件（本轮新证据）

- **「滚动出列表区域时标为已读」只在「显示: 未读」模式下生效**（s18，对照组+实验组各一次）：
  - 显示=全部：滚到底（scrollTop=1128，max=1128）→ 首卡 `is_read 0→0`（不标读）
  - 显示=未读：同样的滚动 → 首卡 `is_read 0→1`（标读）
  - 代码依据：`Timeline.tsx:142-143` 的 `if (timelineFilter !== 'unread') return;` ⇒ 与 round-1 route-S 的 **S-2** 结论一致：**前置条件从未在 UI/设置说明里声明**，用户以为这个开关一直有效。
- **设置区间对照**（我自己核对源码）：前端 `SETTINGS_VALIDATORS`（`store/settingsValidation.ts`）覆盖全部 24 键、类型绑定 `SettingsState`（漏写键 tsc 直接报错，写读两条路径共用）；其中
  - `refreshInterval`：前端 `[5,120]` vs Rust `scheduler.rs:45` 的 `5..=720` ⇒ **不一致**（只能经 config_sync 或手改库触达：那时后端按 300 跑、UI 读回被校验器丢弃而显示默认值）
  - `fetchConcurrency`：前端 `[1,16]` vs Rust `1..=MAX_CONCURRENCY(16)` ⇒ 一致
  - `fontSize/lineHeight/maxWidth/listWidth`：与各自控件 min/max 同源，一致

---

# 二、本轮**推翻/降级**的结论

## 2.1 round-2 的 P2-9（N11 标记粘滞 → 卡片字面显示 `<p>`）：两轮构造均**未复现** ⇒ 建议降级为「机制存在、可达性未证实」

- 代码层面机制确实存在（我逐行核对）：失败路径（`store/slices/ai.ts:220-240`）只清 `isShowingTranslatedProse` 与 `translating`，**不清 `rawTranslatedIds[id]`**；而两条水合写回（`reader.ts:137`、`reader.ts:232`）都把 DB 的 `translated_content` 写进条目、**不联动清除该标记**；社交/通知卡片以 `rawTranslated` 单条件裁决渲染方式。
- 但两次真机构造都没有让它显形：
  1. s12 E8：失败 → 写 DB 译文 → 切视图强制重新水合 → 译文块为空（走的是 `translatedContent` 为空的路径，未触发 HTML 写回 + 粘滞标记的组合）；
  2. s14 G2：失败 → 写 DB 译文 → **切布局再切回（强制重挂载）** → 再点「翻译」时条目里 `translatedContent` 仍为空，于是**重新生成**（成功），最终按 HTML 渲染（`renderedP=true`，无字面标签）。
- 结论：**现象未复现**，但代码缺口是真的。要在真机显形，需要「同一条目在失败后**再次经历水合写回**」这一额外条件，我未能构造出来。按证伪优先原则，本轮**不把它当作已确认缺陷**，建议在修复清单里标为「机制存在、可达性待确认」，并补一条能稳定复现的断言。

## 2.2 我自己的 harness 缺陷（本轮最值得记录的方法学问题）

- **IPC Channel 的流式事件从未投递**：`drive.mjs` 的 shim 只认 `onChannel` 是字符串（`'__CHANNEL__:N'`），而 shim 里的 `invoke` 拿到的是**活的 Channel 实例**（真实 Tauri 会先序列化）。后果：s12 首跑时「摘要一直生成中、译文为空」——**看起来像产品缺陷，实际是我的 shim**。修正为「字符串或 `toJSON()` 两种形态都认」后，E3/E4/E5 立刻全绿。
- **autoplay 手势**：合成 `.click()` 不构成用户手势 ⇒ `<audio>` 不播放。改用 CDP `Input.dispatchMouseEvent` 后播放成立。
- **关闭态的弹窗仍可被程序化点击**（`ModalOverlay` 无条件渲染 children，关闭态仅 `opacity:0 + inert`）：所有选择器必须限定 `.modal-overlay.open`；「是否打开」也必须判 `open` 类（此前用 `.settings-modal` 存在性判断恒为真，导致 Esc 关闭设置被误判为失败）。
- **设置写入的类型陷阱**：用 SQL `json_set(..., json('true'))` 写布尔值曾使开关不生效；新增 `/__setting` 端点做「类型正确的 JSON 合并」后，`markReadOnScrollOut` 的实验组才成立。

---

# 三、仍未覆盖（并说明为什么）

| 事项 | 状态 | 原因 |
| --- | --- | --- |
| 同步引擎深挖（推拉游标、冲突裁决、离线补推、队列收敛） | 未做 | 路线 N 两次派发均未启动；本轮预算优先给了「能跑起来的功能验证」 |
| 设置项四方一致性总表（路线 Q） | 部分 | 本轮只自核了区间与校验表覆盖；其余键的「UI 值域 vs Rust 消费」未逐项列表 |
| config_sync 真机往返（WebDAV/Gist） | 未做 | 我的后端即 Rust 的替身，做 WebDAV 只能在自家 mock 里自证；UI 侧只验了入口与「未配置」态 |
| GitHub 设备流登录 | 未做 | 需要真实 GitHub 授权；本地状态机可用 mock 验，但价值低于成本 |
| SMTC 真实系统媒体键、托盘图标行为、开机自启写注册表、系统通知授权弹窗 | 未验证 | 需真实 Windows 会话；本轮只验到「前端/插件调用侧」 |
| `media.rs` SMTC 后端路径 | 未验证 | 同上（沿用前轮结论） |

---

# 四、证据索引

| 脚本 | 覆盖 | 输出 |
| --- | --- | --- |
| `tmp/audit-r3/harness/s12_reader.mjs` | 阅读器/AI/全文/视觉设置/正文图代理 | `s12_stdout.log` |
| `tmp/audit-r3/harness/s13_system.mjs` | 关闭询问、自启动（含失败回滚）、通知、关于/CSP、OPML 导入 | `s13_stdout.log` |
| `tmp/audit-r3/harness/s14_pin.mjs` | 控制台捕获自证、About 的 fetch 尝试、P2-9 强制重挂载 | `s14_stdout.log` |
| `tmp/audit-r3/harness/s15_about.mjs` | About/CSP 定论（含 CSP 原文与控制台报错原文） | `s15_stdout.log` |
| `tmp/audit-r3/harness/s16_player.mjs` / `s17_pin2.mjs` | 播放/倍速/暂停/Space/M、音频手势策略、滚动标读 | `s16_stdout.log` / `s17_stdout.log` |
| `tmp/audit-r3/harness/s18_scrollout.mjs` | 「滚动出列表标读」的前置条件对照实验 | `s18_stdout.log` |
| `tmp/audit-r3/harness/mock_backend.py` | 忠实假后端：新增真实 WAV、CSP 响应头、`/__setting`、`plugin:app|version`、真实 OPML 解析 | — |

> 门禁未跑（按约定）；业务代码零改动（`git status -- src src-tauri tools` 为空）；
> 真实库只读（`?mode=ro`），本轮未触碰真实库。
