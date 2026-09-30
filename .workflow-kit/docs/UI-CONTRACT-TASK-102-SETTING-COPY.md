# UI 契约：TASK-102 设置页控件对齐与文案删除性精简（3 条 ui_checks）

基线：dev@8b9efe4（v0.16.1 发布后）。取证：tmp/audit-r3/harness（Chrome CDP + 忠实假后端），
SyncTab 浅/深截图 + DOM 断言 + 源级结构断言。

## X1.protocol-control-aligned

「同步协议」卡的下拉控件与其他设置卡控件列对齐：
- FluxDropdown 为 SettingCard 的直接子元素（结构断言：SyncTab 协议卡内不再有包裹
  下拉与提示的裸 div）；
- 控件不再因提示文本撑宽包裹层而左移（真机截图核对与其他卡控件列同一右缘对齐）。

## X2.sync-copy-concise

- 动作行（测试连接/保存并同步按钮）下的常驻提示为**单行 ≤40 字**，语义与两按钮
  一一对应（测试连接 = 仅验证登录；保存并同步 = 确认后拉取订阅与文章）；
- 「断开会移除/删除拉取内容」**不再出现在常驻文案**——该破坏性语义仅由断开确认框
  （ConfirmDialog）承载；
- 「已读/收藏约 1 秒回传」类解释性细节不再出现；
- API 密码提示位于协议卡 desc 位且 ≤32 字，无第二行常驻 hint。

## X3.setting-copy-scan

全设置页（GeneralTab/ReadingTab/AppearanceTab/AboutTab/FeedsTab/AiTab/
ConfigSyncSection/CacheCleanupSection/SyncTab 及 settings/shared）静态扫描：
- SettingCard desc、mini-dialog-hint、组标题下说明文案 ≤48 字；
- 无复述标题的 desc（如「密码」卡 desc 不再重复「密码」字面解释）；
- 约束性语义（数字阈值、确认类、不可回退警示）零丢失——删除仅限复述与花絮。

## 明确不做（本契约范围外）

主界面卡片/阅读器文案；Rust 代码；断开确认框文案（已合格）。
