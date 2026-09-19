/** Endpoint 填法指引与失败提示（TASK-057 起，TASK-059 纠正方向）。
 *
 * 抽成纯模块以便在无 DOM 的 Node 回归框架中直接断言（沿用 compareVersions / aiConfig 的既有做法）。
 */

/** Endpoint 的填法：**只填域名**。
 *
 * 需求（owner 2026-09-18 明确纠正）：用户只需要知道自己的域名，**不需要**知道
 * `/api/greader.php` 这类 API 后缀——后端布局差异由应用自动适配（Rust 侧
 * `endpoint_resolve` 按候选地址探测，见该模块文档）。两种布局对用户不可见：
 * - **Miniflux**：GReader API 在站点根（`{域名}/accounts/ClientLogin`），Fever 在 `{域名}/fever/`；
 * - **FreshRSS**：GReader 在 `{域名}/api/greader.php`，Fever 在 `{域名}/api/fever.php`。
 *
 * 实测（2026-09-18，demo.freshrss.org）：
 * - `POST https://demo.freshrss.org/accounts/ClientLogin` → **404**（该路径下无此端点）
 * - `POST https://demo.freshrss.org/api/greader.php/accounts/ClientLogin` → 401（端点存在，仅凭据不对）
 *
 * 即 404 与 401/403 可可靠区分「路径不存在」与「路径正确但凭据错」。
 *
 * **长度预算（实机实测，勿超）**：这张卡片的说明区 `.setting-card-text p` 宽 **234px**，
 * 且样式带 `-webkit-line-clamp: 2`——**超过两行会被截断成「…」**。实测：
 * - TASK-057 遗留的旧文案需 **4 行**（scrollHeight 69px vs clientHeight 35px）→ **一直在被截断**，
 *   用户根本读不到后半句（这属于既有缺陷，本任务一并修正）；
 * - 现取值实测 **227px、单行**（clientHeight == scrollHeight == 17px）→ 完整可读且留有余量。
 *
 * **方向提醒（勿回退）**：TASK-057 曾在误读需求下把文案写成「FreshRSS 需填
 * /api/greader.php」——那是要求用户做应用该做的事，方向反了。此后的文案一律
 * **不得**再要求用户填后缀。
 */
export const ENDPOINT_DESC = '只填域名即可：FreshRSS / Miniflux 自动适配';

/** 输入框 placeholder。
 *
 * **宽度受控**（实测 2026-09-18，此前两次判断均量错了盒子，已订正）：
 * 该输入框 `box-sizing: border-box`、`width: 240px`、`padding: 6px 10px`、`border: 1px`，
 * 故**内容盒仅 219px**（12px Arial —— input 不继承 body 的 Segoe）。据此：
 * - 「两种填法并列」写法实测 **310px** → 被截断（有害）；
 * - `https://demo.freshrss.org/api/greader.php` 实测 **219.46px** → **仍差 0.46px，末位字符被裁**；
 * - 现取值是**纯域名示例**，明显短于上述两者，完整可读且留有余量。
 *
 * 教训：判定「放不放得下」必须用**内容盒**宽度，不能用外框宽度。
 */
export const ENDPOINT_PLACEHOLDER = '如 https://demo.freshrss.org';

/** 判断错误文本是否表示「该路径下没有该端点」（路径类失败）。 */
export function isMissingPathError(message: string): boolean {
  return /\b(404|405|410)\b/.test(message);
}

/** 把失败信息翻译为用户**可操作**的提示。
 *
 * 自动适配（TASK-059）之后，路径类失败的含义变了：404 表示**该域名下的候选地址
 * 都试过了**（域名根 + 常见 API 子路径）仍未找到该协议的 API。此时用户该核对的是
 * **域名与后端开关**，而不是去补后缀——后缀是应用的责任。
 *
 * 边界（owner 明确，TASK-057 起延续）：**不得**把凭据类失败（401/403）误报为
 * Endpoint 问题，那类消息一律原样透出。
 */
export function endpointHint(message: string): string {
  if (!isMissingPathError(message)) return message;
  return (
    `${message}（已自动尝试该域名下的常见 API 路径：请确认域名是否正确，` +
    '以及后端是否已启用 Google Reader / Fever API）'
  );
}
