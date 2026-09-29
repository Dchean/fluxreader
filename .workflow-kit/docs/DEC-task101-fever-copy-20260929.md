# 决定：TASK-101 Fever 修复与文案精简（DEC-task101-fever-copy-20260929）

- issuer: owner
- status: accepted
- source: owner 2026-09-29 会话指示原话（见 tasks/DECISIONS.json 条目）：Fever+FRESHess 认证失败排查修复 + 同步页面等冗长文案调整为简略清晰
- recorded_at_utc: 2026-09-29T14:02:19Z（tasks/DECISIONS.json）

## 决定

1. 修复 Fever 协议对 FreshRSS 的认证缺陷（REQ-002）：api_key 从 URL query 移到 POST
   form body（FreshRSS p/api/fever.php:172 只读 $_POST['api_key']，取证见
   tmp/task-101/fever-analysis.md）；公式 md5(username:api密码) 经官方文档验证正确不改；
   其余参数留 query（FreshRSS 全走 $_REQUEST）。
2. 设置页冗长提示文案精简为简略清晰口径（REQ-006），以 owner 点名的 SyncTab 段落为标杆，
   其余设置页同类长句一并精简；Fever 入口补「API 密码」简短提示。
3. 行为变化口径：请求形状变化（bug 修复）+ 文案表述变化，语义均保持；
   版本号不动（未要求发版），随 dev → main 流程交付。
