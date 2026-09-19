# 基线 · TASK-059（2026-09-18，TASK-058 之后）

## 门禁基线

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **141 passed / 0 failed / 9 ignored** |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**280/280**（既有 26 + 新增 254） |

工作区：`git status --porcelain` 干净；`check` 全绿。

## 需求（owner 原文，2026-09-18）

> 这里的需求处理有误，我是要不管是 freshrss 还是 miniflux，都是只需要填写域名，
> 不用填写 api/greader.php 这种后缀

## 作者的误读经过（如实记录）

用户最初的报告是：

> 一个是直接填写域名无法登录，需要填写 https://demo.freshrss.org/api/greader.php 这种完整的

**那是在描述故障现象**（「填域名连不上，只有填完整路径才行」）。
我却读成「用户想知道该填什么后缀」，并在提问选项里把**自动探测写成被禁止的边界**：

> 选项：『不改探测逻辑，只改文案与错误提示』/『根路径 404 时自动回退到 /api/greader.php』

用户在该错误选项集内选了前者，于是 TASK-057 交付的是**教用户填完整路径**——**方向反了**。
`DEC-user-testing-bugs-20260918` 中「owner 明确选择不改探测逻辑」记录的是这次误导下的选择，
**不代表用户真实需求**；已由 `DEC-endpoint-autodetect-20260918` 取代（旧记录保留）。

**教训**：用户描述故障时给出的「必须这样做才行」，是**症状**而非**期望**。
把症状当成需求，就会做出让用户去适应缺陷的修复。

## 实证：两种后端的路径形态

### GReader

| 请求 | 实测 | 含义 |
| --- | --- | --- |
| `POST https://demo.freshrss.org/accounts/ClientLogin` | **404**（HTML 错误页） | 该路径下**无端点** |
| `POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` | **401**（`Unauthorized!`） | 端点**存在**，凭据被拒 |

**关键**：`404`（路径不存在）与 `401/403/400`（路径对但凭据错）**可可靠区分**——
这正是自动探测需要的判据，也是**必须守住**的边界（否则凭据填错会被误报成「找不到 API」）。

Miniflux 形态相反：`{域名}/accounts/ClientLogin` 即正确路径（站点根）。

### Fever

| 请求 | 实测 | 含义 |
| --- | --- | --- |
| `GET/POST https://demo.freshrss.org/api/fever.php?api` | **200** `{"api_version":4,"auth":0}` | FreshRSS 的 Fever 真实端点 |
| `POST https://demo.freshrss.org/api/fever.php?api`（带 api_key） | **200** `{"api_version":4,"auth":0}` | 同上（auth=0 因演示凭据不可得） |
| `POST https://demo.freshrss.org/fever/?api` | **404** | 现有客户端拼的路径 → FreshRSS **连不上** |

现有 `fever.rs:126` 写死 `format!("{}/fever/?api", self.base)`，故 **Fever + FreshRSS 当前根本不可用**。
这与 GReader 的缺陷**同源**：把「某种后端的路径形态」当成了协议规范。

### Fever 还有第二道拦路（owner 已追加授权）

`fever.rs:142` 写死：

```rust
if env.api_version != 3 {
    return Err(AppError::new("protocol", format!("不支持的 Fever API 版本 {}", env.api_version)));
}
```

而 FreshRSS 实测返回 **`api_version: 4`**。即**即使路径修对，FreshRSS 仍会被这条校验拒绝**，
用户看到的会是「不支持的 Fever API 版本 4」。

**owner 裁决**：放宽为**兼容 3 及以上**（信封结构与 `auth` 字段两者一致）；
**`auth` 校验不得放松**（认证失败仍须报错）。

## 为什么既有测试全漏掉

所有 Rust e2e 与前端回归都使用**已知正确的 endpoint**——
mock 的 `server.url()` 直接就是 API 根（`http://127.0.0.1:PORT`），
其 `/accounts/ClientLogin` 恰好就在该根下。

**因此「用户填的是纯域名」这一输入形态从未被测过。**
这与 TASK-056 的「零 folder 新库」属同类结构性盲区：
**测试跟着实现的假设走，实现假设之外的世界测试也不会去走。**

## 修复方向（owner 已授权，两个协议都做）

1. **GReader**：按候选地址依次探测 `{域名}` → `{域名}/api/greader.php`，
   以**状态码**判定：命中 401/403/400 ⇒ 该路径正确（凭据问题按凭据报）；
   404 ⇒ 继续下一候选；全部 404 ⇒ 报「未找到 API 端点」。
2. **Fever**：候选 `{域名}/fever/` → `{域名}/api/fever.php`（FreshRSS 形态）。
3. **缓存**：解析成功后持久化，避免 `build_client` 在每次 `feeds_phase`/`states_phase`/调度同步中重复探测。
4. **向后兼容**：已填完整路径的输入首个候选即命中，行为与耗时不变。
5. **前端文案更正**：Endpoint 卡片改为「只填域名」；
   TASK-057 遗留的「需填 /api/greader.php」与对应 404 提示**必须一并改**，
   否则界面在教用户做已经不需要的事。

## 边界

- 不改认证流程与 ClientLogin 解析；
- 不改同步协议语义（推送顺序、对账口径、入队条件）；
- 不改 TASK-058 的失败可见性实现；
- 不引入新依赖；
- **不写入用户真实数据库**（端到端测试如需改数据，先备份、结束后逐字节还原并给哈希证据）。
