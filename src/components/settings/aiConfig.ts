/** AI 配置在 SQLite 里是「一整份 JSON」（settings 表的 ai_config 键）：
    端点配置（preset/baseUrl/apiKey/model）与两个系统提示词同住一个 blob。

    P2-4：此前「保存提示词」直接把表单整份 JSON.stringify 落库，等于连端点配置
    一起保存 —— 与该按钮的注释/语义（仅保存提示词，不动端点配置）相反，还会把
    用户尚未确认可用的 API Key 顺带持久化。所以「只保存提示词」必须先把库里
    现有配置读回来，只覆盖两个提示词字段。 */

export interface PromptFields {
  summaryPrompt: string;
  translatePrompt: string;
}

/** 生成「只改这几个字段」的落库 JSON：保留库里已有的其它字段（端点配置等），
    只覆盖 fields 里的键。库中无配置或 JSON 损坏时，以 fields 为全部内容重建
    —— 保存动作本身不因旧数据损坏而失败。 */
export function mergeConfigFields(persisted: string | null, fields: Record<string, unknown>): string {
  let base: Record<string, unknown> = {};
  if (persisted) {
    try {
      const parsed = JSON.parse(persisted) as unknown;
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        base = parsed as Record<string, unknown>;
      }
    } catch {
      /* 坏 JSON：忽略旧内容，重建为仅 fields 的配置 */
    }
  }
  return JSON.stringify({ ...base, ...fields });
}

/** 「保存提示词」专用：只覆盖两个提示词字段（见 mergeConfigFields 说明）。 */
export function mergePromptsOnly(persisted: string | null, prompts: PromptFields): string {
  return mergeConfigFields(persisted, {
    summaryPrompt: prompts.summaryPrompt,
    translatePrompt: prompts.translatePrompt,
  });
}
