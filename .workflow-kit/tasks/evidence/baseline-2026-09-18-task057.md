# 基线 · TASK-057（2026-09-18，TASK-055 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **137 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**241/241**（既有回归 26 + 新增 store 行为断言 215） |

工作区：`git status --porcelain` 干净；`check` 全绿。

## 用户报告（原文）

> 一个是直接填写域名无法登录，需要填写 https://demo.freshrss.org/api/greader.php 这种完整的，
> 这个是 freshrss 官方的 demo 实例，https://demo.freshrss.org 无法连接上

## 实证定位

### 端点解析：原样当根 URL，不做任何规范化

`src-tauri/src/greader.rs`：

```rust
pub fn new(endpoint: &str, token: &str, http: Client) -> Self {
    let base = endpoint.trim_end_matches('/').to_string();   // 仅去尾斜杠
    ...
}

pub async fn login(endpoint: &str, username: &str, password: &str, http: Client) -> AppResult<Self> {
    let base = endpoint.trim_end_matches('/').to_string();
    let resp = http.post(format!("{base}/accounts/ClientLogin"))  // 直接拼接
    ...
}

fn url(&self, path: &str) -> String {
    format!("{}{}", self.base, path)                          // 直接拼接
}
```

即 **base 永远等于用户填入的字符串**（去尾斜杠），后续所有路径都拼在它后面。
**没有任何逻辑识别或补全 `/api/greader.php`。**

### 实测对照（2026-09-18，curl）

| 请求 | HTTP | 说明 |
| --- | --- | --- |
| `GET https://demo.freshrss.org/` | 302 | 站点根可达（重定向到 Web UI） |
| `GET https://demo.freshrss.org/api/greader.php` | 200 | API 端点存在 |
| `POST https://demo.freshrss.org/accounts/ClientLogin` | **404** | **根路径下无此端点**（返回 HTML 404 页） |
| `POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` | **401** | 端点存在，仅演示凭据不对 |

**结论**：填纯域名时，客户端实际请求的是 `{域名}/accounts/ClientLogin`，而 FreshRSS 的该端点位于
`{域名}/api/greader.php/accounts/ClientLogin` → **404**。用户描述的「无法连接上」即此。

### 文案现状（问题所在）

`src/components/settings/SyncTab.tsx`：

```tsx
<SettingCard title="后端 Endpoint" desc="例如 https://reader.example.com（支持 Google Reader / Fever 协议）">
  <input ... placeholder="https://reader.example.com" ... />
```

- desc 与 placeholder **都只给 Miniflux 形式**（站点根）；
- 完全没有提及 FreshRSS 需要 `/api/greader.php` 子路径。

**因此按界面提示操作必然踩坑。**

### 失败提示现状

失败经由 `api.syncSave` 抛错 → `SyncTab.tsx` 的 `catch` → `showToast(\`保存失败：${extractError(e)}\`)`。
后端错误来自 `AppError::network(format!("ClientLogin → {}", resp.status()))`，
即用户看到的是类似 **「保存失败：ClientLogin → 404」**——
**只有状态码，没有「Endpoint 该填什么」的指引**，用户无从判断是自己填错还是服务端问题。

## 为什么既有测试没抓到

前端 241 项断言覆盖 store 状态机与行为（翻译流、toast、竞态、哨兵判定等），
**不含设置页文案内容断言**；Rust 侧测试一律用**完整 endpoint**（mock server 的
`server.url()` 本身就指向 mock 的根，其 `/accounts/ClientLogin` 就在该根下），
因此「用户填纯域名会 404」这一**输入形态问题**在两侧测试中都没有对应场景。

## 修复方向（owner 已定边界）

owner 明确裁决：**「不改探测逻辑，只改文案与错误提示」**。即：

1. Endpoint 的 desc 与 placeholder 同时给出 Miniflux（站点根）与 FreshRSS（`/api/greader.php`）两种填法；
2. 失败时给出**可操作**提示（指明 Endpoint 需指向 API 路径 + FreshRSS 完整示例），
   而不是只回显 HTTP 状态码。

**明确不做**：不新增自动探测/回退请求；不改 `greader.rs` 的解析语义；不改认证流程。

## 边界

- 本任务为**纯前端 + 文档**：`src-tauri/src/**` 不得改动（`greader.rs` 零改动是硬性证据项）；
- 不引入新依赖（`package.json` 零改动）；
- 不改本卡片之外的设置区块；
- **不写入用户真实数据库**（`%APPDATA%\com.fluxreader.app` 只读）。
