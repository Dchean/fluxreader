/** 同步失败可见性（TASK-058）。
 *
 * 背景：后端 `feeds_phase` / `states_phase` 在 `report.errors` 非空时**仍返回 `Ok(report)`**
 * （有意语义：单项失败不应中断整条同步链）。因此前端的 `.catch()` 对该路径**永不触发**——
 * 若调用点不主动读 `report`，失败就对用户完全不可见：
 *
 * ```text
 * 后端返回：{"pulled_feeds":0, "errors":["拉取订阅 …/uncat.xml 建本地失败: [db] FOREIGN KEY …"]}
 * 界面提示：「后端同步完成」      ← 用户看到成功、数据零进来（TASK-056 端到端实测）
 * ```
 *
 * 本模块把 `SyncReport.errors` 翻译成**用户可读**的提示文案，抽成纯函数以便在
 * 无 DOM 的 Node 回归框架中直接断言（沿用 `endpointHint` / `compareVersions` 的既有做法）。
 *
 * 边界（owner 明确）：只做前端消费；**不改 Rust**、不把 errors 非空改成抛错、
 * **不改成功路径的既有文案与顺序**。
 */

/** 与后端 `SyncReport` 对应的最小结构（避免把 api.ts 整个拖进测试依赖）。 */
export interface SyncErrorsLike {
  errors?: string[] | null;
}

/** 失败提示里最多列出几条原因（避免 toast 过长不可读）。 */
export const MAX_DETAIL = 2;

/**
 * 构造「同步完成但有失败项」的提示文案。
 *
 * @returns 有失败项时返回提示字符串；**无失败项（或报告缺失）时返回 `null`**——
 *          调用方据此保持既有成功文案**逐字不变**（契约要求）。
 */
export function syncFailureMessage(report: SyncErrorsLike | null | undefined): string | null {
  const errors = report?.errors;
  if (!Array.isArray(errors) || errors.length === 0) return null;

  const n = errors.length;
  const shown = errors
    .slice(0, MAX_DETAIL)
    .map((e) => String(e).trim())
    .filter((e) => e.length > 0);

  // 兜底：errors 非空但内容全是空白——仍须让用户知道有失败，只是无可读原因。
  if (shown.length === 0) {
    return `同步完成，但有 ${n} 项失败（原因未提供）`;
  }

  const more = n > shown.length ? `；等共 ${n} 项` : '';
  return `同步完成，但有 ${n} 项失败：${shown.join('；')}${more}`;
}

/** 判断一个 report 是否表示「有失败项」（供调用方分支，语义与上者一致）。 */
export function hasSyncFailures(report: SyncErrorsLike | null | undefined): boolean {
  return syncFailureMessage(report) !== null;
}
