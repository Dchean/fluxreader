import type { StateCreator } from 'zustand';
import { api } from '../../lib/api';
import { appendAiDelta, applyAiProduct, getBodyEntry } from '../bodyCache';
import type { AppState } from '../types';

/** AI slice：摘要与翻译的流式生成（按条目 id 隔离状态），含卡片级与 Reader 级入口。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type AiSlice = Pick<
  AppState,
  | 'summarizingIds'
  | 'translating'
  | 'summaryErrors'
  | 'translateErrors'
  | 'translatingIds'
  | 'rawTranslatedIds'
  | 'translateEntry'
  | 'toggleReaderTranslation'
  | 'summarizeEntry'
  | 'triggerReaderSummary'
>;

export const createAiSlice: StateCreator<AppState, [], [], AiSlice> = (set, get) => {
  /* TASK-122：AI 产物真值源迁往 bodyById（store/bodyCache.ts）。以下三个助手
     收口全部读写：
     - 生效值 = 记录字段 ?? 视图行字段（无记录 = 文章从未水合，产物留在视图行
       过渡字段——此时凭空建记录会让懒水合判定误判「正文已水合」）；
     - 流式增量 appendAiDelta 优先写记录（打字机），无记录回退写视图行；
     - 落账（回读消毒版/重试清半截）applyAiProduct 优先写记录并解除 cleared
       （cleared → ready 的唯一解除路径），无记录回退写视图行。
     视图行上的 aiSummary/translatedContent 因此只剩两种角色：列表行携带的
     DB 真值（未水合卡片的即时显示）与无记录时的流式回退——不再是水合后
     产物的真值源。 */
  const effectiveAi = (id: string, field: 'aiSummary' | 'translatedContent'): string =>
    getBodyEntry(id)?.[field] ?? get().entries.find((a) => a.id === id)?.[field] ?? '';

  const streamAiDelta = (id: string, field: 'aiSummary' | 'translatedContent', delta: string): void => {
    if (appendAiDelta(id, field, delta)) return;
    set((st) => {
      const cur = st.entries.find((a) => a.id === id);
      if (!cur) return st;
      const next = (cur[field] || '') + delta;
      return { entries: st.entries.map((a) => (a.id === id ? { ...a, [field]: next } : a)) };
    });
  };

  const writeAiProduct = (id: string, field: 'aiSummary' | 'translatedContent', value: string): void => {
    if (applyAiProduct(id, field, value)) return;
    set((st) => ({ entries: st.entries.map((a) => (a.id === id ? { ...a, [field]: value } : a)) }));
  };

  return ({
    summarizingIds: {},
    translating: false,
    summaryErrors: {},
    translateErrors: {},
    translatingIds: {},
    rawTranslatedIds: {},

    /** 卡片级翻译（社交/通知卡）：按 id 流式生成该条目译文，不依赖 Reader 选中态。
        复用 toggleReaderTranslation 的流式与消毒回读逻辑，但状态按条目隔离
        （不复用全局 translating 单布尔，避免多卡互串——排查 F4 同类问题的教训）。 */
    translateEntry: (id, opts) => {
      const silent = opts?.silent ?? false;
      if (get().dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      if (!get().entries.some((a) => a.id === id)) return;
      /* 已有译文 → 短路（不重复烧 token）。失败态必须放行：流先产出半截译文
         再报错时，这行会把 toast 的「重试」变成死按钮——不重发请求、
         translateErrors 也不清，卡片永久停在「半截译文 + 错误」并存态（D1b）。 */
      if (effectiveAi(id, 'translatedContent') && !get().translateErrors[id]) return;
      /* 重试语义（与 toggleReaderTranslation 对齐）：清上次的错误**和半截译文**，
         再重新走完整流。此前只清错误不清译文——重试成功后新 delta 会追加在旧半截
         译文后面（打字机里出现「半截+完整」的重复内容）。 */
      writeAiProduct(id, 'translatedContent', '');
      set((st) => ({
        translatingIds: { ...st.translatingIds, [id]: true },
        /* TASK-065 N11：流式 delta 是模型原始输出（未消毒），标记期间渲染走纯文本 */
        rawTranslatedIds: { ...st.rawTranslatedIds, [id]: true },
        translateErrors: { ...st.translateErrors, [id]: '' },
      }));
      void api
        .aiTranslate(
          Number(id),
          (delta) => {
            streamAiDelta(id, 'translatedContent', delta);
          },
          () => {
            /* TASK-065 N11：流式结束回读 DB 的消毒版译文。translatingIds 维持既有
               时序（done 即清）；消毒时序由 rawTranslatedIds 承担——回读成功用
               消毒版覆盖后才清除（期间渲染按纯文本），失败则丢弃未消毒半截 + toast
               （此前无 .catch：半截未消毒译文永久留在渲染路径 + unhandled rejection）。 */
            set((st) => {
              const nextIds = { ...st.translatingIds };
              delete nextIds[id];
              return { translatingIds: nextIds };
            });
            void api
              .getArticle(Number(id))
              .then((row) => {
                const safe = row?.translated_content ?? '';
                set((st) => {
                  const nextRaw = { ...st.rawTranslatedIds };
                  delete nextRaw[id];
                  return { rawTranslatedIds: nextRaw };
                });
                /* 回读不到消毒版（含 row 为空）：丢弃未消毒半截，不留 XSS 窗口 */
                writeAiProduct(id, 'translatedContent', safe);
              })
              .catch(() => {
                set((st) => {
                  const nextRaw = { ...st.rawTranslatedIds };
                  delete nextRaw[id];
                  return {
                    rawTranslatedIds: nextRaw,
                    translateErrors: { ...st.translateErrors, [id]: '译文回读失败' },
                  };
                });
                /* 失败态放行重试（D1 语义），不残留渲染风险 */
                writeAiProduct(id, 'translatedContent', '');
                if (!silent) get().showToast('译文回读失败', { label: '重试', run: () => get().translateEntry(id) });
              });
          },
          (msg) => {
            set((st) => {
              const nextIds = { ...st.translatingIds };
              delete nextIds[id];
              const nextRaw = { ...st.rawTranslatedIds };
              /* TASK-065 N11 × P2-9：清标记的前提是「已无未消毒产物可保护」。
                 流内已落过半截（译文非空）⇒ 标记必须保留，卡片按纯文本渲染，
                 不得进 HTML 路径；半截为空 ⇒ 标记已无保护对象，留着会让后续
                 水合写回的 DB 消毒译文走纯文本分支（字面显示 <p>已消毒译文</p>，
                 即 P2-9 的标记粘连症状）。 */
              if (!effectiveAi(id, 'translatedContent')) delete nextRaw[id];
              return {
                translatingIds: nextIds,
                rawTranslatedIds: nextRaw,
                translateErrors: { ...st.translateErrors, [id]: msg },
              };
            });
            if (!silent) {
              get().showToast(`翻译失败：${msg}`, { label: '重试', run: () => get().translateEntry(id) });
            }
          },
        )
        .catch(() => {
          set((st) => {
            const nextIds = { ...st.translatingIds };
            delete nextIds[id];
            const nextRaw = { ...st.rawTranslatedIds };
            /* 同上：清标记与「是否有未消毒半截」绑定 */
            if (!effectiveAi(id, 'translatedContent')) delete nextRaw[id];
            return {
              translatingIds: nextIds,
              rawTranslatedIds: nextRaw,
              translateErrors: { ...st.translateErrors, [id]: 'AI 服务未配置或不可达' },
            };
          });
          if (!silent) {
            get().showToast('翻译失败：AI 服务未配置或不可达', { label: '重试', run: () => get().translateEntry(id) });
          }
        });
    },

    toggleReaderTranslation: (opts) => {
      const silent = opts?.silent ?? false;
      const s = get();
      /* 关闭 → 直接切回原文 */
      if (s.isShowingTranslatedProse) {
        set({ isShowingTranslatedProse: false });
        return;
      }
      const art = s.activeArticleId ? s.entries.find((a) => a.id === s.activeArticleId) : null;
      if (!art) return;
      const articleId = art.id;
      /* 已有缓存译文 → 直接切换展示；失败态同样放行（D1b 同源）：流内先出半截
         译文再报错时，重试必须真的重发请求，而不是把半截译文当缓存展示。
         TASK-065 N11：未消毒的流式产物（rawTranslatedIds 未清）也不当缓存——
         切换展示会把它按 HTML 渲染进 DOM。
         TASK-122：缓存判定从 bodyById 生效值取（真值源迁移）。 */
      if (effectiveAi(articleId, 'translatedContent') && !s.translateErrors[articleId] && !s.rawTranslatedIds[articleId]) {
        set({ isShowingTranslatedProse: true });
        return;
      }
      /* 无缓存 → 流式生成（打字机效果落到译文） */
      if (s.dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      /* 重试语义：清上次的错误与半截译文，重新走完整流 */
      writeAiProduct(articleId, 'translatedContent', '');
      set((st) => ({
        translating: true,
        isShowingTranslatedProse: true,
        /* TASK-065 N11：流式 delta 未消毒，标记期间 Reader 按纯文本渲染 */
        rawTranslatedIds: { ...st.rawTranslatedIds, [articleId]: true },
        translateErrors: { ...st.translateErrors, [articleId]: '' },
      }));
      void api
        .aiTranslate(
          Number(articleId),
          (delta) => {
            /* 增量追加：Reader 直接渲染译文（打字机） */
            streamAiDelta(articleId, 'translatedContent', delta);
          },
          () => {
            /* TASK-065 N11：流式结束回读 DB 的消毒版译文（后端 ai_translate 落库前
               已 sanitize，流中 delta 是未消毒原样）。translating 维持既有时序（done
               即清）；消毒时序由 rawTranslatedIds 承担——回读成功用消毒版覆盖后才
               清除并切 HTML 渲染；失败则丢弃未消毒半截 + toast（此前无 .catch：半截
               未消毒译文永久留在渲染路径 + unhandled rejection）。 */
            set({ translating: false });
            void api
              .getArticle(Number(articleId))
              .then((row) => {
                const safe = row?.translated_content ?? '';
                set((st) => {
                  const nextRaw = { ...st.rawTranslatedIds };
                  delete nextRaw[articleId];
                  return { rawTranslatedIds: nextRaw };
                });
                /* 回读不到消毒版（含 row 为空）：丢弃未消毒半截，不留 XSS 窗口 */
                writeAiProduct(articleId, 'translatedContent', safe);
              })
              .catch(() => {
                set((st) => {
                  const nextRaw = { ...st.rawTranslatedIds };
                  delete nextRaw[articleId];
                  return {
                    rawTranslatedIds: nextRaw,
                    translateErrors: { ...st.translateErrors, [articleId]: '译文回读失败' },
                  };
                });
                writeAiProduct(articleId, 'translatedContent', '');
                if (!silent) {
                  get().showToast('译文回读失败', { label: '重试', run: () => get().toggleReaderTranslation() });
                }
              });
          },
          (msg) => {
            /* 内联错误（Reader 正文上方展示）+ 非 silent 时 toast 带重试。
               F3（Batch 1/2 独立审查 P3）：失败路径与 translateEntry 同口径处理
               rawTranslatedIds——无半截未消毒产物（译文为空）即清标记，
               留着会让后续水合写回的 DB 消毒译文走纯文本分支（字面显示 <p>…</p>，
               即 P2-9 标记粘连）；有半截则保留，按纯文本渲染（重试语义，D1b）。 */
            set((st) => {
              const nextRaw = { ...st.rawTranslatedIds };
              if (!effectiveAi(articleId, 'translatedContent')) delete nextRaw[articleId];
              return {
                translating: false,
                isShowingTranslatedProse: false,
                rawTranslatedIds: nextRaw,
                translateErrors: { ...st.translateErrors, [articleId]: msg },
              };
            });
            if (!silent) {
              get().showToast(`翻译失败：${msg}`, { label: '重试', run: () => get().toggleReaderTranslation() });
            }
          },
        )
        .catch(() => {
          /* F3：同上——清标记与「是否有未消毒半截」绑定（与 translateEntry 同口径） */
          set((st) => {
            const nextRaw = { ...st.rawTranslatedIds };
            if (!effectiveAi(articleId, 'translatedContent')) delete nextRaw[articleId];
            return {
              translating: false,
              isShowingTranslatedProse: false,
              rawTranslatedIds: nextRaw,
              translateErrors: { ...st.translateErrors, [articleId]: 'AI 服务未配置或不可达' },
            };
          });
          if (!silent) {
            get().showToast('翻译失败：请先在设置中配置 AI 服务', { label: '重试', run: () => get().toggleReaderTranslation() });
          }
        });
    },

    /** 按 id 流式生成摘要（增量落账，卡片实时打字机）。有缓存直接短路。
        失败记录到 summaryErrors[id]（卡片内联展示 + 重试依据）；重试前先清错误与半截文本。
        TASK-122：缓存判定/流式/落账全部经 bodyById 收口助手（见本文件头助手注释）。 */
    summarizeEntry: (id, opts) => {
      const silent = opts?.silent ?? false;
      const s = get();
      if (!s.entries.some((a) => a.id === id)) return;
      /* 已有缓存 → 直接短路（ai_summarize 后端也会短路，这里前端提前判断）。
         失败态除外：摘要流先产出半截文本再报错时，若在这里 return，toast 的
         「重试」就是死按钮——不重发 ai_summarize、summaryErrors 也不清，卡片
         永久停在与错误并存的半截摘要上（D1a）。 */
      if (effectiveAi(id, 'aiSummary') && !s.summaryErrors[id]) {
        return;
      }
      if (s.dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      /* 重试语义：清掉上次的错误与半截摘要，重新走完整流 */
      writeAiProduct(id, 'aiSummary', '');
      set((st) => ({
        summarizingIds: { ...st.summarizingIds, [id]: true },
        summaryErrors: { ...st.summaryErrors, [id]: '' },
      }));
      void api
        .aiSummarize(
          Number(id),
          (delta) => {
            streamAiDelta(id, 'aiSummary', delta);
          },
          () =>
            set((st) => {
              const nextIds = { ...st.summarizingIds };
              delete nextIds[id];
              return { summarizingIds: nextIds };
            }),
          (msg) => {
            /* 内联错误（卡片上直接可见）+ 非 silent 时 toast 带重试 */
            set((st) => {
              const nextIds = { ...st.summarizingIds };
              delete nextIds[id];
              return { summarizingIds: nextIds, summaryErrors: { ...st.summaryErrors, [id]: msg } };
            });
            if (!silent) {
              get().showToast(`摘要失败：${msg}`, { label: '重试', run: () => get().summarizeEntry(id) });
            }
          },
        )
        .catch(() => {
          set((st) => {
            const nextIds = { ...st.summarizingIds };
            delete nextIds[id];
            return {
              summarizingIds: nextIds,
              summaryErrors: { ...st.summaryErrors, [id]: 'AI 服务未配置或不可达' },
            };
          });
          if (!silent) {
            get().showToast('摘要失败：请先在设置中配置 AI 服务', { label: '重试', run: () => get().summarizeEntry(id) });
          }
        });
    },

    triggerReaderSummary: (opts) => {
      const id = get().activeArticleId;
      if (!id) return;
      get().summarizeEntry(id, opts);
    },
  });
};
