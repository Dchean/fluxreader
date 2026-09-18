# TASK-057 实施报告：Endpoint 填法指引与失败提示（用户实测 Bug 1）

- 运行：RUN-2c493374821e440bb88b5a04adcb6c50
- 任务：TASK-057（批次 BATCH-68ca4c8d0dff468fb49faef5eea686fb）
- 授权：DEC-user-testing-bugs-20260918（owner 明确『**不改探测逻辑**，只改文案与错误提示』）
- 用户报告：`https://demo.freshrss.org` 无法连接，必须填 `https://demo.freshrss.org/api/greader.php`
- 日期：2026-09-18

---

## 1. 缺陷与根因

`greader.rs` 把 Endpoint **原样**当根 URL 拼接（`let base = endpoint.trim_end_matches('/')`，
随后 `{base}/accounts/ClientLogin`），**从不规范化、也不识别 API 路径**。
而两种后端把 GReader API 放在不同位置：

| 后端 | API 真实位置 | 实测（2026-09-18，curl） |
| --- | --- | --- |
| Miniflux | 站点**根** | `POST https://reader.example.com/accounts/ClientLogin` |
| FreshRSS | **子路径** `/api/greader.php` | `POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` → **401**（端点存在） |
| FreshRSS（填纯域名） | — | `POST https://demo.freshrss.org/accounts/ClientLogin` → **404**（HTML 错误页，端点不存在） |

设置页原文案 `例如 https://reader.example.com` **只教了 Miniflux 填法** →
FreshRSS 用户按提示填纯域名必然 404；且失败提示只回显状态码（`ClientLogin → 404`），
用户无从判断是自己填错还是服务端问题。

## 2. 改动内容

| 文件 | 改动 |
| --- | --- |
| `src/components/settings/endpointHint.ts` | **新增**：`ENDPOINT_DESC` / `ENDPOINT_PLACEHOLDER` / `isMissingPathError` / `endpointHint`（纯模块，便于在无 DOM 的 Node 回归中直接断言，沿用 `compareVersions` / `aiConfig` 既有做法） |
| `src/components/settings/SyncTab.tsx` | Endpoint 卡片 desc/placeholder 改用上述常量；两处失败 toast（测试连接、保存并同步）改走 `endpointHint(extractError(e))` |
| `tools/frontend-regression.mjs` | 新增 14 条 `(e1)`–`(e5)` 断言（含 3 条宽度防回归项） |
| `tsconfig.test.json` | include 加入 `endpointHint.ts` |

**核心行为**：仅对「路径类」失败（404/405/410）附加可操作指引；
凭据类失败（401/403/`BadAuthentication`/网络错误）**保持原样**，不误报为 Endpoint 填错。

## 3. 验证

### 3.1 前端断言（修前失败 / 修后通过）

修前对照实验（把 `endpointHint` 退回直接返回原文、desc/placeholder 退回旧文案后跑套件）：

```text
exit: 1
failing new assertions: 5
  ❌ (e1) Endpoint 说明同时给出 Miniflux（站点根）与 FreshRSS（/api/greader.php）两种填法
  ❌ (e1) placeholder 给出 FreshRSS 的完整 API 路径…
  ❌ (e3) 纯域名导致的 404 → 提示指明 Endpoint 需指向 API 路径
  ❌ (e3) 该提示给出 FreshRSS 的完整写法，用户能据此改正
  ❌ (e3) 405/410 同属「路径不存在」，同样附加指引
```

修复后：**255/255 通过**（既有 26 + 新增 229，其中本任务新增 **14** 条）。
> 计数订正（第 2 轮审查指出）：本节初版写 254/254 与「新增 13 条」，
> 那是**加入第 4 条 `(e1)` 宽度断言之前**的读数（RUN-6f6e5982 记 254）；
> 加入后为 14 条、合计 **255**（RUN-4e0352510 实测）。**原数字偏小、非夸大**，现更正。

### 3.2 实机 UI 证据（本任务 `ui_change=true`）

经 CDP 在**运行中的应用**内驱动（未注入任何 OS 级键鼠事件），产出三张实机截图 +
交互报告 `TASK-057-interaction-report.md`，逐条核对 `ui_checks`：

- 深色 / 浅色两套主题的 Endpoint 卡片（新 desc 完整换行、未溢出）；
- 对**真实 FreshRSS 实例**填纯域名点「测试连接」，实测 toast：
  `连接失败：ClientLogin → 404 Not Found（该地址下没有 GReader API：请确认 Endpoint 是否需指向 API 路径。Miniflux 填站点根，FreshRSS 需填 https://主机/api/greader.php）`

### 3.3 宽度问题：两次判断错误，第三次才量对（如实记录）

placeholder 的宽度在本任务中**错了两次**，都由自查/审查发现：

| 次 | 取值 | 声称宽度 | 实际 | 问题 |
| --- | --- | --- | --- | --- |
| 1 | `https://reader.example.com 或 https://主机/api/greader.php` | — | **310.16px** | 被截断，且**恰好截掉 `/api/greader.php`**——部分抵消修复目的 |
| 2 | `https://demo.freshrss.org/api/greader.php` | 「219px，可完整显示」 | **219.46px** | 超出内容盒 0.63px（见下方订正：是否真的裁字，两轮审查结论不同） |
| 3 | `如 https://主机/api/greader.php` | **162.09px** | 162.09px | 完整可读，余量 56.74–56.91px（取决于用 218.82 还是 219.0 的严格/取整口径） |

> **关于第 2 次的「末位字符被裁」——本报告此前的措辞过强，现订正**：
> 第 1 轮审查按字宽差（219.46 > 218.82）判定末位 `p` 被裁；
> 第 2 轮审查用**像素级真值对比**（真实 input 与同字体、同原点的未裁剪 span 叠放，DPR 1.71）
> **未能复现裁字**——input 的墨迹右边缘比对照 span 还宽 0.44 CSS px，`p` 及其下伸部完整渲染。
> 两轮审查对「是否真的裁掉一个字符」结论不同，**我本人未做像素级复核，故不声称任一结论**；
> 可确证的是：该串的 advance **超出内容盒 0.63px**，而当前取值有约 57px 余量。
> 无论裁字与否，第 2 次取值都不该采用。

**两次都错在同一个地方：盒模型。** 该输入框 `box-sizing: border-box`、`width: 240px`、
`padding: 6px 10px`、`border: 1px`，故**内容盒只有 219px**：

- 第 1 次我根本没量宽度，只在源码里挑了「看起来完整」的字符串；
- 第 2 次我量了，但用 `getBoundingClientRect().width` 拿到的是**外框 240px**，
  拿它当「能否放下」的门槛 → 结论偏乐观 21px。

**第 2 次是第 1 轮独立审查用真实 Chromium 复算出来的**（审查者按 `box-sizing: border-box`
推出内容盒，再算 Arial 12px 的字宽），我随后用 canvas `measureText` 复测确认：

```text
contentBox 218.82（严格：rect − padding − border） / 219.0（取整：clientWidth − padding）
现值      162.09px  (fits，余量 56.74–56.91)
prevDemo 219.46px  (超出内容盒 0.63px)
prevTwo  310.16px  (fits: false)
```

> 两个口径差异说明：第 1 轮报 218、我报 219，是**同一个盒子的截断与进位之差**，
> 非两次不同测量（第 2 轮审查裁定）。当前取值在**两种口径下都通过**。

**教训（与本会话反复出现的同源毛病一致）**：判定「放不放得下」必须量**内容盒**，
而不是外框；且**「我量过了」不等于「我量对了」**——第 2 次我确实做了测量，
却用错了盒子的定义，若只自检「有没有测量」是发现不了的。
故把该结论写成了断言 + 代码注释，并在 UI 证据里**把输入框清空**，
让 placeholder 真正出现在截图中可被目视核对（此前截图里输入框有值，placeholder 根本不可见）。

另：第 1 轮审查还指出「截图里看不到 placeholder」——该问题已一并修正（见上述清空做法）。

> **断言强度的已知边界（第 2 轮审查指出，如实登记）**：新增的宽度断言是
> **字符预算代理**（`length <= 30` 等），**不是真正的宽度测量**。
> 审查者用真实字宽演示了可绕过它的反例：
> `@@@@@@@@@@@@@@/api/greader.php`（30 字符，**257.28px**）能通过全部 `(e1)` 断言却溢出内容盒。
> 保留理由：该串是同文件内的**常量**，且注释已明示用的是字符预算而非测量口径；
> 另一条断言要求必须含 `/api/greader.php`，故「短而错」的写法会被挡住。
> 此处如实登记其局限，不声称它是通用宽度保证。

## 4. 门禁结果

| 门禁 | 命令 | 结果 |
| --- | --- | --- |
| cargo_test | `cargo test`（src-tauri） | **141 passed / 0 failed / 9 ignored**（未受本任务影响，证明 Rust 侧零改动） |
| lint | `npm run lint` | exit 0，0 warnings / 0 errors |
| build | `npm run build` | exit 0 |
| frontend | `npm run test:frontend` | exit 0，**255/255**（既有 26 + 新增 229；本任务新增 14） |

## 5. 边界遵守（owner 明确）

- **未新增任何 endpoint 自动探测/回退请求**——实现仅为文案与提示映射；
- **`src-tauri/src/**` 零改动**（`git diff --stat -- src-tauri` 为空，`greader.rs` 未触碰）；
- 未改认证流程、`ClientLogin` 解析、表结构；未引入新依赖（`package.json` 零改动）；
- 未改本卡片之外的设置区块。

## 6. 限制（如实记录）

- **未以真实账号走通「填对完整路径后登录成功」**：`demo.freshrss.org` 的公开演示凭据不可得
  （实测任意用户名/密码组合 `ClientLogin` 均回 401）。该成功分支的**拉取行为**
  由 TASK-056 的 mock e2e 覆盖，但**真实实例的端到端成功路径本轮未验证**，
  不声称已验证。
- 未承载「前端不读 `SyncReport.errors`」的缺口修复（`TASK-056-frontend-errors-gap.md` 登记）：
  本任务范围仅 Endpoint 卡片文案与提示。若 owner 希望一并处理，需另行授权（涉及展示同步失败信息）。
