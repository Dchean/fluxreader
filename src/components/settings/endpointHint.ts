/** Endpoint 填法指引与失败提示（TASK-057 / Bug 1）。
 *
 * 抽成纯模块以便在无 DOM 的 Node 回归框架中直接断言（沿用 compareVersions / aiConfig 的既有做法）。
 */

/** Endpoint 两种协议的真实填法。
 *
 * 背景：`greader.rs` 把 Endpoint **原样**当根 URL 拼接（`{base}/accounts/ClientLogin`），
 * 而两种后端把 GReader API 放在不同位置：
 * - **Miniflux**：站点**根**（`https://reader.example.com/accounts/ClientLogin`）；
 * - **FreshRSS**：**子路径** `/api/greader.php`（`https://主机/api/greader.php/accounts/ClientLogin`）。
 *
 * 实测（2026-09-18，demo.freshrss.org）：
 * - `POST https://demo.freshrss.org/accounts/ClientLogin` → **404**（该路径下无此端点）
 * - `POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` → 401（端点存在，仅凭据不对）
 *
 * 故界面必须同时给出两种填法——此前只写了 Miniflux 形式，FreshRSS 用户按提示填必然失败。
 */
export const ENDPOINT_DESC =
  'Miniflux 填站点根（如 https://reader.example.com）；' +
  'FreshRSS 填 API 路径（如 https://demo.freshrss.org/api/greader.php）';

/** 输入框 placeholder。
 *
 * **宽度受控**（实测 2026-09-18，此前两次判断均量错了盒子，已订正）：
 * 该输入框 `box-sizing: border-box`、`width: 240px`、`padding: 6px 10px`、`border: 1px`，
 * 故**内容盒仅 219px**（12px Arial —— input 不继承 body 的 Segoe）：
 * - 「两种填法并列」写法实测 **310px** → 被截断且**丢掉 `/api/greader.php`**（有害）；
 * - `https://demo.freshrss.org/api/greader.php` 实测 **219.46px** → **仍差 0.46px，末位字符被裁**；
 * - 现取值实测 **162.09px** → 完整可读且留有余量。
 *
 * 教训：判定「放不放得下」必须用**内容盒**宽度，不能用外框宽度；前两次分别误用了
 * 外框 240px 与「按 240px 判断」，故结论都偏乐观。
 */
export const ENDPOINT_PLACEHOLDER = '如 https://主机/api/greader.php';

/** 判断错误文本是否表示「该路径下没有该端点」（路径类失败）。 */
export function isMissingPathError(message: string): boolean {
  return /\b(404|405|410)\b/.test(message);
}

/** 把失败信息翻译为用户**可操作**的提示（TASK-057）。
 *
 * 用户填「纯域名」时请求会打到 `{域名}/accounts/ClientLogin` 并返回 404；
 * 原文案只回显状态码（如「ClientLogin → 404」），用户无从判断是填错还是服务端问题。
 *
 * 边界（owner 明确）：**不新增任何自动探测/回退请求**，只改文案；
 * 且仅对「路径类」失败附加指引，**不得**把凭据类失败（401/403）误报为 Endpoint 问题。
 */
export function endpointHint(message: string): string {
  if (!isMissingPathError(message)) return message;
  return (
    `${message}（该地址下没有 GReader API：请确认 Endpoint 是否需指向 API 路径。` +
    'Miniflux 填站点根，FreshRSS 需填 https://主机/api/greader.php）'
  );
}
