# TASK-101 根因分析：Fever + FreshRSS「api_key 不正确」（主控取证 2026-09-29）

## 现象

用户：Fever 协议登录 FreshRSS →「连接失败：Fever认证失败(api_key不正确)」；GReader 协议正常。

## 根因（已用 FreshRSS 官方源码逐行确认）

本客户端 `src-tauri/src/fever.rs` 把 api_key 拼在 **URL query** 上（`call_probe` :157 与
`mark_items` :395 两处 `url.push_str("&api_key=…")`），POST 请求 body 为空。该形状是当初
对 **Miniflux** 用 curl 实证定的（fever.rs 头注释；Miniflux 的 Go 实现用 r.FormValue，
query 与 form body 都收）。

**FreshRSS 不同**：`p/api/fever.php` 第 172 行——

```php
$feverKey = empty($_POST['api_key']) || !is_string($_POST['api_key']) ? '' : substr(trim($_POST['api_key']), 0, 128);
```

**只读 `$_POST['api_key']`（POST form body）**，完全不读 query。于是它收到的 api_key 恒为空
→ `authenticate()` 恒 false → 信封回 `auth:0` → 客户端报「api_key 不正确」。路径没走错
（能拿到合法 Fever 信封说明端点已达），是传参位置错。

其余参数不受影响：action（groups/feeds/items/…）与 mark/as/id/with_ids/before/since_id 在
FreshRSS 全部走 **`$_REQUEST`**（query 或 body 均可，fever.php:214-258 实证）——我们把这些
留在 query 是对的，不用动。

## 排除项

- **公式无误**：官方文档 docs/en/developers/06_Fever_API.md 明确
  `api_key = MD5("$username:$apiPassword")`，示例 `echo -n "kevin:freshrss" | md5sum` →
  `4a6911fb47a87a77f4de285f4fac856d`；本机 hashlib 逐字验证一致，与现有实现
  `Md5::digest(format!("{username}:{password}"))` 相同。单测
  `api_key_is_md5_of_username_colon_password` 保留。
- **GReader 正常**合理：另一套端点与 ClientLogin 表单认证，与本 bug 无关。
- **API 密码应已设置**：FreshRSS 的 GReader 登录同样使用「API 密码」（个人设置里那个），
  用户 GR 能登上说明密码有效——纯客户端传参问题。但 UI 上仍值得补一行提示，防止
  用户拿登录密码试 Fever。

## 修法（spec）

1. `fever.rs` `call_probe`：从 URL 移除 `&api_key=…`，改 `.form(&[("api_key", &self.api_key)])`
   （application/x-www-form-urlencoded；FreshRSS `$_POST` 与 Miniflux `r.FormValue` 都收）。
   顺带消除 api_key 进 URL/服务器访问日志的泄露面。
2. `fever.rs` `mark_items`（:393-396 第二处）：同样处理——mark/as/id 留 query（`$_REQUEST`），
   api_key 进 body。
3. `endpoint_resolve.rs` `fever_candidates`：增补第三候选 `{base}/p/api/fever.php`
   （FreshRSS 新版把文件移到 p/api 下，官方文档仍写 /api/fever.php=服务器别名；仅 404
   后顺延尝试，顺序在 /api/fever.php 之后，标准安装零影响）。
4. 错误文案：认证失败提示补 API 密码指引（如「Fever 认证失败（api_key 不正确；FreshRSS
   请使用个人设置里的「API 密码」）」）。
5. 测试：
   - 新增 mock Fever 服务器集成测试，**模拟 FreshRSS 行为**（api_key 只认 POST body，
     query 里带了也当无效）——对旧代码跑一遍留修前红证据，修后绿；
   - 断言请求 URL query 中不含 api_key（防回退）；
   - p/api 候选顺延测试。

## 证据来源

- https://raw.githubusercontent.com/FreshRSS/FreshRSS/edge/p/api/fever.php （已存 tmp/freshrss-fever.php）
- https://raw.githubusercontent.com/FreshRSS/FreshRSS/edge/docs/en/developers/06_Fever_API.md
