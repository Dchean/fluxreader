import type { ArticleEntry, CategoryGroup, FeedItem } from '../types';

/* ============================================================
   Tauri invoke 封装 —— 浏览器开发环境回退 mock
   ============================================================ */

/** 检测是否在 Tauri 窗口内（有无 IPC） */
export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let invokeFn: Invoke | null = null;

async function getInvoke(): Promise<Invoke | null> {
  if (!isTauri()) return null;
  if (invokeFn) return invokeFn;
  const { invoke } = await import('@tauri-apps/api/core');
  invokeFn = invoke as Invoke;
  return invokeFn;
}

/* ============================================================
   后端行类型（Rust 侧 Serialize 的镜像）
   ============================================================ */

export interface FolderRow {
  id: number;
  name: string;
  layout: string;
  auto_summary: boolean;
  auto_translate: boolean;
  collapsed: boolean;
}

export interface FeedRow {
  id: number;
  folder_id: number;
  feed_url: string;
  site_url: string | null;
  title: string;
  favicon_url: string | null;
  layout: string;
  auto_summary: boolean;
  auto_translate: boolean;
  fetch_failed: boolean;
  fetch_error: string | null;
  last_fetched_at: string | null;
}

export interface ArticleListItemRow {
  id: number;
  feed_id: number;
  title: string;
  author: string | null;
  snippet: string;
  image_url: string | null;
  enclosure_url: string | null;
  enclosure_mime: string | null;
  duration_sec: number | null;
  ai_summary: string | null;
  source: string;
  published_at: string | null;
  is_read: boolean;
  is_starred: boolean;
}

export interface ArticleDetailRow extends ArticleListItemRow {
  url: string | null;
  content_html: string | null;
  translated_content: string | null;
  fulltext_extracted: boolean;
}

export interface RefreshSummary {
  new_articles: number;
  failed_feeds: number;
}

export interface SyncReport {
  pushed_states: number;
  pushed_feeds: number;
  pulled_feeds: number;
  pulled_entries: number;
  merged_states: number;
  fallback_entries: number;
  errors: string[];
}

export interface SyncStatusInfo {
  connected: boolean;
  endpoint: string | null;
  /** 服务端账户名（连接时记录，设置页动态显示） */
  account: string | null;
  last_sync: number;
}

export interface ArticleListArgs {
  feed_id?: number | null;
  folder_id?: number | null;
  only_unread?: boolean;
  only_starred?: boolean;
  only_today?: boolean;
  newest_first?: boolean;
  limit?: number;
}

/* ============================================================
   API 门面：Tauri 调用 + 浏览器 mock 回退
   ============================================================ */

export const api = {
  async listFolders(): Promise<FolderRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_folders') as FolderRow[]) : null;
  },
  async createFolder(name: string, layout: string): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('create_folder', { name, layout }) as number) : null;
  },
  async deleteFolder(id: number): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('delete_folder', { id }) as null) : null;
  },
  async updateFolderLayout(id: number, layout: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_folder_layout', { id, layout }) as null) : null;
  },
  async listFeeds(): Promise<FeedRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_feeds') as FeedRow[]) : null;
  },
  async addFeed(
    feedUrl: string,
    title: string | null,
    folderId: number,
    layout: string,
    autoSummary: boolean,
    autoTranslate: boolean,
  ): Promise<FeedRow | null> {
    const inv = await getInvoke();
    return inv
      ? (await inv('add_feed', {
          feedUrl,
          title,
          folderId,
          layout,
          autoSummary,
          autoTranslate,
        }) as FeedRow)
      : null;
  },
  async deleteFeed(id: number): Promise<null> {
    const mount = await getInvoke();
    return mount ? (await mount('delete_feed', { id }) as null) : null;
  },
  /** 编辑源：标题/分类/布局/AI 开关一次性更新（undefined 字段保持不变） */
  async updateFeed(args: {
    id: number;
    title?: string;
    folderId?: number;
    layout?: string;
    autoSummary?: boolean;
    autoTranslate?: boolean;
  }): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_feed', args) as null) : null;
  },
  async renameFolder(id: number, name: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('rename_folder', { id, name }) as null) : null;
  },
  /** 刷新单个订阅源（直连），返回新增条数 */
  async refreshFeed(feedId: number): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('refresh_feed', { feedId }) as number) : null;
  },
  async updateFeedLayout(id: number, layout: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('update_feed_layout', { id, layout }) as null) : null;
  },
  async listArticles(args: ArticleListArgs): Promise<ArticleListItemRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('list_articles', { args }) as ArticleListItemRow[]) : null;
  },
  /** FTS5 全文搜索（标题/正文/作者/AI 摘要/翻译）。浏览器环境返回 null。 */
  async searchArticles(query: string, limit?: number): Promise<ArticleListItemRow[] | null> {
    const inv = await getInvoke();
    return inv ? (await inv('search_articles', { query, limit }) as ArticleListItemRow[]) : null;
  },
  async getArticle(id: number): Promise<ArticleDetailRow | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_article', { id }) as ArticleDetailRow) : null;
  },
  async setRead(id: number, read: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_read', { id, read }) as null) : null;
  },
  async setStarred(id: number, starred: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_starred', { id, starred }) as null) : null;
  },
  async markAllRead(feedId: number | null, folderId: number | null): Promise<number | null> {
    const inv = await getInvoke();
    return inv ? (await inv('mark_all_read', { feedId, folderId }) as number) : null;
  },
  async refreshAllFeeds(): Promise<RefreshSummary | null> {
    const inv = await getInvoke();
    return inv ? (await inv('refresh_all_feeds') as RefreshSummary) : null;
  },
  async getSetting(key: string): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_setting', { key }) as string | null) : null;
  },
  async setSetting(key: string, value: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_setting', { key, value }) as null) : null;
  },

  /* ---- 分类/订阅源级设置落库（接线后端已有命令） ---- */
  async setFolderCollapsed(id: number, collapsed: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_folder_collapsed', { id, collapsed }) as null) : null;
  },
  async setFolderAiFlags(id: number, autoSummary: boolean, autoTranslate: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_folder_ai_flags', { id, autoSummary, autoTranslate }) as null) : null;
  },
  async setFeedAiFlags(id: number, autoSummary: boolean, autoTranslate: boolean): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('set_feed_ai_flags', { id, autoSummary, autoTranslate }) as null) : null;
  },

  /* ---- AI 引擎（OpenAI 兼容：官方 / DeepSeek / GLM / newapi） ---- */
  async getAiConfig(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('get_ai_config') as string | null) : null;
  },
  async saveAiConfig(value: string): Promise<null> {
    const inv = await getInvoke();
    return inv ? (await inv('save_ai_config', { value }) as null) : null;
  },
  /** 连通性测试 + 拉模型列表；失败抛错（UI 层 toast） */
  async aiListModels(baseUrl: string, apiKey: string): Promise<string[] | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const raw = (await inv('ai_list_models', { baseUrl, apiKey })) as unknown;
    if (Array.isArray(raw)) return raw as string[];
    throw new Error(String(raw));
  },
  /** 流式 AI 事件（channel 推送） */
  async aiSummarize(
    articleId: number,
    onDelta: (text: string) => void,
    onDone: () => void,
    onError: (msg: string) => void,
  ): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const { Channel } = await import('@tauri-apps/api/core');
    const channel = new Channel<Record<string, unknown>>();
    channel.onmessage = (msg) => {
      const m = msg as { type?: string; data?: unknown };
      if (m.type === 'delta') onDelta(String(m.data ?? ''));
      else if (m.type === 'done') onDone();
      else if (m.type === 'error') onError(String(m.data ?? 'AI 错误'));
    };
    return (await inv('ai_summarize', { articleId, onChannel: channel })) as string;
  },
  async aiTranslate(
    articleId: number,
    onDelta: (text: string) => void,
    onDone: () => void,
    onError: (msg: string) => void,
  ): Promise<string | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    const { Channel } = await import('@tauri-apps/api/core');
    const channel = new Channel<Record<string, unknown>>();
    channel.onmessage = (msg) => {
      const m = msg as { type?: string; data?: unknown };
      if (m.type === 'delta') onDelta(String(m.data ?? ''));
      else if (m.type === 'done') onDone();
      else if (m.type === 'error') onError(String(m.data ?? 'AI 错误'));
    };
    return (await inv('ai_translate', { articleId, onChannel: channel })) as string;
  },

  /** 全文提取（Readability）：拉原文网页抽正文并覆盖缓存。返回提取的 HTML。 */
  async extractFulltext(articleId: number): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('extract_fulltext', { articleId }) as string) : null;
  },

  /* ---- SMTC 系统媒体控制 ---- */
  /** 播放状态/元数据同步到系统媒体控制（PlayerBar 节流调用）。 */
  async mediaUpdateFull(title: string, show: string, durationSec: number, positionSec: number, playing: boolean): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('media_update_full', { title, show, durationSec, positionSec, playing });
  },

  /** 播放结束/关闭播放条：SMTC 置 Stopped。 */
  async mediaStop(): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('media_stop');
  },

  /* ---- 配置同步（Gist / WebDAV） ---- */
  /** 同步状态（是否已配置/后端/上次上传时间）。 */
  async configSyncStatus(): Promise<{ configured: boolean; backend?: string; lastUpload?: string } | null> {
    const inv = await getInvoke();
    return inv ? (await inv('config_sync_status')) as { configured: boolean; backend?: string; lastUpload?: string } : null;
  },

  /** 保存凭据（JSON：backend/token/server/username/gist_id）。 */
  async configSyncSaveCredentials(credentials: string): Promise<void> {
    const inv = await getInvoke();
    if (inv) await inv('config_sync_save_credentials', { credentials });
  },

  /** 上传配置。返回上传时间戳。 */
  async configSyncUpload(): Promise<string> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Tauri 客户端可用');
    return (await inv('config_sync_upload')) as string;
  },

  /** 下载远端配置（不应用）。返回 payload JSON 字符串。 */
  async configSyncDownload(): Promise<string> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Tauri 客户端可用');
    return (await inv('config_sync_download')) as string;
  },

  /** 应用下载的配置。返回 { imported, skipped }。 */
  async configSyncApply(payload: string): Promise<{ imported: number; skipped: number }> {
    const inv = await getInvoke();
    if (!inv) throw new Error('仅 Tauri 客户端可用');
    return (await inv('config_sync_apply', { payload })) as { imported: number; skipped: number };
  },

  /* ---- OPML 导入导出 ---- */
  /** 导入 OPML 内容（字符串）。返回 (新增, 跳过)。 */
  async opmlImport(content: string): Promise<{ imported: number; skipped: number } | null> {
    const inv = await getInvoke();
    if (!inv) return null;
    return (await inv('opml_import', { content })) as { imported: number; skipped: number };
  },
  /** 导出全部订阅为 OPML 字符串。 */
  async opmlExport(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('opml_export') as string) : null;
  },

  /* ---- Miniflux 同步 ---- */
  /** 返回 null = 浏览器环境（mock 模式） */
  /** 轻量连通测试（GET /v1/me），不落库不做同步 */
  async syncTest(endpoint: string, token: string): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_test', { endpoint, token }) as string) : null;
  },
  /** 保存凭据（先测试，失败不保存）。重活（拉订阅/同步状态）由前端随后台阶段执行 */
  async syncSave(endpoint: string, token: string): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_save', { endpoint, token }) as string) : null;
  },
  /** 分步同步：which='feeds'（订阅层）| 'states'（状态+条目层）。full=true 含全量对账 */
  async syncPhase(which: 'feeds' | 'states', full = false): Promise<SyncReport | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_phase', { which, full }) as SyncReport) : null;
  },
  async syncDisconnect(): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_disconnect') as string) : null;
  },
  /** 缓存清理：days 天前的文章（收藏保留）或 AI 缓存。scope='articles'|'ai' */
  async cacheCleanup(days: number, scope: 'articles' | 'ai'): Promise<string | null> {
    const inv = await getInvoke();
    return inv ? (await inv('cache_cleanup', { days, scope }) as string) : null;
  },
  async syncNow(): Promise<SyncReport | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_now') as SyncReport) : null;
  },
  async syncStatus(): Promise<SyncStatusInfo | null> {
    const inv = await getInvoke();
    return inv ? (await inv('sync_status') as SyncStatusInfo) : null;
  },
};

/* ============================================================
   行类型 → 前端模型适配（snake_case → 前端字段语义）
   ============================================================ */

/** ISO 时间字符串 → unix ms（无时间时取 0，排序仍稳定） */
function parseTs(iso: string | null): number {
  if (!iso) return 0;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? 0 : t;
}

export function feedRowToItem(row: FeedRow): FeedItem {
  return {
    id: String(row.id),
    name: row.title,
    url: row.feed_url,
    favicon: row.favicon_url ?? '',
    // 后端列受 schema 约束（'inherit' | 五布局），此处窄化到前端联合类型
    layout: row.layout as FeedItem['layout'],
    autoSummary: row.auto_summary,
    autoTranslate: row.auto_translate,
    fetchFailed: row.fetch_failed,
  };
}

export function folderRowsToCategories(folders: FolderRow[], feeds: FeedRow[]): CategoryGroup[] {
  return folders.map((f) => {
    const own = feeds.filter((x) => x.folder_id === f.id);
    return {
      id: `cat-${f.id}`,
      name: f.name,
      collapsed: f.collapsed,
      settingsCollapsed: false,
      layout: f.layout as CategoryGroup['layout'],
      autoSummary: f.auto_summary,
      autoTranslate: f.auto_translate,
      feeds: own.map(feedRowToItem),
    };
  });
}

export function articleRowToEntry(row: ArticleListItemRow): ArticleEntry {
  return {
    id: String(row.id),
    feedId: String(row.feed_id),
    title: row.title,
    publishedAt: parseTs(row.published_at),
    isRead: row.is_read,
    isStarred: row.is_starred,
    tags: [],
    source: row.source as ArticleEntry['source'],
    snippet: row.snippet,
    author: row.author ?? '',
    cover: row.image_url ?? undefined,
    imageUrl: row.image_url ?? undefined,
    audioUrl: row.enclosure_url ?? undefined,
    enclosureUrl: row.enclosure_url ?? undefined,
    durationSec: row.duration_sec ?? undefined,
    url: (row as ArticleDetailRow).url ?? undefined,
    aiSummary: row.ai_summary ?? '',
    content: '',
    rawContent: '',
    translatedContent: '',
    fulltextExtracted: (row as ArticleDetailRow).fulltext_extracted ?? false,
  };
}
