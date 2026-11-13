import type { StateCreator } from 'zustand';
import { api, articleRowToEntry, extractError } from '../../lib/api';
import { isHttpUrl } from '../../lib/external';
import { applyBodyRow, applyExtractedFulltext, beginFulltextReadback, beginFulltextRequest, dropBodyEntry, finishFulltextReadback, finishFulltextRequest, getBodyEntry, isFulltextTargetCurrent, markBodyFailed, markBodyLoading, markBodyMissing, reconcileFulltextTargetRow } from '../bodyCache';
import type { FulltextRequest, FulltextTarget } from '../bodyCache';
import type { EntryRollbackClaim } from '../internals';
import { appStore, applyReadReceipt, asReadReceipts, beginEntryFieldWrite, beginLocalFlagWrite, currentLocalFlagWriteSerial, endEntryFieldWrite, endLocalFlagWrite, getEntryVersion, markEntriesRead, optimisticEntryFlagToggle, scheduleCountsReconcile, syncCurrentViewCache, trackEntryFlagWrite } from '../internals';
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
  | 'bodyCacheNonce'
  | 'openedReadIds'
  | 'selectArticle'
  | 'markEntryOpened'
  | 'ensureArticleContent'
  | 'retryHydration'
  | 'hydrateArticleContent'
  | 'clearReaderSelection'
  | 'readerCloseNonce'
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
let hydrationQueueGeneration = -1;
let hydrationFlushScheduled = false;

function enqueueHydration(id: string) {
  const generation = appStore().getState().libraryGeneration;
  if (hydrationQueueGeneration !== generation) hydrationQueue = null;
  hydrationQueueGeneration = generation;
  if (!hydrationQueue) hydrationQueue = new Set();
  hydrationQueue.add(id);
  if (hydrationFlushScheduled) return;
  hydrationFlushScheduled = true;
  /* 微任务：等当前同步帧内所有卡片都入队后一次性 flush */
  Promise.resolve().then(() => {
    hydrationFlushScheduled = false;
    const q = hydrationQueue;
    const queuedGeneration = hydrationQueueGeneration;
    hydrationQueue = null;
    if (!q || q.size === 0) return;
    const state = appStore().getState();
    if (state.libraryGeneration !== queuedGeneration) return;
    state.hydrateArticleContent(Array.from(q));
  });
}

/* TASK-103：在途水合去重 → TASK-122：由 bodyById 的 loading 记录承载——
   同 id 在途不重复 IPC 的判定从模块级 Set 收敛为记录状态（无记录才入队；
   loading 记录存在即在途）。useLazyHydrate 的条件重触发（快照替换让水合前提
   重新成立）与虚拟列表的重挂载都会在请求未落地时再次入队；loading 态让
   entryNeedsHydration 为假，重复入队被守卫拦下（原 hydrationInFlight 的
   跨帧窗口与 enqueueHydration 的同帧 Set 去重语义都由此承接）。 */

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

/** 只有当前后端受检提交返回的真实身份可以结束恢复；缺回执不冒充成功。 */
function extractIdentity(res: { contentId?: number | null; contentRevision?: number }): { contentId: number; contentRev: number } | undefined {
  if (typeof res.contentId !== 'number' || !Number.isSafeInteger(res.contentId) || res.contentId <= 0
    || typeof res.contentRevision !== 'number' || !Number.isSafeInteger(res.contentRevision) || res.contentRevision < 0) return undefined;
  return { contentId: res.contentId, contentRev: res.contentRevision };
}

export const createReaderSlice: StateCreator<AppState, [], [], ReaderSlice> = (set, get) => {
  /** 正文stamp只保护缓存落地；恢复后连提示、重试闭包及嵌套请求也必须失效。
   * 同库切篇不换代，失败restore也不换代，原离屏重试语义保持。
   * Note: SUI-A1 — 见 .agents/notes/implemented/architecture/2026-11-09-本地同步与便携恢复界面边界.md */
  const libraryCallbacks = () => {
    const generation = get().libraryGeneration;
    return <Args extends unknown[]>(callback: (...args: Args) => void) => (...args: Args): void => {
      if (get().libraryGeneration === generation) callback(...args);
    };
  };
  /* Note: 正文重试只认失败文章 id；allowDetached 允许视图外/已切走的目标继续走 by-id 详情链 — 见 .agents/notes/implemented/feature/2026-11-13-前端交互契约.md */
  const retryArticleContentFor = (id: string): void => {
    get().ensureArticleContent(id, { extractFulltext: true, allowDetached: true });
  };
  const showArticleContentRetry = (id: string, message: string): void => {
    const inLibrary = libraryCallbacks();
    get().showToast(message, {
      label: '重试',
      key: `reader:content:${id}`,
      run: inLibrary(() => retryArticleContentFor(id)),
    });
  };

  /** 命令归属之外的共同目标资格。列表CID/rev/标题可能仍是旧快照，正文对与锚
   * 只读实体；已接受轻位置/详情的变化由统一通知永久撤销，URL再补看当前轻行。 */
  const fulltextTargetStatus = (id: string, ticket: FulltextRequest): 'current' | 'unknown' | 'stale' => {
    if (ticket.retired || ticket.target.revoked) return 'stale';
    const entry = get().entries.find(item => item.id === id);
    if (entry?.unavailable || (entry?.url !== undefined && entry.url !== ticket.target.url)) {
      ticket.target.revoked = true;
      return 'stale';
    }
    const current = getBodyEntry(id);
    if (!current) return 'unknown';
    if (!isFulltextTargetCurrent(current, ticket)) {
      ticket.target.revoked = true;
      return 'stale';
    }
    return 'current';
  };

  // Note: 全文重试只使用失败时文章 id，不得回读 activeArticleId — 见 .agents/notes/implemented/feature/2026-11-13-前端交互契约.md
  const extractFulltextFor = (
    id: string,
    opts?: { announce?: boolean; allowDetached?: boolean; url?: string },
  ): void => {
    const { entries, dataMode } = get();
    if (dataMode !== 'tauri' || get().libraryRestoreBusy) return;
    const art = entries.find((a) => a.id === id);
    if (art?.unavailable || (!art && !opts?.allowDetached)) {
      if (opts?.announce) get().showToast('全文提取重试目标已失效，未执行');
      return;
    }
    const rec = getBodyEntry(id);
    if (!rec) {
      get().ensureArticleContent(id, { extractFulltext: true, allowDetached: opts?.allowDetached });
      return;
    }
    if (rec.state === 'missing') return;
    const url = art ? art.url : opts?.url;
    if (!url || !isHttpUrl(url)) {
      if (opts?.announce) get().showToast('该条目没有可用的原文链接');
      return;
    }
    const request = beginFulltextRequest(id, url);
    if (!request) return;
    const recovering = rec.state === 'ready' && !rec.content.trim();
    if (opts?.announce) {
      get().showToast(recovering ? '正在从原网页获取…' : rec.fulltextExtracted ? '正在刷新全文…' : '正在提取全文…');
    }
    const inLibrary = libraryCallbacks();
    // Note: 缺正文恢复与普通提取共用文章、正文戳、请求归属和资料库代次守卫 — 见 .agents/notes/implemented/architecture/2026-10-06-正文与-AI-产物的实体缓存.md
    const isCurrent = () => {
      const current = getBodyEntry(id);
      return !get().libraryRestoreBusy && current?.fulltextRequest === request
        && fulltextTargetStatus(id, request) === 'current';
    };
    // LRU 淘汰只丢缓存，不证明内容已更新。错误中心保留的轻身份可按原 ID
    // 重新核对；不保留正文副本/无界墓碑，也不借详情水合触发自动全文或标读。
    const readEvictedTarget = (target: FulltextTarget): void => {
      if (target.revoked || get().libraryRestoreBusy || getBodyEntry(id)) return;
      const stamp = markBodyLoading(id);
      const verification = beginFulltextReadback(id, target);
      if (!verification) return;
      let retryPending = false;
      // 普通详情可以在同一 stamp 上先完成：stamp 只判失效，不是本请求的
      // 独占归属。缓存写入总是换记录对象，成功/失败/旧重试都须保留此归属。
      const loadingOwner = getBodyEntry(id);
      const reportReadFailure = (message: string, missing = false) => {
        if (fulltextTargetStatus(id, verification) === 'stale') return;
        const current = getBodyEntry(id);
        if (current && current !== loadingOwner) return;
        if (current) {
          if (missing) markBodyMissing(id, stamp);
          else markBodyFailed(id, stamp, message);
        }
        const failureOwner = getBodyEntry(id);
        retryPending = true;
        get().showToast(message, { label: '重试', key: `reader:fulltext:${id}`, run: inLibrary(() => {
          if (fulltextTargetStatus(id, verification) === 'stale' || get().libraryRestoreBusy) return;
          const latest = getBodyEntry(id);
          if (latest && latest !== failureOwner) return;
          if (latest) dropBodyEntry(id);
          verification.retired = true;
          readEvictedTarget(target);
        }) });
      };
      void api.getArticle(Number(id)).then(inLibrary((row) => {
        if (fulltextTargetStatus(id, verification) === 'stale') return;
        const current = getBodyEntry(id);
        if (!current) { reportReadFailure('正文缓存已变化，请重试'); return; }
        if (current !== loadingOwner) return;
        if (!row) { reportReadFailure('正文加载失败：文章不存在或已被删除', true); return; }
        if (get().libraryRestoreBusy) { reportReadFailure('资料库正在恢复，请稍后重试正文获取'); return; }
        const entry = get().entries.find(item => item.id === id);
        if (!applyBodyRow(id, row, stamp, { aiSummary: entry?.aiSummary ?? '',
          translatedContent: entry?.translatedContent ?? '', translationDraftOwner: entry?.translationDraftOwner })) return;
        reconcileFulltextTargetRow(id, row);
        if (fulltextTargetStatus(id, verification) !== 'current') {
          get().showToast('条目已更新，未执行旧的全文重试'); return;
        }
        finishFulltextReadback(id, verification);
        extractFulltextFor(id, { announce: true, allowDetached: true, url: row.url ?? undefined });
      })).catch(inLibrary((error: unknown) => reportReadFailure(`正文加载失败：${extractError(error)}`)))
        .finally(inLibrary(() => { if (!retryPending) finishFulltextReadback(id, verification); }));
    };
    const reportFailure = (message: string) => {
      if (!isCurrent()) { finishFulltextRequest(id, request); return; }
      if (!finishFulltextRequest(id, request, message)) return;
      get().showToast(message, {
        label: '重试', key: `reader:fulltext:${id}`,
        run: inLibrary(() => {
          if (get().libraryRestoreBusy) return;
          const status = fulltextTargetStatus(id, request);
          if (status === 'stale') return;
          if (status === 'unknown') {
            request.retired = true;
            readEvictedTarget(request.target);
            return;
          }
          const current = getBodyEntry(id);
          if (current?.fulltextFailure?.ticket !== request) return;
          extractFulltextFor(id, { announce: true, allowDetached: true, url });
        }),
      });
    };
    void api
      .extractFulltext(Number(id))
      .then(inLibrary((res) => {
        if (!isCurrent()) { finishFulltextRequest(id, request); return; }
        if (!res) { reportFailure('未获取到正文，请重试'); return; }
        if (res.degraded) {
          reportFailure(`未采用全文提取：${res.reason ?? '提取结果不可用'}`);
          return;
        }
        if (!res.html.trim()) { reportFailure('原网页未返回可用正文，请重试或查看原文'); return; }
        const identity = extractIdentity(res);
        if (!identity) { reportFailure('未能确认正文已保存，请重试'); return; }
        applyExtractedFulltext(id, res.html, identity);
        set((s) => {
          // 列表的 CID/rev 是上次属性快照的基线；提前抬高它会把旧 reload 误判为
          // 新修订并清空刚恢复的 body。真实提交身份仅落正文实体，等新快照自行对齐。
          const entries = s.entries.map((entry) => entry.id === id ? { ...entry,
            bodyMissing: false, fulltextExtracted: true } : entry);
          syncCurrentViewCache(entries);
          return { entries };
        });
        if (get().activeArticleId === id) set({ showFulltext: true });
        if (opts?.announce) get().showToast(recovering ? '正文已获取' : '全文提取完成');
        // 封面/画廊信息只取真实详情，不从返回 HTML 猜测；回读不覆盖读藏状态。
        const metadata = beginFulltextReadback(id, request.target.url);
        if (!metadata) return;
        const metadataCurrent = () => !get().libraryRestoreBusy
          && getBodyEntry(id)?.fulltextReadback === metadata && fulltextTargetStatus(id, metadata) === 'current';
        void api.getArticle(Number(id)).then(inLibrary((row) => {
          if (!metadataCurrent() || !row || row.content_id !== identity.contentId || row.content_revision !== identity.contentRev
            || row.url !== metadata.target.url || (metadata.target.anchor !== null && `${row.feed_id}|${row.title}` !== metadata.target.anchor)) return;
          const fresh = articleRowToEntry(row);
          set((s) => {
            // 元数据回读只补媒体：正文戳不代表媒体新鲜度。URL/snippet由原属性
            // 链路维护，不能借媒体版本覆盖较新非媒体字段或改变正文失效基线。
            const entries = s.entries.map((entry) => entry.id === id
              && (fresh.mediaRevision ?? 0) >= (entry.mediaRevision ?? 0) ? { ...entry,
              cover: fresh.cover, imageUrl: fresh.imageUrl,
              imageCount: fresh.imageCount, mediaRevision: fresh.mediaRevision,
              audioUrl: fresh.audioUrl, enclosureUrl: fresh.enclosureUrl, durationSec: fresh.durationSec,
            } : entry);
            syncCurrentViewCache(entries);
            return { entries };
          });
        })).catch(inLibrary((error: unknown) => {
          if (metadataCurrent()) get().showToast(`正文已获取，条目信息刷新失败：${extractError(error)}`);
        })).finally(inLibrary(() => finishFulltextReadback(id, metadata)));
      }))
      .catch(inLibrary((e: unknown) => {
        reportFailure(`全文提取失败：${extractError(e)}`);
      }));
  };

  return ({
  activeArticleId: null,
  isShowingTranslatedProse: false,
  isRawRenderMode: false,
  showFulltext: false,
  /* TASK-122：bodyById 写入通知序号（真实写入由 store.ts 注入的 notify 回调
     bump；本 slice 只持初值。语义见 types.ts bodyCacheNonce 注释） */
  bodyCacheNonce: 0,
  openedReadIds: {},
  /* TASK-115②：阅读器关闭信号（nonce，不透明计数器；字段说明见 types.ts）。
     仅在 clearReaderSelection bump——本 action 只发信号，不携带载荷：原选中卡
     id 由 Timeline 在 activeArticleId 跟随 effect 用 ref 记账（关闭 commit 时
     该 ref 保留关闭前值），Timeline 的 readerClose effect 消费本信号归还焦点。
     选型理由见下方 clearReaderSelection 长注释与 readingPosition.ts 头注 X2 节。 */
  readerCloseNonce: 0,

  /* ================= 阅读器 ================= */

  markEntryOpened: (id) => {
    // 灯箱详情可能晚于恢复前沿才完成；所有打开副作用共享此守卫，既有写仍完整结清。
    if (get().libraryRestoreBusy) return;
    const { entries, settings, dataMode } = get();
    const art = entries.find((a) => a.id === id);
    if (!art || art.unavailable) return;
    const shouldMarkRead = dataMode === 'tauri' && settings.markReadOnOpen && !art.isRead;
    if (shouldMarkRead) {
      /* 后端模式：已读落库（不重载快照，本地同步置位即可） */
      /* TASK-067 N10：标读失败对用户可见（此前静默，重启后回退未读）。
         TASK-118（审计相邻缺口）：乐观置位（markEntriesRead）后快照 isRead
         字段版本，失败走统一回滚助手 rollbackEntryClaims——与单条 toggle /
         批量标读同一份实现（恢复读态 + 逐 feed 回补 unread，版本守卫：期间
         已被接管则跳过）。
         边界裁定：失败时文章刚被选中阅读——只回滚读态与计数，不清
         activeArticleId / openedReadIds（用户还在读；「打开过」的保留标记
         使卡片在未读筛选下原地变灰而非消失）。 */
      markEntriesRead(new Set([id]));
      const claim: EntryRollbackClaim = { id, field: 'isRead', prev: false, version: getEntryVersion(id, 'isRead') };
      const settle = trackEntryFlagWrite(claim, true);
      beginLocalFlagWrite();
      beginEntryFieldWrite(id, 'isRead'); // R4：articles 快照的在途写保护
      const flagSerialAtRequest = currentLocalFlagWriteSerial();
      void api.setRead(Number(id), true).then((receipt) => {
        endLocalFlagWrite();
        endEntryFieldWrite(id, 'isRead');
        settle(false);
        /* OPT-008B R2/N1：读闭包可能一并标读同 URL 未读副本——回执驱动条目/
           计数本地收敛（被点行已乐观翻转，跳过重复扣减）。 */
        applyReadReceipt(asReadReceipts(receipt), new Set([id]), flagSerialAtRequest);
        scheduleCountsReconcile();
      }).catch((e) => {
        endLocalFlagWrite();
        endEntryFieldWrite(id, 'isRead');
        const outcome = settle(true);
        /* R2：回滚被接管跳过（如快照替换）时乐观计数不可线性复原——强制一次
           权威对账（不受 dedup 门控），失败保守留待下次 reload 自愈。 */
        scheduleCountsReconcile(outcome.takenOver ? { force: true } : undefined);
        get().showToast(`标读失败：${extractError(e)}`);
      });
    }
  },

  selectArticle: (id) => {
    const st = get();
    if (!st.entries.some((entry) => entry.id === id)) return;
    // 文章分区选中即展示正文；其它分区只选中，不借隐藏 Reader 发起阅读副作用。
    const opensReader = st.activeContentLayout === 'article';
    if (opensReader) get().markEntryOpened(id);
    set((s) => ({
      /* 记录"本次会话中被打开过"：即使标已读，在未读筛选下也保留显示（原地变灰） */
      openedReadIds: { ...s.openedReadIds, [id]: true },
      activeArticleId: id,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
      /* 全局 translating 单布尔不跨文章继承：在途请求的终态复位虽已无条件执行，
         但复位前打开新文章会让自动翻译触发（!st.translating）被他篇在途误拦
         （effect 依赖不含 translating，错过即整篇不再自动翻译）；同篇在途仍由
         translatingIds[id] 守卫，复位不构成双发窗口。
         — 见 .agents/notes/implemented/architecture/2026-11-13-AI与设置边界.md */
      translating: false,
    }));
    /* 打开文章：触发「智能全文」判定（摘要型源才提取原文；列表卡片水合不触发） */
    if (opensReader) get().ensureArticleContent(id, { extractFulltext: true });
  },

  /** 正文懒加载（幂等）：列表快照不含 HTML，选中/社交卡片挂载时拉详情水合。
      守卫用「条目仍在」（不依赖 activeArticleId：社交卡片挂载时也走这里）。
      实现：并入批量水合队列（微任务合批）——首屏几十张可见卡片同帧触发时，
      合并成一次 get_articles IPC + 一次 set，避免逐篇 IPC 洪峰与逐篇 O(n) 重渲染。
      注意：extractFulltext（打开文章的智能全文）即使正文已水合也须执行——
      列表卡片批量水合只填 content，不触发智能全文判定；打开文章时若 bodyById
      已 ready（列表水合过），仍要走 extractFulltext 分支。
      TASK-122：水合真值落 bodyById（记录 state：loading→ready/missing/failed），
      视图行只同步 snippet/url 轻字段；幂等守卫从「art.content || hydratedIds」
      改为「bodyById 已有记录」（loading=在途、ready/cleared/missing/failed=终态，
      失败只能经 retryHydration 删记录后重入队）。 */
  ensureArticleContent: (id, opts) => {
    const { dataMode, entries } = get();
    if (dataMode !== 'tauri') return;
    const art = entries.find((a) => a.id === id);
    if (art?.unavailable) return;
    if (!art && !opts?.allowDetached) return;
    if (opts?.extractFulltext) {
      /* 打开文章：需要完整详情（含 url/fulltext_extracted）+ 智能全文判定。
         若已水合（列表卡片批量水合过），content 已有，但仍需取详情以判断
         是否要提取全文——故不因记录存在而短路。
         TASK-122：发起前签失效戳（记录现 revision）；期间发生显式失效
         （清理 AI 缓存 / 再次生成）则迟到的详情响应整体丢弃。 */
      const stamp = markBodyLoading(id);
      const inLibrary = libraryCallbacks();
      void api.getArticle(Number(id)).then(inLibrary((row) => {
        if (!row) {
          /* 详情缺行 = 文章不存在（旧实现静默 return，记录卡 loading；TASK-122
             终态化：missing 呈现「文章不存在」，可经 retryHydration 幂等重试） */
          markBodyMissing(id, stamp);
          if (opts.allowDetached) {
            showArticleContentRetry(
              id,
              `正文加载失败：${getBodyEntry(id)?.message ?? '文章不存在或已被删除'}`,
            );
          }
          return;
        }
        const cur = get().entries.find((a) => a.id === id);
        /* TASK-122：详情落 bodyById（正文/AI/译文真值）；AI 列 NULL 时保留现值
           （流式半截/未落库产物——与旧 `row.x ?? a.x` 回退逐字同语义）。
           生成中传视图行值做回退（记录在途时其 AI 为空，不能当回退源）。
           条目已被快照替换移除也照常落记录（实体缓存语义，见批量路径同款裁定）；
           仅视图行轻字段同步跳过。 */
        const generating = !!(get().summarizingIds[id] || get().translatingIds[id]);
        const rec = getBodyEntry(id);
        const accepted = applyBodyRow(id, row, stamp, {
          aiSummary: generating ? (cur?.aiSummary || '') : (rec?.aiSummary ?? cur?.aiSummary ?? ''),
          translatedContent: generating ? (cur?.translatedContent || '') : (rec?.translatedContent ?? cur?.translatedContent ?? ''),
          translationDraftOwner: generating ? cur?.translationDraftOwner : rec ? rec.translationDraftOwner : cur?.translationDraftOwner,
        });
        if (accepted) reconcileFulltextTargetRow(id, row);
        if (!accepted || !cur) return;
        /* 视图行轻字段照旧同步：snippet/url（列表行可能缺 url）；无变化不 set
           （保持 entries 引用稳定，避免无谓重渲染） */
        const nextUrl = cur.url ?? row.url ?? undefined;
        const nextSnippet = row.snippet || cur.snippet;
        if (nextUrl !== cur.url || nextSnippet !== cur.snippet) {
          set((s) => ({
            entries: s.entries.map((a) =>
              a.id === id ? { ...a, url: nextUrl, snippet: nextSnippet } : a,
            ),
          }));
        }
        const mode = get().settings.defaultOpenMode;
        const alreadyExtracted = row.fulltext_extracted ?? false;
        if (mode === 'fulltext' && isHttpUrl(row.url) && !alreadyExtracted && shouldExtractFulltext(row.content_html ?? '')) {
          extractFulltextFor(id);
        }
      })).catch(inLibrary((e: unknown) => {
        /* 打开文章的详情拉取失败：Reader 不能静默空白（REQ-001 排查 P1-8）。
           TASK-122：失败落 bodyById failed 态（卡片内联重试入口可用） */
        const msg = extractError(e);
        markBodyFailed(id, stamp, msg);
        showArticleContentRetry(id, `正文加载失败：${msg}`);
      }));
      return;
    }
    /* 列表卡片（社交/通知）水合：bodyById 已有记录且**不是 cleared** 则短路
       （loading=在途、ready/missing/failed=终态），否则并入批量队列。不能只判
       正文有无——content_html 为 NULL 的条目水合后 content 仍为空串，仅按内容
       判定会让它每次挂载都重新入队（重复 IPC 洪峰 + 永挂「加载正文…」）。
       OPT-008B 第二阶段：cleared 记录 = 正文修订失效（contents 已更新）——必须
       重取新正文，不得停在空正文。R3：不在这里删档，由 hydrateArticleContent
       的 markBodyLoading 把 cleared→loading 并**保留失效计数**（旧戳在途响应
       仍被丢弃，见 bodyCache.markBodyLoading）。 */
    const existing = getBodyEntry(id);
    if (existing && existing.state !== 'cleared') return;
    enqueueHydration(id);
  },

  /** 水合失败重试：删记录回「未请求」态后重新入队（卡片内联重试入口）。
      TASK-122：missing/failed 都是可重试终态（重试幂等：missing 重试后仍是
      missing），与旧「清 hydrationErrors 后入队」同语义。 */
  retryHydration: (id) => {
    dropBodyEntry(id);
    enqueueHydration(id);
  },

  /** 批量水合正文：一批 id 一次 IPC 拉取、一次 set 更新（消除逐篇洪峰）。
      TASK-103 终态机 → TASK-122：状态收敛为 bodyById 记录的判别态
      （loading → ready / missing / failed，见 bodyCache 模块头注状态机）：
      - 命中行 → applyBodyRow 写 ready（正文/AI/译文落记录；视图行同步
        snippet/url 轻字段；AI 列 NULL 时保留现值——流式产物不丢）；
      - 响应中缺行的 id → markBodyMissing「文章不存在」终态（卡片呈现明确
        文案，不得静默留加载占位）；空 rows 即整批不存在，同口径；
      - 请求失败 → markBodyFailed（retryHydration 内联重试入口保留）；
      - 应用与终态标记都按**当前 store 状态**逐 id 复核：请求在途期间条目可能
        已被快照替换移除、或已经他路水合（selectArticle 详情拉取）——迟到的
        旧响应不得覆盖新状态。这是与 reloadGeneration 同目标的乱序防护，用
        「按 id 现态复核」而非整批代际号：滚动时并发多批是常态，整批代际会把
        旧批的有效行一并丢弃、卡片反而回到无请求死区；在途去重（loading
        记录）已保证同 id 同时至多一个请求在途。 */
  hydrateArticleContent: (ids) => {
    if (get().dataMode !== 'tauri') return;
    /* 过滤出「仍存在且无记录/需重取」的 id（幂等 + 去重）：loading=在途、
       ready/missing/failed=终态不入队；OPT-008B 第二阶段：cleared（正文修订
       失效）照常入队——按新修订重取，不停在空正文。R3：不删档重取（markBodyLoading
       把 cleared→loading 并保留失效计数，失效前发出的旧戳响应落地时仍被丢弃）。 */
    const pending = ids.filter((id) => {
      const rec = getBodyEntry(id);
      if (rec && rec.state !== 'cleared') return false;
      return get().entries.some((e) => e.id === id);
    });
    if (pending.length === 0) return;
    /* 发起即置 loading（签发失效戳）：同帧重复入队与跨帧重复 IPC 都被
       「记录存在」拦下；响应落地按 stamp 比对丢弃失效后的迟到响应 */
    const stamps = new Map<string, number>(pending.map((id) => [id, markBodyLoading(id)] as const));
    const pendingSet = new Set(pending);
    const inLibrary = libraryCallbacks();
    void api.getArticles(pending.map(Number)).then(inLibrary((rows) => {
      /* 正文接受资格同时约束轻字段，迟到旧批次不能先覆盖 snippet/url。 */
      const byId = new Map((rows ?? []).map((r) => [String(r.id), r] as const));
      const acceptedIds = new Set<string>();
      /* 正文/AI 落记录：逐 id 现态复核——只写仍是 **loading** 的记录（本批的
         在途标记）：他路已水合（selectArticle 详情先落地 → ready）、显式失效
         （cleared）的 id 一律跳过——迟到的旧批次不得覆盖新状态（TASK-103 语义，
         TASK-122 以记录状态表达）。
         条目已被快照替换移除的 id **照常落记录**（TASK-122 裁定）：记录是文章
         实体缓存，不随视图行存活——行真值是该实体的合法缓存，条目若在后续快照
         重现可直接命中（不产生任何视图污染；旧「滞留 hydratedIds 阻断重水合」
         的危害形态在结构上不存在：内容与终态同体存活，重现即有正文）。
         缺行（missing）/失败（failed）同理按实体落账——响应缺行即 DB 已删，
         与视图是否还在无关；失败落账避免 loading 记录无人收尾的死区。 */
      for (const id of pending) {
        const rec = getBodyEntry(id);
        if (!rec || rec.state !== 'loading') continue;
        const row = byId.get(id);
        if (!row) {
          markBodyMissing(id, stamps.get(id) ?? 0);
          continue;
        }
        const generating = !!(get().summarizingIds[id] || get().translatingIds[id]);
        const entry = get().entries.find((a) => a.id === id);
        const accepted = applyBodyRow(id, row, stamps.get(id) ?? 0, {
          aiSummary: generating ? (entry?.aiSummary || '') : (rec.aiSummary || entry?.aiSummary || ''),
          translatedContent: generating ? (entry?.translatedContent || '') : (rec.translatedContent || entry?.translatedContent || ''),
          translationDraftOwner: generating ? entry?.translationDraftOwner : rec.translatedContent ? rec.translationDraftOwner : entry?.translationDraftOwner,
        });
        if (accepted) {
          reconcileFulltextTargetRow(id, row);
          acceptedIds.add(id);
        }
      }
      set((s) => {
        let changed = false;
        const entries = s.entries.map((a) => {
          if (!pendingSet.has(a.id) || !acceptedIds.has(a.id)) return a;
          const row = byId.get(a.id)!;
          const nextUrl = a.url ?? row.url ?? undefined;
          const nextSnippet = row.snippet || a.snippet;
          if (nextUrl === a.url && nextSnippet === a.snippet) return a;
          changed = true;
          return { ...a, url: nextUrl, snippet: nextSnippet };
        });
        if (changed) syncCurrentViewCache(entries);
        return changed ? { entries } : {};
      });
    })).catch(inLibrary((e: unknown) => {
      /* 批量水合失败：错误落到对应记录（卡片内联重试），不再静默假加载。
         TASK-103：只对「仍在途（loading）」的 id 落 failed——过期失败不得污染
         已被快照替换/他路水合/显式失效的新状态 */
      const msg = extractError(e);
      for (const id of pending) {
        const rec = getBodyEntry(id);
        if (rec?.state === 'loading') markBodyFailed(id, stamps.get(id) ?? 0, msg);
      }
    }));
  },

  /* TASK-115②：关闭阅读器（Esc / App 全局快捷键的唯一关闭路径）。修前只清
     四个字段、无焦点归还——关闭后焦点落空、J/K 从列表顶部重新起步（阅读
     上下文丢失）。修法：bump readerCloseNonce 发出关闭信号，Timeline 消费
     信号把焦点归还原选中卡（原卡不在当前列表 → 归零回落不聚焦）。
     选型（Timeline 消费关闭信号 vs 本 action 记录原 id）：选前者——本 action
     只 bump 不透明计数器，原选中卡由 Timeline 在 activeArticleId 跟随 effect
     里记录（关闭时刻它必然持有最后选中的文章 id）。理由：store 不反向依赖
     components 层；共享位置虽归lib/readingPosition，DOM焦点仍由Timeline消费。
     且「只有原 id 字段无法区分连续两次开关
     同一篇」（值不变不触发 effect），nonce 计数器才是可靠的重触发信号。 */
  clearReaderSelection: () =>
    set((s) => ({
      readerCloseNonce: s.readerCloseNonce + 1,
      activeArticleId: null,
      isShowingTranslatedProse: false,
      isRawRenderMode: false,
      showFulltext: false,
      /* 关闭阅读器同样不遗留他篇在途的全局布尔（与 openArticle 同口径，同 Note） */
      translating: false,
    })),

  /** 手动全文提取：Readability 拉原文网页存 content（TASK-122：落 bodyById）；
      rawContent 仅在当前缓存确实持有 RSS 原文时可用于回切；不承诺跨重开保留。
      提取成功后进入全文视图。 */
  extractCurrentArticle: () => {
    const id = get().activeArticleId;
    if (!id) return;
    extractFulltextFor(id, { announce: true });
  },

  /** 全文视图切换：已提取全文 → 在 RSS 原文与全文间切换（不重复请求）；
      未提取 → 触发提取（同 extractCurrentArticle）。
      TASK-122：fulltextExtracted 从 bodyById 记录取（真值源迁移）。 */
  toggleReaderFulltext: () => {
    const { activeArticleId, showFulltext } = get();
    if (!activeArticleId) return;
    const rec = getBodyEntry(activeArticleId);
    if (!rec) return;
    if (rec.fulltextExtracted) {
      if (rec.rawContent && rec.rawContent !== rec.content) set({ showFulltext: !showFulltext });
    } else {
      // 未提取：触发提取
      get().extractCurrentArticle();
    }
  },

  /* F3（Batch 1/2 独立审查 P3）：阅读器两条切换入口与卡片路径共用
     optimisticEntryFlagToggle——修前这里先弹乐观「已标为已读」再 fire-and-forget，
     失败既不回滚也留着成功提示（与卡片路径删假成功 toast 的口径相反）。
     现在：乐观翻转仍立即生效；成功提示经 onSuccess 只在落库成功后出现
     （P1-5 去假成功同口径）；失败回滚「仅当当前值仍等于乐观值」并给失败 toast
     （文案沿用既有）。mock 模式 request 立即 resolve：本地翻转 + toast、无 IPC，
     与修前一致。 */
  toggleCurrentReadStatus: () => {
    const { activeArticleId, dataMode } = get();
    if (!activeArticleId) return;
    optimisticEntryFlagToggle(
      activeArticleId,
      'isRead',
      (next) => (dataMode === 'tauri' ? api.setRead(Number(activeArticleId), next) : Promise.resolve()),
      (msg) => `标读状态保存失败：${msg}`,
      (next) => get().showToast(next ? '已标为已读' : '已标为未读'),
    );
  },

  toggleCurrentStar: () => {
    const { activeArticleId, dataMode } = get();
    if (!activeArticleId) return;
    optimisticEntryFlagToggle(
      activeArticleId,
      'isStarred',
      (next) => (dataMode === 'tauri' ? api.setStarred(Number(activeArticleId), next) : Promise.resolve()),
      (msg) => `收藏状态保存失败：${msg}`,
    );
  },

  toggleReaderRenderMode: () => set((s) => ({ isRawRenderMode: !s.isRawRenderMode })),

  markEntriesReadBulk: (ids) => {
    // 已排队的 Reader scroll / Timeline 滚出 effect 不受 DOM inert 撤销。
    // 恢复先等已有读写前沿；这段期间不得再登记旧成员的被动标读。
    if (get().libraryRestoreBusy) return;
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
    if (dataMode === 'tauri') {
      /* AUDIT P3[F4]（TASK-084）：改为**一次**批量 IPC。
         修前是 `Promise.allSettled(unread.map((id) => api.setRead(...)))` —— 每个 id 一次
         invoke，几百个 id 就是几百次往返。Rust 侧 set_read_bulk 在同一把锁内逐 id 走
         record_read_state（本地写入 + 入队口径与逐条路径完全一致），故语义等价。
         失败提示：整批一次（比修前的“部分失败”粒度更粗，但不再有 toast 洪峰，
         且 catch 保证不会产生 unhandled rejection）。
         TASK-118（审计相邻缺口）：乐观置位后快照各 id 的 isRead 字段版本，
         失败走统一回滚助手 rollbackEntryClaims——与单条 toggle / 打开即标读
         同一份实现：按版本守卫恢复被翻转行、逐 feed 回补 unread（Math.max(0,)），
         不再只提示不回滚。openedReadIds 不回撤：「打开过」的原地变灰保留语义
         不随落库失败撤销（与打开即标读回滚的边界裁定同口径）。 */
      const claims: EntryRollbackClaim[] = unread.map((id) => ({ id, field: 'isRead', prev: false, version: getEntryVersion(id, 'isRead') }));
      const settlements = claims.map((claim) => trackEntryFlagWrite(claim, true));
      beginLocalFlagWrite();
      for (const id of unread) beginEntryFieldWrite(id, 'isRead'); // R4：articles 快照的在途写保护
      const flagSerialAtRequest = currentLocalFlagWriteSerial();
      const listed = new Set(unread);
      void api.setReadBulk(unread.map((id) => Number(id)), true).then((receipt) => {
        endLocalFlagWrite();
        for (const id of unread) endEntryFieldWrite(id, 'isRead');
        for (const settle of settlements) settle(false);
        /* OPT-008B R2/N1：整批回执（含闭包副本）驱动条目/计数本地收敛。 */
        applyReadReceipt(asReadReceipts(receipt), listed, flagSerialAtRequest);
        scheduleCountsReconcile();
      }).catch(() => {
        endLocalFlagWrite();
        for (const id of unread) endEntryFieldWrite(id, 'isRead');
        const takenOver = settlements.map((settle) => settle(true)).some((outcome) => outcome.takenOver);
        /* R2：同单条路径——存在被接管跳过的声明时强制权威对账。 */
        scheduleCountsReconcile(takenOver ? { force: true } : undefined);
        get().showToast('批量标读失败');
      });
    }
  },

  /* ================= 卡片就地操作 ================= */

  toggleEntryFlag: (id, field) => {
    const { dataMode } = get();
    /* F2（Batch 1/2 独立审查 P3）：失败回滚改走共用 helper——「仅当当前值仍等于
       乐观写入值才恢复点击前值」。修前是「再翻一次当前值」：连点两次、第一次
       失败第二次成功时，第一次迟到的 catch 会把第二次已落库的新值踩回旧值
       （探针实测 UI isRead=true / DB is_read=false）。失败 toast 文案沿用既有。 */
    optimisticEntryFlagToggle(
      id,
      field,
      (next) => {
        if (dataMode !== 'tauri') return Promise.resolve();
        return field === 'isRead' ? api.setRead(Number(id), next) : api.setStarred(Number(id), next);
      },
      (msg) => (field === 'isRead' ? `标读保存失败：${msg}` : `收藏保存失败：${msg}`),
    );
  },
  });
};
