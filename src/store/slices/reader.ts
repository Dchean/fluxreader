import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { appStore, flipEntryFlag, markEntriesRead, syncCurrentViewCache } from '../internals';
import type { AppState } from '../types';

/** 阅读器 slice：选中文章、正文水合（懒加载 + 批量合批）与卡片就地标读/收藏。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type ReaderSlice = Pick<
  AppState,
  | 'activeArticleId'
  | 'isShowingTranslatedProse'
  | 'isRawRenderMode'
  | 'showFulltext'
  | 'hydrationErrors'
  | 'hydratedIds'
  | 'openedReadIds'
  | 'selectArticle'
  | 'ensureArticleContent'
  | 'retryHydration'
  | 'hydrateArticleContent'
  | 'clearReaderSelection'
  | 'extractCurrentArticle'
  | 'toggleReaderFulltext'
  | 'toggleCurrentReadStatus'
  | 'toggleCurrentStar'
  | 'toggleReaderRenderMode'
  | 'markEntriesReadBulk'
  | 'toggleEntryFlag'
>;

/* ============================================================
   正文水合批量队列 —— 首屏/布局切换时几十张可见卡片同帧触发 ensureArticleContent，
   逐篇 getArticle（各一次 IPC + 各一次 set + 全量 selector 重算）是「加载正文
   几秒 + 切换卡顿」的根因。这里把同一帧内的请求合批：微任务 flush 成一次
   get_articles IPC + 一次 set。 */
let hydrationQueue: Set<string> | null = null;
let hydrationFlushScheduled = false;

function enqueueHydration(id: string) {
  if (!hydrationQueue) hydrationQueue = new Set();
  hydrationQueue.add(id);
  if (hydrationFlushScheduled) return;
  hydrationFlushScheduled = true;
  /* 微任务：等当前同步帧内所有卡片都入队后一次性 flush */
  Promise.resolve().then(() => {
    hydrationFlushScheduled = false;
    const q = hydrationQueue;
    hydrationQueue = null;
    if (!q || q.size === 0) return;
    appStore().getState().hydrateArticleContent(Array.from(q));
  });
}

/** 「智能全文」判定：正文是否已是全文（无需 Readability 提取）。
    启发式：只认明确的"正文被截断"信号——摘要型源（少数派等）的结尾标记
    "查看全文/阅读全文/继续阅读/阅读原文" 等。返回 true = 需要提取全文（是摘要）；
    false = 已是全文（跳过，省请求）。
    注意：不按"文本短"判断——论坛（Linux DO）的短帖本身就是完整全文，短 ≠ 摘要；
    也不认"阅读更多"——那是论坛/列表"去原帖看回复"链接，非正文截断。 */
function shouldExtractFulltext(html: string): boolean {
  if (!html) return false;
  const text = html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  // 明确的正文截断标记（"全文/原文"字样；不含"阅读更多"= 论坛去原帖链接）
  const truncationMarks = /查看全文|阅读全文|继续阅读|展开全文|阅读原文|查看原文|Read more|read more|Continue reading|continue reading/;
  return truncationMarks.test(text);
}

export const createReaderSlice: StateCreator<AppState, [], [], ReaderSlice> = (set, get) => ({
  activeArticleId: null,
  isShowingTranslatedProse: false,
  isRawRenderMode: false,
  showFulltext: false,
  hydrationErrors: {},
  hydratedIds: {},
  openedReadIds: {},

  /* ================= 阅读器 ================= */

  selectArticle: (id) => {
    const { entries, settings, dataMode } = get();
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    const shouldMarkRead = dataMode === 'tauri' && settings.markReadOnOpen && !art.isRead;
    if (shouldMarkRead) {
      /* 后端模式：已读落库（不重载快照，本地同步置位即可） */
      /* TASK-067 N10：标读失败对用户可见（此前静默，重启后回退未读） */
      void api.setRead(Number(id), true).catch((e) => {
        get().showToast(`标读失败：${extractError(e)}`);
      });
      markEntriesRead(new Set([id]));
    }
    set((s) => ({
      /* 记录"本次会话中被打开过"：即使标已读，在未读筛选下也保留显示（原地变灰） */
      openedReadIds: { ...s.openedReadIds, [id]: true },
      activeArticleId: id,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
    }));
    /* 打开文章：触发「智能全文」判定（摘要型源才提取原文；列表卡片水合不触发） */
    get().ensureArticleContent(id, { extractFulltext: true });
  },

  /** 正文懒加载（幂等）：列表快照不含 HTML，选中/社交卡片挂载时拉详情水合。
      守卫用「条目仍在」（不依赖 activeArticleId：社交卡片挂载时也走这里）。
      实现：并入批量水合队列（微任务合批）——首屏几十张可见卡片同帧触发时，
      合并成一次 get_articles IPC + 一次 set，避免逐篇 IPC 洪峰与逐篇 O(n) 重渲染。
      注意：extractFulltext（打开文章的智能全文）即使 content 已水合也须执行——
      列表卡片批量水合只填 content，不触发智能全文判定；打开文章时若 content
      已在（列表水合过），仍要走 extractFulltext 分支。 */
  ensureArticleContent: (id, opts) => {
    const { dataMode, entries } = get();
    if (dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === id);
    if (!art) return;
    if (opts?.extractFulltext) {
      /* 打开文章：需要完整详情（含 url/fulltext_extracted）+ 智能全文判定。
         若已水合（列表卡片批量水合过），content 已有，但仍需取详情以判断
         是否要提取全文——故不因 art.content 短路。 */
      void api.getArticle(Number(id)).then((row) => {
        if (!row) return;
        const cur = get().entries.find((a) => a.id === id);
        if (!cur) return;
        const html = row.content_html ?? '';
        /* 若已有正文（列表水合过）且详情正文相同，跳过重复 set，仅补全可能
           缺失的 url/fulltext_extracted 等字段；否则正常水合 */
        if (!cur.content || cur.content !== html) {
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id
                ? {
                    ...a,
                    content: html,
                    rawContent: html,
                    translatedContent: row.translated_content ?? '',
                    snippet: row.snippet || a.snippet,
                    aiSummary: row.ai_summary ?? a.aiSummary,
                    url: row.url ?? a.url,
                    fulltextExtracted: row.fulltext_extracted ?? false,
                  }
                : a,
            ),
          }));
        } else {
          /* content 相同：仍补全 url（列表水合可能缺 url） */
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id && !a.url && row.url ? { ...a, url: row.url, fulltextExtracted: row.fulltext_extracted ?? false } : a,
            ),
          }));
        }
        const mode = get().settings.defaultOpenMode;
        const alreadyExtracted = row.fulltext_extracted ?? false;
        if (mode === 'fulltext' && row.url && !alreadyExtracted && shouldExtractFulltext(html)) {
          void api
            .extractFulltext(Number(id))
            .then((res) => {
              if (!res) return;
              /* TASK-076（P2-10 后半，DEC-req104-p2-10b-fulltext-degraded-20260920）：
                 后端现在直接给结构化 degraded 标志，取代此前「返回内容 == 当前正文」
                 的字符串比对——那种猜法在正文恰好相同时会误判，而且静默、用户看不到
                 原因。降级时如实提示并保持原状（不置标志、不切全文视图）。 */
              if (res.degraded) {
                get().showToast(
                  `未采用全文提取：${res.reason ?? '提取结果不可用'}`,
                  { label: '重试', run: () => get().extractCurrentArticle() },
                );
                return;
              }
              set((s) => ({
                entries: s.entries.map((a) => (a.id === id ? { ...a, content: res.html, fulltextExtracted: true } : a)),
                showFulltext: true,
              }));
            })
            .catch((e: unknown) => {
              const msg = extractError(e);
              get().showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
            });
        }
      }).catch((e: unknown) => {
        /* 打开文章的详情拉取失败：Reader 不能静默空白（REQ-001 排查 P1-8） */
        const msg = extractError(e);
        set((s) => ({ hydrationErrors: { ...s.hydrationErrors, [id]: msg } }));
        get().showToast(`正文加载失败：${msg}`, { label: '重试', run: () => get().ensureArticleContent(id, { extractFulltext: true }) });
      });
      return;
    }
    /* 列表卡片（社交/通知）水合：已水合（含空正文终态）则短路，否则并入批量队列。
       不能只判 art.content——content_html 为 NULL 的条目水合后 content 仍为空串，
       仅按 content 判定会让它每次挂载都重新入队（重复 IPC 洪峰 + 永挂「加载正文…」）。 */
    if (art.content || get().hydratedIds[id]) return;
    enqueueHydration(id);
  },

  /** 水合失败重试：清错误态后重新入队（卡片内联重试入口）。 */
  retryHydration: (id) => {
    set((s) => {
      const next = { ...s.hydrationErrors };
      delete next[id];
      return { hydrationErrors: next };
    });
    enqueueHydration(id);
  },

  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰）。 */
  hydrateArticleContent: (ids) => {
    if (get().dataMode !== 'tauri') return;
    /* 过滤出「仍存在且未水合」的 id（幂等 + 去重） */
    const pending = ids.filter((id) => {
      const a = get().entries.find((e) => e.id === id);
      return a && !a.content && !get().hydratedIds[id];
    });
    if (pending.length === 0) return;
    void api.getArticles(pending.map(Number)).then((rows) => {
      if (!rows || rows.length === 0) return;
      /* 构建 id → 详情 映射，一次性合并进 entries（单次 map，单次 set） */
      const byId = new Map(rows.map((r) => [String(r.id), r]));
      set((s) => {
        let changed = false;
        const entries = s.entries.map((a) => {
          if (a.content) return a;
          const row = byId.get(a.id);
          if (!row) return a;
          changed = true;
          const html = row.content_html ?? '';
          return {
            ...a,
            content: html,
            rawContent: html,
            translatedContent: row.translated_content ?? '',
            snippet: row.snippet || a.snippet,
            aiSummary: row.ai_summary ?? a.aiSummary,
            url: row.url ?? a.url,
            fulltextExtracted: row.fulltext_extracted ?? false,
            hydrated: true,
          };
        });
        if (changed) syncCurrentViewCache(entries);
        return changed ? { entries } : s;
      });
      /* 水合成功的条目记入 hydratedIds 终态（空正文也算已水合），并清其错误态 */
      const hydrated = rows.map((r) => String(r.id));
      set((s) => {
        const nextErrors = { ...s.hydrationErrors };
        for (const id of hydrated) delete nextErrors[id];
        const nextHydrated = { ...s.hydratedIds };
        for (const id of hydrated) nextHydrated[id] = true;
        return { hydrationErrors: nextErrors, hydratedIds: nextHydrated };
      });
    }).catch((e: unknown) => {
      /* 批量水合失败：错误落到对应卡片（社交卡内联重试），不再静默假加载 */
      const msg = extractError(e);
      set((s) => {
        const next = { ...s.hydrationErrors };
        for (const id of pending) next[id] = msg;
        return { hydrationErrors: next };
      });
    });
  },

  clearReaderSelection: () =>
    set({ activeArticleId: null, isShowingTranslatedProse: false, isRawRenderMode: false, showFulltext: false }),

  /** 手动全文提取：Readability 拉原文网页存 content；rawContent 始终保留 RSS 原文
      （供「全文 ↔ RSS 正文」切换回跳）。提取成功后进入全文视图。 */
  extractCurrentArticle: () => {
    const { activeArticleId, entries, dataMode, showToast } = get();
    if (!activeArticleId || dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (!art.url) {
      showToast('该条目没有原文链接');
      return;
    }
    showToast(art.fulltextExtracted ? '正在刷新全文…' : '正在提取全文…');
    void api
      .extractFulltext(Number(activeArticleId))
      .then((res) => {
        if (!res) return;
        const id = activeArticleId;
        /* TASK-076：改判结构化 degraded 标志。此前靠「返回内容 == 当前正文」猜，
           且为了绕开「已提取过的刷新本来就会相同」还要额外判断 fulltextExtracted，
           逻辑脆弱；现在后端直接说明本次是否采用了提取结果，手动刷新与自动全文
           两条路径用同一判据。 */
        if (res.degraded) {
          showToast(`未采用全文提取：${res.reason ?? '提取结果不可用'}`);
          return;
        }
        set((s) => ({
          entries: s.entries.map((a) =>
            a.id === id ? { ...a, content: res.html, fulltextExtracted: true } : a,
          ),
          showFulltext: true,
        }));
        showToast('全文提取完成');
      })
      .catch((e: unknown) => {
        const msg = extractError(e);
        showToast(`全文提取失败：${msg}`, { label: '重试', run: () => get().extractCurrentArticle() });
      });
  },

  /** 全文视图切换：已提取全文 → 在 RSS 原文与全文间切换（不重复请求）；
      未提取 → 触发提取（同 extractCurrentArticle）。 */
  toggleReaderFulltext: () => {
    const { activeArticleId, entries, showFulltext } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (art.fulltextExtracted) {
      // 已提取：直接切换视图
      set({ showFulltext: !showFulltext });
    } else {
      // 未提取：触发提取
      get().extractCurrentArticle();
    }
  },

  toggleCurrentReadStatus: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') {
      void api.setRead(Number(activeArticleId), !art.isRead).catch((e) => {
        get().showToast(`标读状态保存失败：${extractError(e)}`);
      });
    }
    flipEntryFlag(activeArticleId, 'isRead');
    get().showToast(art.isRead ? '已标为未读' : '已标为已读');
  },

  toggleCurrentStar: () => {
    const { activeArticleId, entries, dataMode } = get();
    if (!activeArticleId) return;
    const art = entries.find((a) => a.id === activeArticleId);
    if (!art) return;
    if (dataMode === 'tauri') {
      void api.setStarred(Number(activeArticleId), !art.isStarred).catch((e) => {
        get().showToast(`收藏状态保存失败：${extractError(e)}`);
      });
    }
    flipEntryFlag(activeArticleId, 'isStarred');
  },

  toggleReaderRenderMode: () => set((s) => ({ isRawRenderMode: !s.isRawRenderMode })),

  markEntriesReadBulk: (ids) => {
    if (ids.length === 0) return;
    const { dataMode, entries } = get();
    /* 只处理未读项：已读的重复 setRead 无意义（还会刷 sync_queue）。
       L1：先建一次 id → 条目的索引表，避免对每个 id 都做一次 entries.find
       （O(n·m)）——与 markEntriesRead 的一次遍历口径一致；「全部已读/
       滚动标读」传入几百个 id 时这是可感知的开销。 */
    const byId = new Map(entries.map((e) => [e.id, e] as const));
    const unread = ids.filter((id) => {
      const e = byId.get(id);
      return e ? !e.isRead : false;
    });
    if (unread.length === 0) return;
    if (dataMode === 'tauri') {
      /* TASK-067 N10：批量标读失败单条提示（allSettled 防 toast 洪峰） */
      void Promise.allSettled(unread.map((id) => api.setRead(Number(id), true))).then((rs) => {
        if (rs.some((r) => r.status === 'rejected')) get().showToast('部分文章标读失败');
      });
    }
    const marked = new Set(unread);
    markEntriesRead(marked);
    set((s) => ({
      /* 标读的卡片原地变灰保留（未读筛选下不消失）——合并而非替换：
         批量标读（滚动/全部已读）不能抹掉之前"打开过"的保留记录，
         否则那些卡片会在未读筛选下突然消失（体验为"已读的直接隐藏"）；
         切视图/布局/筛选时的清理逻辑统一把它们移除 */
      openedReadIds: {
        ...s.openedReadIds,
        ...Object.fromEntries(unread.map((id) => [id, true])),
      },
    }));
  },

  /* ================= 卡片就地操作 ================= */

  toggleEntryFlag: (id, field) => {
    const { dataMode, entries } = get();
    if (dataMode === 'tauri') {
      const cur = entries.find((e) => e.id === id);
      if (cur) {
        if (field === 'isRead') void api.setRead(Number(id), !cur.isRead);
        else void api.setStarred(Number(id), !cur.isStarred);
      }
    }
    flipEntryFlag(id, field);
  },
});
