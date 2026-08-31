/* 核心实体模型 —— 对应《FluxReader 客户端功能 UI 与交互定义规范》§2.1

   设计原则：实体字段是「同步引擎写入的原始数据」，不是展示态。
   - 归属只存 feedId；分类归属、源名称、头像等由 feed 解析表派生，
     与 Miniflux（feed → category）的结构一致，改名/移动零成本。
   - 时间是 unix 毫秒时间戳；"今天/相对时间"由展示层派生，不落库。
   - 布局不是条目属性：由「feed 布局绑定 → 分类布局」动态解析。 */

export type ContentLayoutType = 'article' | 'social' | 'image' | 'podcast' | 'notification';
export type ViewFilterType = 'all' | 'today' | 'unread' | 'starred';
export type PaletteTheme = 'blue' | 'zinc' | 'purple' | 'emerald' | 'terracotta';
export type ThemeMode = 'dark' | 'light' | 'auto';

export interface FeedItem {
  id: string;
  name: string;
  url: string;
  favicon?: string;
  layout: 'inherit' | ContentLayoutType;
  autoSummary: boolean;
  autoTranslate: boolean;
  /** 该源最近一次直连抓取是否失败（失败走 Miniflux 兜底 + 退避重试） */
  fetchFailed?: boolean;
}

export interface CategoryGroup {
  id: string;
  name: string;
  collapsed: boolean;          // 侧边栏折叠状态
  settingsCollapsed: boolean;  // 设置页折叠状态
  layout: ContentLayoutType;
  autoSummary: boolean;
  autoTranslate: boolean;
  feeds: FeedItem[];
}

export interface ArticleEntry {
  /* ---- 同步引擎写入的原始字段 ---- */
  id: string;
  feedId: string;
  title: string;
  /** 条目发布时间（unix 毫秒）。排序/今天/相对时间全部由此派生。 */
  publishedAt: number;
  isRead: boolean;
  isStarred: boolean;
  /** RSS item 的 categories 标签（本就是数组） */
  tags: string[];
  /** 条目获取来源：direct=客户端直连源站（第一优先级） | miniflux=直连失败兜底拉取 */
  source: 'direct' | 'miniflux';

  /* ---- 正文与增强内容 ---- */
  snippet: string;            // 列表摘要（同步时截取）
  author: string;
  cover?: string;             // 文章封面 / 播客封面
  content: string;            // 渲染态正文 HTML
  rawContent: string;         // RSS 原始正文
  translatedContent: string;  // 译文
  aiSummary: string;
  /** 正文已被 Readability 全文覆盖（手动按钮/设置自动模式共用状态源，DB 持久化；mock 数据可缺省） */
  fulltextExtracted?: boolean;

  /* ---- 布局专属扩展（可空，按布局使用） ---- */
  durationSec?: number;   // 播客：时长秒数
  imageUrl?: string;      // 画廊：大图地址
  audioUrl?: string;      // 播客：音频地址
  enclosureUrl?: string;  // 附件/大图 enclosure
  /** 条目原文网页地址（「源网页」「查看原文」外链打开用） */
  url?: string;
}

export type TabName =
  | 'general'
  | 'appearance'
  | 'reading'
  | 'feeds'
  | 'ai'
  | 'sync'
  | 'shortcuts'
  | 'about';

export interface ToastMessage {
  id: number;
  text: string;
}
