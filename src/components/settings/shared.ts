import { Icons } from '../icons';

export const TAB_META: { id: string; title: string; subtitle: string; icon: () => React.ReactElement }[] = [
  { id: 'general', title: '通用', subtitle: '订阅源刷新、默认行为、启动方式', icon: Icons.settings },
  { id: 'appearance', title: '外观', subtitle: '深浅主题模式、全局调色盘方案', icon: Icons.appearance },
  { id: 'reading', title: '阅读', subtitle: '正文字体、字号、版面、打开方式', icon: Icons.article },
  { id: 'feeds', title: '订阅', subtitle: '分类管理、内容布局绑定、AI规则', icon: Icons.rss },
  { id: 'ai', title: 'AI服务', subtitle: '模型端点配置、连通性探测、自定义提示词', icon: Icons.spark },
  { id: 'sync', title: '同步', subtitle: '后端连接、双向增量同步', icon: Icons.refresh },
  { id: 'shortcuts', title: '快捷键', subtitle: '全键盘导航流转、全局指令', icon: Icons.keyboard },
  { id: 'about', title: '关于', subtitle: '客户端版本信息、底层架构', icon: Icons.info },
];

export const PALETTES = [
  { id: 'blue', name: '幻境蓝', color: '#4880c8' },
  { id: 'zinc', name: '锌灰', color: '#6b7280' },
  { id: 'purple', name: '堇紫', color: '#7873b8' },
  { id: 'emerald', name: '翡翠', color: '#419e79' },
  { id: 'terracotta', name: '赤陶', color: '#c46d54' },
] as const;

/* 字体选项：一律用系统本地字体，不依赖 CDN webfont。
   Windows 优先回退链：UI 走 Segoe UI Variable → Segoe UI → 微软雅黑；
   等宽走 Cascadia Mono → Consolas（JetBrains Mono 非系统字体，移除）。 */
export const FONT_OPTIONS = [
  { value: '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif', label: '系统默认 (Segoe UI / 微软雅黑)' },
  { value: '"Microsoft YaHei", "PingFang SC", system-ui, sans-serif', label: '微软雅黑 / 苹方 (无衬线)' },
  { value: 'Georgia, Cambria, "Times New Roman", serif', label: 'Georgia (优雅衬线)' },
  { value: '"SimSun", "Songti SC", "Source Han Serif SC", serif', label: '宋体 / 思源宋体 (中文衬线)' },
  { value: '"Cascadia Mono", Consolas, monospace', label: 'Cascadia / Consolas (等宽代码)' },
];

/** 不使用 AI 功能的布局：image（画廊·纯图）与 podcast（播客·音频）布局
 *  的卡片不渲染摘要/翻译内容——设置页对这类布局隐藏自动摘要/自动翻译
 *  开关，避免无效开关误导（article/notification 用摘要+翻译，social 用翻译）。 */
export const LAYOUT_NO_AI: ReadonlySet<string> = new Set(['image', 'podcast']);

/** 待确认的删除目标（null = 无确认弹窗） */
export interface PendingDelete {
  kind: 'category' | 'feed';
  catId: string;
  catName: string;
  feedId?: string;
  feedName?: string;
}

/** 预设 → 默认 base_url + 默认模型（custom = newapi 等任意 OpenAI 兼容中转） */
export const AI_PRESETS: Record<string, { url: string; model: string; label: string }> = {
  deepseek: { url: 'https://api.deepseek.com', model: 'deepseek-chat', label: 'DeepSeek 官方 API' },
  openai: { url: 'https://api.openai.com/v1', model: 'gpt-4.1-mini', label: 'OpenAI 官方 API' },
  glm: { url: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash', label: '智谱 BigModel (GLM)' },
  custom: { url: '', model: '', label: '自定义 / newapi 中转' },
};

export interface AiConfigState {
  preset: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  summaryPrompt: string;
  translatePrompt: string;
}

export const DEFAULT_PROMPTS = {
  summary: '你是一名资讯编辑。请用简洁的中文总结这篇文章，输出 3-5 个要点，每个要点一行，以 - 开头。不要重复文章标题。',
  translate: '你是一名专业译者。请把用户提供的 HTML 片段翻译成简体中文：保留所有 HTML 标签和属性原样不动，只翻译标签内的文本内容。直接输出翻译后的 HTML，不要任何解释或代码块包裹。',
};

export const CACHE_PERIODS = [
  { days: 7, label: '1 周' },
  { days: 30, label: '1 个月' },
  { days: 90, label: '3 个月' },
  { days: 365, label: '1 年' },
  { days: 3650, label: '全部' },
];
