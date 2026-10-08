# UI 契约：TASK-057（REQ-002 Endpoint 填法指引 + REQ-008 文案一致性）

目标设备：Windows 桌面端（Tauri）。UI 模式：existing（沿用现有布局与交互习惯，**不改版**）。
范围：**仅设置 → 同步 → 「后端 Endpoint」卡片及其失败提示**。其余设置区块不得改动。

## 背景（实测证据）

两种 GReader 兼容后端的 API 布局不同，而当前文案只教了一种：

| 后端 | API 真实位置 | ClientLogin 实测 |
| --- | --- | --- |
| Miniflux | 站点**根** | `POST https://reader.example.com/accounts/ClientLogin` |
| FreshRSS | **子路径** `/api/greader.php` | `POST https://主机/api/greader.php/accounts/ClientLogin` |

实测（2026-09-18）：

```text
POST https://demo.freshrss.org/accounts/ClientLogin                  -> 404（HTML 错误页，端点不存在）
POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin  -> 401（端点存在，仅凭据不对）
GET  https://demo.freshrss.org/                                      -> 302
```

当前文案：`例如 https://reader.example.com`（desc）、`https://reader.example.com`（placeholder）——
**只给 Miniflux 形式**，FreshRSS 用户按提示填写必然失败。

## 需要核对的状态

### A. Endpoint 文案（必须同时覆盖两种协议）

1. **desc 文案**：同时说明 Miniflux 与 FreshRSS 的填法。须让用户能据此判断自己该填哪一种。
   - Miniflux：站点根，如 `https://reader.example.com`
   - FreshRSS：**追加 `/api/greader.php`**，如 `https://demo.freshrss.org/api/greader.php`
2. **placeholder**：反映两种真实填法，不再只给 Miniflux 形式（可为其中一种的完整示例，但不得误导）。
3. **已有值不被破坏**：文案改动不得影响输入框现有取值、受控行为与 `connected` 复用逻辑
   （已连接且留空提交 = 复用已存密码的既有语义**保持不变**）。

### B. 失败提示可操作性（核心验收点）

4. **纯域名填错时的提示**：用户填 `https://demo.freshrss.org`（无 API 路径）保存/测试失败时，
   提示必须**可操作**——明确指出 Endpoint 需指向 **API 路径**，并给出 FreshRSS 的完整写法示例。
   **不得只显示 HTTP 状态码**（如「ClientLogin → 404」）让用户无从下手。
5. **提示不误伤正常失败**：凭据错误（401/`BadAuthentication`）等**非 Endpoint 形态**的失败，
   不应被误报为「Endpoint 填错」。两类失败须可区分，或提示同时给出两类可能原因。
6. **形态一致**：沿用既有 `showToast` / `extractError` 通道与既有 toast 外观，**不新造控件或弹窗**；
   不得引入新的错误展示层级。

### C. 视觉与主题

7. **深色主题**：文案（desc 两行以上时）、placeholder、失败 toast 均完整可读，不截断、不溢出。
8. **浅色主题**：同上。
9. **窄窗口**（设置面板最小宽度）：文案换行正常，不撑破卡片布局。
10. **既有控件状态不变**：「测试连接」「保存并同步」按钮的禁用态、加载态（`testing`/`saving`）
    与既有行为完全一致；协议下拉（`FluxDropdown`）不受本次改动影响。

## 明确不改（owner 边界）

- **不新增 endpoint 自动探测或回退逻辑**：不得在根路径失败后自动尝试 `/api/greader.php`。
- **不改变 endpoint 解析与拼接语义**：`src-tauri/src/greader.rs` 须**零改动**。
- 不改认证流程、不改 `ClientLogin` 解析。
- 不改本卡片之外的任何设置区块。

## 验证方式

- 代码层面：`npm run lint`（0 警告）、`npm run build`、`npm run test:frontend`
  （既有 241 项不回归 + 本次新增断言**修前失败、修后通过**）
- 人工核对：运行应用后按 A1–A3、B4–B6、C7–C10 逐条观察，**深浅主题各一次**，
  并覆盖「填纯域名失败」与「填完整路径」两种输入
- 证据：深/浅主题截图（实机，非设计稿）+ 交互报告说明操作路径与观察结果
- 复查：独立上下文审查核对 diff 与上述状态的可实现性，并验证**未引入自动回退探测**
