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

/* ============================================================
   OPT-013 B1：请求代次与在途表（模块级；不承载可渲染状态，无需进 store 快照）

   Note: 命中判定取后端权威（无签名/过期产物不得短路）、代次/在途守卫与
   回读时序契约 — 见 .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md
   末节「OPT-013 B1 追加（2026-10-15）」。

   - 代次：每个 (条目, 产物) 发起请求时自增。delta / Done / getArticle 安全回读
     全部携带发起时代次，落地时不等即整体丢弃——过时回包不得覆盖较新成功结果，
     也不得清除较新请求的标记（rawTranslatedIds 等）。
   - 在途：同一 (条目, 产物) 至多一个在途生成请求。自动触发（分类/源
     autoSummary/autoTranslate）与手动入口（按钮/右键/重试）都先查此表，
     命中即复用（不重复计费）；不同条目/不同产物互不阻塞。

   模块级而非 store 状态：代次/在途是发送侧的记账，不参与渲染；进快照会让
   每次请求自增触发无谓重渲染。测试经 resetAiRequestStateForTests 复位
   （harness 的 resetStore 调用——跨夹具不残留死挂起的在途项）。
   ============================================================ */
type AiProductKind = 'summary' | 'translation';

const aiRequestGen = new Map<string, number>();
const aiInFlight = new Map<string, Promise<unknown>>();
const aiKeyOf = (id: string, kind: AiProductKind): string => `${id}:${kind}`;

/** 请求发起：代次自增并返回本次代次（此后所有回包都按它守卫）。 */
function beginAiRequest(key: string): number {
  const gen = (aiRequestGen.get(key) ?? 0) + 1;
  aiRequestGen.set(key, gen);
  return gen;
}

/** 该代次是否仍是最新——旧代次的 delta/Done/回读一律丢弃。 */
function isCurrentAiRequest(key: string, gen: number): boolean {
  return aiRequestGen.get(key) === gen;
}

/** 登记在途请求：settle（含成功/失败/取消）即摘除；只摘自己那一条。 */
function trackAiRequest(key: string, p: Promise<unknown>): void {
  aiInFlight.set(key, p);
  void p.finally(() => {
    if (aiInFlight.get(key) === p) aiInFlight.delete(key);
  });
}

/** 测试专用：清空代次与在途表（frontend-regression 的夹具复位；生产代码不得调用） */
export function resetAiRequestStateForTests(): void {
  aiRequestGen.clear();
  aiInFlight.clear();
}

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

  const writeAiProduct = (
    id: string,
    field: 'aiSummary' | 'translatedContent',
    value: string,
    signature?: string | null,
  ): void => {
    if (applyAiProduct(id, field, value, signature)) return;
    const sigPatch = signature === undefined
      ? {}
      : (field === 'aiSummary' ? { aiSummarySig: signature } : { translatedSig: signature });
    set((st) => ({ entries: st.entries.map((a) => (a.id === id ? { ...a, [field]: value, ...sigPatch } : a)) }));
  };

  /** OPT-013 B1：缓存命中判定（后端权威）。
   *
   *  签名输入（模型/endpoint/提示词/正文哈希/修订/分段策略）只有后端在同一份
   *  配置快照下能算——前端不再以「产物非空」直接短路，判定在动作时刻经
   *  ai_cache_check 取回：true = 产物非空且签名匹配当前输入，复用展示；
   *  false = 未生成/无签名（存量产物）/已过期（换模型/提示词/正文修订）→ 调用
   *  方走重新生成（旧产物保留显示，「未标记版本」标记由行签名字段呈现）。
   *  校验本身失败（IPC 故障/未配置）按「不可验证」保守处理 = 不触发重新生成：
   *  校验故障不得把可用产物清空重烧（B3 再补「已过期」态与入口）。
   *  浏览器 mock 无 IPC（返回 null）→ 维持既有演示语义（按可复用展示）。 */
  const verifyCacheHit = async (id: string, kind: AiProductKind): Promise<boolean> => {
    try {
      const res = await api.aiCacheCheck(Number(id));
      if (!res) return true;
      return kind === 'summary' ? res.summaryVerified : res.translationVerified;
    } catch {
      return true;
    }
  };

  /** R1（复审 P2-1）：「验签 + 条件生成」整链登记进在途表。
   *
   *  修前只有 `run*` 内部登记在途，而入口闸门（`aiInFlight.has`）在验签**之前**
   *  检查——`ai_cache_check` 在途期间第二次触发同样通过闸门、同样拿到「未命中」，
   *  于是同一 `(条目, 产物)` 各发一笔生成（双倍计费）。现在整链可被登记：校验
   *  在途期间第二次触发即命中闸门复用（不重复校验、不重复生成）。
   *
   *  登记交接：未命中时 `run*` 在同步路径内以自身 promise 覆盖本链登记（两个
   *  finally 都按身份比对，先到的那个不会误删后登记的）；`run*` 之前的复查
   *  （`aiInFlight.get(key) !== chain`）只作防御——若窗口内已有更晚的链接管，
   *  本链放弃发起。 */
  const ensureAiProduct = (
    id: string,
    kind: AiProductKind,
    silent: boolean,
    run: (id: string, silent: boolean) => void,
  ): void => {
    const key = aiKeyOf(id, kind);
    let chain: Promise<void> = Promise.resolve();
    chain = (async () => {
      if (await verifyCacheHit(id, kind)) return; // 命中：复用展示，链结束
      if (aiInFlight.get(key) !== chain) return; // 防御：已被更新的在途接管
      run(id, silent);
    })();
    trackAiRequest(key, chain);
  };

  /* ---------- 生成流程（三个入口共用：请求代次 + 落地守卫） ---------- */

  /** 摘要生成/重试：清错误与半截 → 流式 → Done 后安全回读（含签名）。 */
  const runSummaryRequest = (id: string, silent: boolean): void => {
    const key = aiKeyOf(id, 'summary');
    const gen = beginAiRequest(key);
    const current = (): boolean => isCurrentAiRequest(key, gen);
    /* 重试语义：清掉上次的错误与半截摘要（产物清空则签名一并失效——签名不得
       脱离产物独立存活），重新走完整流 */
    writeAiProduct(id, 'aiSummary', '', null);
    set((st) => ({
      summarizingIds: { ...st.summarizingIds, [id]: true },
      summaryErrors: { ...st.summaryErrors, [id]: '' },
    }));
    const p = api
      .aiSummarize(
        Number(id),
        (delta) => {
          if (!current()) return; // 过时 delta：不得污染较新请求的产物
          streamAiDelta(id, 'aiSummary', delta);
        },
        () => {
          if (!current()) return;
          set((st) => {
            const nextIds = { ...st.summarizingIds };
            delete nextIds[id];
            return { summarizingIds: nextIds };
          });
          /* OPT-013 B1：Done 后安全回读 DB 真值——带回新产物对应的缓存签名
             （前端命中判定与「未标记版本」显示都以签名是否在为止）。行未见
             产物（mock/旧后端/竞态）时**保留已渲染文本**，不因回读缺行清掉
             刚生成的摘要（摘要无 XSS 消毒时序，缺行不构成渲染风险）。 */
          const readGen = gen;
          void api
            .getArticle(Number(id))
            .then((row) => {
              if (!isCurrentAiRequest(key, readGen)) return; // 逆序回读守卫
              const dbSummary = row?.ai_summary ?? null;
              if (dbSummary !== null && dbSummary.trim() !== '') {
                writeAiProduct(id, 'aiSummary', dbSummary, row?.ai_summary_sig ?? null);
              }
            })
            .catch(() => { /* 回读失败：保留流式产物，不置错误态（与生成成功无关） */ });
        },
        (msg) => {
          /* 内联错误（卡片上直接可见）+ 非 silent 时 toast 带重试 */
          if (!current()) return;
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
        if (!current()) return;
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
    trackAiRequest(key, p);
  };

  /** 卡片级翻译生成/重试（状态按条目隔离，不复用全局 translating 单布尔）。 */
  const runCardTranslation = (id: string, silent: boolean): void => {
    const key = aiKeyOf(id, 'translation');
    const gen = beginAiRequest(key);
    const current = (): boolean => isCurrentAiRequest(key, gen);
    /* 重试语义（与 toggleReaderTranslation 对齐）：清上次的错误**和半截译文**
       （产物清空则签名一并失效）。此前只清错误不清译文——重试成功后新 delta
       会追加在旧半截译文后面（打字机里出现「半截+完整」的重复内容）。 */
    writeAiProduct(id, 'translatedContent', '', null);
    set((st) => ({
      translatingIds: { ...st.translatingIds, [id]: true },
      /* TASK-065 N11：流式 delta 是模型原始输出（未消毒），标记期间渲染走纯文本 */
      rawTranslatedIds: { ...st.rawTranslatedIds, [id]: true },
      translateErrors: { ...st.translateErrors, [id]: '' },
    }));
    const p = api
      .aiTranslate(
        Number(id),
        (delta) => {
          if (!current()) return;
          streamAiDelta(id, 'translatedContent', delta);
        },
        () => {
          /* TASK-065 N11：流式结束回读 DB 的消毒版译文。translatingIds 维持既有
             时序（done 即清）；消毒时序由 rawTranslatedIds 承担——回读成功用
             消毒版覆盖后才清除（期间渲染按纯文本），失败则丢弃未消毒半截 + toast
             （此前无 .catch：半截未消毒译文永久留在渲染路径 + unhandled rejection）。
             OPT-013 B1：回读行的签名与产物成对落账。 */
          if (!current()) return;
          set((st) => {
            const nextIds = { ...st.translatingIds };
            delete nextIds[id];
            return { translatingIds: nextIds };
          });
          const readGen = gen;
          void api
            .getArticle(Number(id))
            .then((row) => {
              if (!isCurrentAiRequest(key, readGen)) return; // 逆序回读守卫：旧回包不得覆盖新结果
              const safe = row?.translated_content ?? '';
              set((st) => {
                const nextRaw = { ...st.rawTranslatedIds };
                delete nextRaw[id];
                return { rawTranslatedIds: nextRaw };
              });
              /* 回读不到消毒版（含 row 为空）：丢弃未消毒半截，不留 XSS 窗口 */
              writeAiProduct(id, 'translatedContent', safe, row?.translated_sig ?? null);
            })
            .catch(() => {
              if (!isCurrentAiRequest(key, readGen)) return;
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
          if (!current()) return;
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
        if (!current()) return;
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
    trackAiRequest(key, p);
  };

  /** Reader 级翻译生成/重试（复用全局 translating 单布尔 + 展示态切换）。 */
  const runReaderTranslation = (id: string, silent: boolean): void => {
    const key = aiKeyOf(id, 'translation');
    const gen = beginAiRequest(key);
    const current = (): boolean => isCurrentAiRequest(key, gen);
    /* 重试语义：清上次的错误与半截译文（签名随产物清空），重新走完整流 */
    writeAiProduct(id, 'translatedContent', '', null);
    set((st) => ({
      translating: true,
      isShowingTranslatedProse: true,
      /* TASK-065 N11：流式 delta 未消毒，标记期间 Reader 按纯文本渲染 */
      rawTranslatedIds: { ...st.rawTranslatedIds, [id]: true },
      translateErrors: { ...st.translateErrors, [id]: '' },
    }));
    const p = api
      .aiTranslate(
        Number(id),
        (delta) => {
          if (!current()) return;
          /* 增量追加：Reader 直接渲染译文（打字机） */
          streamAiDelta(id, 'translatedContent', delta);
        },
        () => {
          /* TASK-065 N11：流式结束回读 DB 的消毒版译文（后端 ai_translate 落库前
             已 sanitize，流中 delta 是未消毒原样）。translating 维持既有时序（done
             即清）；消毒时序由 rawTranslatedIds 承担——回读成功用消毒版覆盖后才
             清除并切 HTML 渲染；失败则丢弃未消毒半截 + toast（此前无 .catch：半截
             未消毒译文永久留在渲染路径 + unhandled rejection）。
             OPT-013 B1：回读行的签名与产物成对落账；过时回读整体丢弃。 */
          if (!current()) return;
          set({ translating: false });
          const readGen = gen;
          void api
            .getArticle(Number(id))
            .then((row) => {
              if (!isCurrentAiRequest(key, readGen)) return;
              const safe = row?.translated_content ?? '';
              set((st) => {
                const nextRaw = { ...st.rawTranslatedIds };
                delete nextRaw[id];
                return { rawTranslatedIds: nextRaw };
              });
              /* 回读不到消毒版（含 row 为空）：丢弃未消毒半截，不留 XSS 窗口 */
              writeAiProduct(id, 'translatedContent', safe, row?.translated_sig ?? null);
            })
            .catch(() => {
              if (!isCurrentAiRequest(key, readGen)) return;
              set((st) => {
                const nextRaw = { ...st.rawTranslatedIds };
                delete nextRaw[id];
                return {
                  rawTranslatedIds: nextRaw,
                  translateErrors: { ...st.translateErrors, [id]: '译文回读失败' },
                };
              });
              writeAiProduct(id, 'translatedContent', '');
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
          if (!current()) return;
          set((st) => {
            const nextRaw = { ...st.rawTranslatedIds };
            if (!effectiveAi(id, 'translatedContent')) delete nextRaw[id];
            return {
              translating: false,
              isShowingTranslatedProse: false,
              rawTranslatedIds: nextRaw,
              translateErrors: { ...st.translateErrors, [id]: msg },
            };
          });
          if (!silent) {
            get().showToast(`翻译失败：${msg}`, { label: '重试', run: () => get().toggleReaderTranslation() });
          }
        },
      )
      .catch(() => {
        /* F3：同上——清标记与「是否有未消毒半截」绑定（与 translateEntry 同口径） */
        if (!current()) return;
        set((st) => {
          const nextRaw = { ...st.rawTranslatedIds };
          if (!effectiveAi(id, 'translatedContent')) delete nextRaw[id];
          return {
            translating: false,
            isShowingTranslatedProse: false,
            rawTranslatedIds: nextRaw,
            translateErrors: { ...st.translateErrors, [id]: 'AI 服务未配置或不可达' },
          };
        });
        if (!silent) {
          get().showToast('翻译失败：请先在设置中配置 AI 服务', { label: '重试', run: () => get().toggleReaderTranslation() });
        }
      });
    trackAiRequest(key, p);
  };

  return ({
    summarizingIds: {},
    translating: false,
    summaryErrors: {},
    translateErrors: {},
    translatingIds: {},
    rawTranslatedIds: {},

    /** 卡片级翻译（社交/通知卡）：按 id 流式生成该条目译文，不依赖 Reader 选中态。
        状态按条目隔离（不复用全局 translating 单布尔，避免多卡互串）。 */
    translateEntry: (id, opts) => {
      const silent = opts?.silent ?? false;
      if (get().dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      if (!get().entries.some((a) => a.id === id)) return;
      const key = aiKeyOf(id, 'translation');
      /* OPT-013 B1：同一 (条目, 产物) 已有在途请求或验签链在途（自动触发/另一
         入口发起）→ 复用，不重复计费（R1 P2-1：整链登记，验签窗口也算在途）。 */
      if (aiInFlight.has(key)) return;
      /* 已有译文 → 命中判定必须验签名（后端权威）：确认可复用才短路（不重复
         烧 token）；无签名（存量产物）/签名不匹配（换模型/提示词/正文修订）
         → 走重新生成，旧产物保留显示、不冒充缓存。
         失败态必须放行：流先产出半截译文再报错时，这里必须真的重发请求——
         否则 toast 的「重试」是死按钮（D1b）。 */
      if (effectiveAi(id, 'translatedContent') && !get().translateErrors[id]) {
        ensureAiProduct(id, 'translation', silent, runCardTranslation);
        return;
      }
      runCardTranslation(id, silent);
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
      const key = aiKeyOf(articleId, 'translation');
      /* OPT-013 B1：同键在途（含验签链在途）→ 复用（自动触发已发起或另一入口
         点击中），只对齐展示态、不二次计费。 */
      if (aiInFlight.has(key)) {
        set({ isShowingTranslatedProse: true });
        return;
      }
      /* 已有缓存译文 → 命中判定验签名（后端权威）：命中则直接切换展示；
         未命中（无签名/已过期）→ 重新生成（旧产物保留显示，不冒充缓存）。
         失败态同样放行（D1b 同源）：流内先出半截译文再报错时，重试必须真的
         重发请求，而不是把半截译文当缓存展示。
         TASK-065 N11：未消毒的流式产物（rawTranslatedIds 未清）也不当缓存——
         切换展示会把它按 HTML 渲染进 DOM。
         TASK-122：缓存判定从 bodyById 生效值取（真值源迁移）。 */
      if (effectiveAi(articleId, 'translatedContent') && !s.translateErrors[articleId] && !s.rawTranslatedIds[articleId]) {
        set({ isShowingTranslatedProse: true });
        ensureAiProduct(articleId, 'translation', silent, runReaderTranslation);
        return;
      }
      /* 无缓存 → 流式生成（打字机效果落到译文） */
      if (s.dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      runReaderTranslation(articleId, silent);
    },

    /** 按 id 流式生成摘要（增量落账，卡片实时打字机）。有已验证缓存直接短路。
        失败记录到 summaryErrors[id]（卡片内联展示 + 重试依据）；重试前先清错误与半截文本。
        TASK-122：缓存判定/流式/落账全部经 bodyById 收口助手（见本文件头助手注释）。
        OPT-013 B1：缓存判定改为后端验签名（见 verifyCacheHit）——无签名/已过期
        产物不再短路，走重新生成覆盖。 */
    summarizeEntry: (id, opts) => {
      const silent = opts?.silent ?? false;
      const s = get();
      if (!s.entries.some((a) => a.id === id)) return;
      const key = aiKeyOf(id, 'summary');
      /* OPT-013 B1：同键在途（含验签链在途）→ 复用（自动触发/手动入口并发只发
         一次请求；R1 P2-1：验签窗口也算在途）。 */
      if (aiInFlight.has(key)) return;
      /* 已有缓存 → 命中判定验签名（后端权威）：命中即短路；
         失败态除外：摘要流先产出半截文本再报错时，若在这里 return，toast 的
         「重试」就是死按钮——不重发 ai_summarize、summaryErrors 也不清，卡片
         永久停在与错误并存的半截摘要上（D1a）。 */
      if (effectiveAi(id, 'aiSummary') && !s.summaryErrors[id]) {
        ensureAiProduct(id, 'summary', silent, runSummaryRequest);
        return;
      }
      if (s.dataMode !== 'tauri') {
        if (!silent) get().showToast('演示模式不支持 AI 服务');
        return;
      }
      runSummaryRequest(id, silent);
    },

    triggerReaderSummary: (opts) => {
      const id = get().activeArticleId;
      if (!id) return;
      get().summarizeEntry(id, opts);
    },
  });
};
