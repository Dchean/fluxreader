# Agent Note: GReader ClientLogin 同时支持 JSON 与经典文本响应

Status: implemented

## Problem

`src-tauri/src/greader.rs` 的 `GReaderClient::login` 用 `resp.json()` 解析 ClientLogin 响应，
只在服务端尊重 `output=json` 时可用。Miniflux 尊重该参数（实证 3/3 通过），
FreshRSS 忽略它、返回经典文本行格式（`SID=…\nLSID=…\nAuth=…\n`，失败时为 `Error=BadAuthentication`），
于是 `error decoding response body`，FreshRSS 上 GReader 三项 live 测试全部失败（ISSUE-017）。

`REQ-SYNC-001` 明确要求同时面向 FreshRSS 与 Miniflux，所以这是已确认需求下的兼容缺口，不是可选增强。

## Decision

登录响应用「先 JSON、失败回退文本」的解析，抽成可单测的纯函数 `parse_client_login(&str) -> AppResult<String>`：

- 先 `serde_json::from_str::<ClientLoginResponse>`；解析成功且 `Auth` 非空即用——**保持 Miniflux 既有行为**。
- 否则按行扫描：`Auth=` 行非空即取其值；同时记住 `Error=` 行。
- 两者都拿不到时：有 `Error=` 就把它作为失败原因返回（如 `ClientLogin 失败：BadAuthentication`），
  否则返回「既非 JSON 也未包含 Auth= 行」。

`login` 改为 `resp.text()` 取原文后交给该函数。`GReaderClient::login` 的公开签名、以及 token 的后续用法
（GET 的 `Authorization: GoogleLogin auth=<token>`、POST 的表单参数 `T=<token>`）都不变。
`serde_json` 已是直接依赖，故不新增依赖。

## Alternatives considered

- **只按文本解析、删掉 JSON 分支**：Miniflux 是当前基线后端，删除 JSON 支持会破坏已实证通过的路径。否决。
- **只按 JSON、请求时不再发送 `output=json`**：FreshRSS 本来就忽略该参数，去了也没用；文本形态仍要解析，等于没解决问题。否决。
- **按响应 `Content-Type` 分支**：FreshRSS 返回 `text/plain`、Miniflux 返回 `application/json`，看似更"干净"，
  但这把正确性押在服务端与中间代理正确设置头部上；「尝试-回退」只看响应体本身，不依赖头部。否决。
- **加一个"响应格式"配置项交给用户选**：把兼容性推给用户，且与 `REQ-SYNC-001` 要求的"同时面向两者"相悖。否决。
- **先 `resp.json()` 失败再 `resp.text()`**：reqwest 的响应体只能消费一次，`json()` 失败后无法再读原文。技术上不可行。

## Consequences

- 收益：FreshRSS 与 Miniflux 两种 ClientLogin 形态都能登录；凭据错误时错误信息带出 `Error=` 码，便于定位。
- 收益：解析逻辑成为纯函数，可用单元测试固定两种形态（本 Note 随附的测试即为此）。
- 代价：JSON 形态会先经历一次注定失败的 `serde_json` 解析尝试（微秒级，可忽略）；
  文本形态在 JSON 解析器报错后才进入按行扫描。
- 代价：**多了一条隐式兼容分支**——将来接第三方后端时，要记得这里支持两种响应形态，
  而响应形态本身没有配置项可查。若日后出现第三种形态，应在此处集中扩展而不是各调用点各写一套。
- 未覆盖：真实服务的 live 复测依赖外部服务与凭据，属运行取证而非本次实现的一部分，结果单独记录在任务证据里。
