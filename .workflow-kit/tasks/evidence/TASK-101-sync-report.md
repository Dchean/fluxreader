# TASK-101 UI 审查证据：V1/V2（独立审查员真机取证 2026-09-29）

环境：tmp/audit-r3/harness（Chrome headless CDP + 忠实假后端 mock_backend.py），
被测对象 = 候选工作区构建的 dist/（digest 213c30a48… 与 RUN-9669a13e… 一致）。
真实用户路径：Ctrl+, 打开设置 → 「同步」页签；主题经「外观」页「浅色/深色模式」按钮切换。

| 检查 | 结论 | 证据 |
| --- | --- | --- |
| env.theme-light 浅色主题生效 | PASS | data-theme=light |
| V1.new-main-hint V1 新短文案存在（DOM） | PASS | 主提示「「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。」 |
| V1.a-test-only V1 语义a：测试连接仅验证登录不拉数据 | PASS | 「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。 |
| V1.b-save-pull V1 语义b：保存并同步才开始拉取订阅与文章 | PASS | 「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。 |
| V1.c-disconnect-remove V1 语义c：断开连接会移除同步拉取的内容 | PASS | 「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。 |
| V1.d-pushback-kept V1 回传语义保留（实现取舍：保留） | PASS | 「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。 |
| V1.len-40 V1 单句 ≤40 字 | PASS | 句子长度=[33,32] |
| V1.old-gone V1 被点名旧长句在 DOM 中不存在 | PASS | pane 文本不含四种旧片段 |
| V2.hint-exists V2 Fever「API 密码」提示存在（DOM） | PASS | 同步协议卡内提示「Fever / GReader 均用 FreshRSS 个人设置的「API 密码」，非登录密码。」 |
| V2.single-line-50 V2 提示 ≤50 字且无分号长链 | PASS | 长度 48，无分号 |
| V2.in-fever-area V2 提示位于「同步协议」卡（协议选择区域，Fever/GReader 通用） | PASS | 卡片含下拉框与提示，卡片文本含 API密码=true |
| env.theme-dark 深色主题生效且 V1/V2 文案仍在 | PASS | data-theme=dark，API 密码提示仍在=true |
| env.no-page-errors 全程无页面 JS 错误 | PASS | [] |

**总计：13/13 PASS，总体 = PASS**

## V1.sync-hint-concise
- PASS：新短文案在 DOM 中存在且被点名旧长句四种片段整段不存在；
  三关键语义（a 仅验证登录不拉数据 / b 保存并同步才开始拉取 / c 断开会移除同步拉取内容）逐条断言通过；
  已读/收藏回传语义按实现取舍保留（契约允许精简掉但需说明，progress 已说明取舍）。
  主提示实测："「测试连接」仅验证登录不拉数据，「保存并同步」才开始拉取订阅与文章。已读/收藏等变更约1秒内回传服务端，断开连接会移除同步拉取的内容。"（分句长度 [33,32]）。
## V2.fever-api-password-hint
- PASS：「同步协议」卡（协议选择区域、下拉框正下方）存在单行提示，
  实测："Fever / GReader 均用 FreshRSS 个人设置的「API 密码」，非登录密码。"（长度 48 ≤50，无分号），
  含「API 密码」「个人设置」「FreshRSS」「非登录密码」，并点明 Fever / GReader 通用。

证据文件：TASK-101-sync-light.png（浅色）、TASK-101-sync-dark.png（深色）。