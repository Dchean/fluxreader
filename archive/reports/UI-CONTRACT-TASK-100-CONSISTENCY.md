# UI 契约：TASK-100 自检遗留一致性收口（8 条 ui_checks）

基线：v0.15.0（tag v0.15.0，e05f2e1）。本契约约定本卡 UI 改动后「必须为真」的可核对状态。
取证方式：tmp/audit-r3/harness（Chrome CDP + 忠实假后端），浅色/深色各一套截图 + 交互报告，
DOM 断言采集（图标节点为 svg、title 属性、提示文案文本）。

## U1.shortcut-hint-format
快捷键提示全仓统一为紧凑加号形态：`Ctrl+K`、`Ctrl+,`、`Esc`。
禁止形态：`Ctrl K`、`Ctrl ,`、`Ctrl + K`、`ESC 关闭`。
涉及：Sidebar（搜索入口、设置入口）、SettingsModal、ShortcutsTab、Overlays（浮层关闭提示）。

## U2.add-feed-wording
同一动作只叫「添加订阅源」。右键菜单（ContextMenu）与设置页订阅源页签（FeedsTab）
不得出现「新建订阅源」「添加源」；侧栏「+」、命令面板、弹窗标题维持「添加订阅源」。

## U3.star-visual
收藏状态视觉统一为 `Icons.star` / `Icons.starFilled` SVG 图标：
SocialCard（收藏按钮）、GalleryCard（收藏按钮）、ArticleCard 页脚标记三处
不再出现 `★`/`☆` 字符或「★ 已收藏」文字形态；Reader 与 ContextMenu 维持现状。

## U4.player-icons
PlayerBar 播放/暂停/关闭/回退 15s/前进 30s/全屏 六个控件与 AppearanceTab
主题三按钮（浅色/深色/跟随系统）全部使用 Icons SVG，无字符/emoji 图标；
图标颜色走 currentColor，浅色/深色主题下均为主题前景色。

## U5.truncate-title
line-clamp 截断的文本带 `title` 属性：文章卡标题/摘要、画廊卡标题、播客卡标题。
DOM 断言：title 属性存在且等于未截断全文。

## U6.favicon-fallback
favicon 加载失败回退到与「无 favicon」相同的 dot 占位，行首不再留空槽。

## U7.close-ask-esc
「关闭询问」弹窗（CloseAskDialog）可用 Esc 关闭；Esc 关闭顺序与视觉层级一致
（视觉在上者先关：确认框 3000 > 弹窗 150 > 全屏播放器 140）。

## U8.theme-color
`index.html` 的 `meta name="theme-color"` 与深色主题 `--bg-base`（#14161a）一致。

## 明确不做（本契约范围外）
跨布局 J/K 键盘导航、--text-tertiary 对比度调整（均为产品级决策，另行立项）。
