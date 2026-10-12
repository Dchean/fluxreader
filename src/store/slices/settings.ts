import type { StateCreator } from 'zustand';
import { api, extractError } from '../../lib/api';
import { STARTUP_VIEWS, type ViewFilterType } from '../../types';
import { appStore } from '../internals';
import { isValidSetting, sanitizeSettingsPatch } from '../settingsValidation';
import type { AppState, SettingsState } from '../types';

/** 设置 slice：设置项初值、乐观更新与启动恢复。
 *
 *  Pick 的键集即本 slice 的全部键；与其它 slice 两两不相交（合起来 = 原 useAppStore 全集）。
 */
export type SettingsSlice = Pick<
  AppState,
  | 'settings'
  | 'updateSettings'
  | 'bootstrapSettings'
>;

/* ============================================================
OPT-008B 设置持久化协议（R1 顺序与确认态；R2 接力；R3 读凭据；R4 时序）；
R5 机制收敛：**基线 + 每笔已校验 patch** 生成载荷，意图按序，UI 由投影得出

缺陷链（R5 关闭的三个主控真实反例）：
- 反例一：门后 A/B 两笔点击，B 的键被远端新值覆盖时载荷回退用的是「乐观前
  旧本值」——A 的载荷把刚导入的远端键打回旧本地；
- 反例二：回读失败后的校准只覆盖门后写——窗口外普通点击仍以旧全量快照覆盖
  未读回的远端键；
- 反例三：门释放后 A 在网络在途、C 点击「后于门」——B 的发送载荷把 C 的未落定
  意图偷带提交（C 失败后 DB/UI 停在 C 的值）。
共同根：**用「当前乐观全量快照」当每笔载荷**。R5 不再在 gated/非 gated 上各补
特判，统一为：

- 确认基线 base：后端 app_settings 原文（新鲜读取 / 应用回读 / 有界校准可刷新；
  写成功后按提交序推进为该笔 payload）。`appSettingsBaseKnown` 区分「已知」
  （含已知为空）与「未知」（从未读到 / 应用回读失败）。
- 待写意图 intents：按发起序排队的 `{seq, patch, prev}`（patch=该笔已校验键值，
  prev=乐观前本值仅供显示回退）。
- **每笔发送载荷 = base 解析对象 + 本笔 patch**（发送时生成，不携带任何其它笔
  意图；base 未知或为空时也绝不回退到旧全量快照）。
- **乐观 UI = 当前值叠 base + 按序叠仍有效意图**（`projectSettings`）——回读
  落地/校准/写落定都走同一投影；失败意图出队并按其键回投到 base（无 base 键时
  回 prev），不残留「永远成功」的乐观值。
- 发送前若 base 未知：一次**有界校准读取**，成功即以原文为底；失败→本笔不发送
  （可见失败、不改库、该笔键回显），持续失败由下一次成功读取/重试恢复。
- 写入串行（后写永不先发，A 网络在途时后发请求排队）；成功推进 base 并按本笔
  patch 是否含 smartDedup 触发 reload；失败如实 toast（每笔一次）。
- 应用边界（runAppSettingsApplyBoundary）：窗口内点击请求排在应用+回读之后
  （乐观落地不受影响）；回读落地=新 base，窗口内点击键由投影自然保留（无需
  专门保留集合）；上传只等发起时前沿（awaitAppSettingsWriteFrontier），不设门。
- 读取凭据（R3/R4 保留）：写序号 + 已发出未落定写数 + 读取发起序号（后发起者
  优先）；bootstrap 先等捕获时写前沿落定（不依赖 IPC FIFO）。
============================================================ */

let appSettingsWriteChain: Promise<unknown> = Promise.resolve();
/** 写入发起序号（每笔 updateSettings 推进）——意图序与读取凭据的比较基准。 */
let appSettingsWriteSerial = 0;
/** 已实际发出、尚未落定的写请求数（读取凭据：发起/落地两侧都要求 0）。 */
let appSettingsSentInFlight = 0;
/** 读取发起序号（bootstrap / 应用回读 / 校准共用）——后发起者优先：更早的在途
    读取落地即过期，不得倒灌更晚已被接受的基线。 */
let appSettingsReadSerial = 0;
/** 确认基线原文（后端 app_settings；写成功推进 / 读取与校准刷新）。 */
let confirmedAppSettings: string | null = null;
/** 基线是否已知：false = 从未读到或应用回读失败——发送前必须校准，
    校准失败则不发送（不以旧全量覆盖未读回的远端键）。 */
let appSettingsBaseKnown = false;
/** 基线最近一次更新时对应的写入发起序号（noteConfirmedAppSettings 的迟到守卫）。 */
let confirmedAtWriteSerial = 0;
/** 配置应用写边界（最新一个）：非空时此后发出的设置请求等待其释放（乐观落地
    不受影响）；只等待进入时已存在的链尾与前一屏障——不等待后来写，无自锁。 */
let appSettingsApplyGate: Promise<void> | null = null;
/** 单飞共享读取：并发的 bootstrap/校准复用同一在途 `get_setting`（各自凭据判定
    接受与否）；readback 为「应用后的新鲜读取」，强制发起并替换共享槽。
    每条共享读取同时绑定**发起时**的写序号与应用代次——跨提交（新写/新应用）
    后到达的等待者不得把它当作新鲜读取（R6：读取凭据必须绑定原始发起时世代）。 */
let sharedReadPromise: Promise<string | null> | null = null;
let sharedReadToken = 0;
let sharedReadIssuedSerial = 0;
let sharedReadIssuedEpoch = 0;
/** 应用代次：应用回读开始 / 基线被判定不可信时推进——使更早发起的共享读取
    对后续等待者失效（不得 join 为新鲜）。 */
let appSettingsAppliedEpoch = 0;

interface SharedRead {
  promise: Promise<string | null>;
  token: number;
  issuedSerial: number;
  issuedEpoch: number;
}

/** 读取一次 app_settings（单飞）：force=true 时发起新读取并替换共享槽。
    返回 promise 与凭据（读取序号 + **发起时**写序号/应用代次）——读取序号的
    推进只发生在**新发起**的读取上（join 不占号，避免把同一次读取的等待者互相
    判过期）；跨提交的等待者必须自行 force（见 bootstrap/calibrateBase）。 */
function readRawShared(force = false): SharedRead {
  if (!force && sharedReadPromise) {
    return {
      promise: sharedReadPromise,
      token: sharedReadToken,
      issuedSerial: sharedReadIssuedSerial,
      issuedEpoch: sharedReadIssuedEpoch,
    };
  }
  const token = ++appSettingsReadSerial;
  const issuedSerial = appSettingsWriteSerial;
  const issuedEpoch = appSettingsAppliedEpoch;
  const promise = api.getSetting('app_settings');
  sharedReadPromise = promise;
  sharedReadToken = token;
  sharedReadIssuedSerial = issuedSerial;
  sharedReadIssuedEpoch = issuedEpoch;
  void promise.then(
    () => { if (sharedReadPromise === promise) sharedReadPromise = null; },
    () => { if (sharedReadPromise === promise) sharedReadPromise = null; },
  );
  return { promise, token, issuedSerial, issuedEpoch };
}

/** 有界校准（base 未知时调用）：只复用**同一应用代次**内发起的在途读取；
    否则/仍未知则强制新鲜读取（至多两轮）。成功置 base 返回 true；失败/持续被
    更新读取取代返回 false（调用方不发送本笔，可见失败、不改库）。 */
async function calibrateBase(): Promise<boolean> {
  if (appSettingsBaseKnown) return true;
  for (let i = 0; i < 2; i += 1) {
    let read = readRawShared(false);
    if (i > 0 || read.issuedEpoch !== appSettingsAppliedEpoch) {
      read = readRawShared(true); // 跨应用代次的在途读取不得当新鲜
    }
    let raw: string | null;
    try {
      raw = await read.promise;
    } catch {
      return false;
    }
    if (appSettingsBaseKnown) return true; // 更晚读取已置基线
    if (appSettingsReadSerial === read.token) {
      try {
        acceptBase(raw, { applyViewFilters: false });
      } catch {
        return false; // 原文损坏：不发送（可见失败由调用方呈现）
      }
      return true;
    }
  }
  return false;
}
/** 待写意图（发起序）：乐观 UI 投影与失败回投的唯一事实来源。 */
interface SettingsWriteIntent {
  seq: number;
  patch: Record<string, unknown>;
  /** 乐观落地前的本值（仅显示回退用；载荷生成绝不使用——R5 反例一根因）。 */
  prev: Record<string, unknown>;
}
let pendingIntents: SettingsWriteIntent[] = [];

/** 显式登记读取到的 app_settings 原文（测试/外部接受路径）。
    仅当「确认基线之后没有发起过任何本地写」时接受——不能让迟到的旧原文
    回退已被更新写推进的确认态；读取路径走带凭据的 accept/校准。 */
export function noteConfirmedAppSettings(raw: string | null): void {
  if (appSettingsWriteSerial !== confirmedAtWriteSerial) return;
  confirmedAppSettings = raw;
  appSettingsBaseKnown = true;
  confirmedAtWriteSerial = appSettingsWriteSerial;
}

/** 测试专用：清空持久化链/确认基线与待写意图（避免用例间串扰）。 */
export function resetAppSettingsWriteStateForTests(): void {
  appSettingsWriteChain = Promise.resolve();
  appSettingsWriteSerial = 0;
  appSettingsSentInFlight = 0;
  appSettingsReadSerial = 0;
  confirmedAppSettings = null;
  appSettingsBaseKnown = false;
  confirmedAtWriteSerial = 0;
  appSettingsApplyGate = null;
  sharedReadPromise = null;
  sharedReadToken = 0;
  sharedReadIssuedSerial = 0;
  sharedReadIssuedEpoch = 0;
  appSettingsAppliedEpoch = 0;
  pendingIntents = [];
}

/** 基线原文 → 已校验键值对象（非法键丢弃；含 maxWidth 旧默认迁移；损坏原文
    视为空对象——base 已知为空与损坏都只影响该笔载荷的底，不回退旧全量）。 */
function parseSettingsRaw(raw: string): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  let saved: Partial<SettingsState>;
  try {
    saved = JSON.parse(raw) as Partial<SettingsState>;
  } catch {
    return out;
  }
  for (const [k, v] of Object.entries(saved)) {
    if (isValidSetting(k, v)) out[k] = v;
  }
  if (out.maxWidth === 760) out.maxWidth = 860; // v0.10.x 旧默认迁移
  return out;
}

/** 乐观 UI 投影：当前值叠 base（已知时）+ 按序叠仍有效意图。
    回读/校准/写落定/失败回投都经本函数——同一份「基线+意图」事实，UI 不再由
    某次快照整包覆写。 */
function projectSettings(): void {
  const store = appStore();
  const next = { ...(store.getState().settings as unknown as Record<string, unknown>) };
  if (appSettingsBaseKnown && confirmedAppSettings) {
    Object.assign(next, parseSettingsRaw(confirmedAppSettings));
  }
  for (const intent of pendingIntents) Object.assign(next, intent.patch);
  store.setState({ settings: next as unknown as SettingsState });
}

/** 接受一份基线原文（启动读取 / 应用回读 / 有界校准共用）：置 base 并投影
    （仍有效意图的键值被保留——窗口内点击/校准期间的点击不丢）。 */
function acceptBase(raw: string | null, opts?: { applyViewFilters?: boolean }): void {
  if (raw) JSON.parse(raw); // 损坏原文不进入接受（由调用方 catch 呈现）
  confirmedAppSettings = raw;
  appSettingsBaseKnown = true;
  confirmedAtWriteSerial = appSettingsWriteSerial;
  projectSettings();
  if (opts?.applyViewFilters && raw) {
    const store = appStore();
    const saved = JSON.parse(raw) as Partial<SettingsState>;
    const view = saved.startupView;
    const validView = typeof view === 'string' && STARTUP_VIEWS.includes(view);
    store.setState((s) => ({
      activeViewFilter: validView ? (view as ViewFilterType) : s.activeViewFilter,
      timelineFilter: saved.hideReadOnStartup === false ? 'all' : 'unread',
    }));
  }
}

/** 该笔发送载荷：base 解析对象 + 本笔 patch（不携带任何其它笔意图；
    base 未知/为空时不回退旧全量）。 */
function buildIntentPayload(intent: SettingsWriteIntent): string {
  const baseObj = appSettingsBaseKnown && confirmedAppSettings ? parseSettingsRaw(confirmedAppSettings) : {};
  return JSON.stringify({ ...baseObj, ...intent.patch });
}

/** 意图出队（不碰 UI）。 */
function removeIntent(intent: SettingsWriteIntent): void {
  pendingIntents = pendingIntents.filter((i) => i !== intent);
}

/** 失败/未发送意图的显示回投：该笔键回到 base 值（无 base 键时回 prev）；更晚
    未落定意图触及的键**不直接改 UI**，但把稳定底值下传给它们（其 prev 重写为
    该底值）——更晚意图再失败时回投到底值，而不是"失败笔乐观后的中间值"
    （R6：空/partial 基线下的同键连续失败不得经后笔 prev 残留）。随后整体投影。 */
function revertIntent(intent: SettingsWriteIntent): void {
  const store = appStore();
  const later = pendingIntents.filter((i) => i !== intent && i.seq > intent.seq);
  const laterKeys = new Set(later.flatMap((i) => Object.keys(i.patch)));
  const baseObj = appSettingsBaseKnown && confirmedAppSettings ? parseSettingsRaw(confirmedAppSettings) : null;
  const next = { ...(store.getState().settings as unknown as Record<string, unknown>) };
  for (const k of Object.keys(intent.patch)) {
    const floor = baseObj && k in baseObj ? baseObj[k] : intent.prev[k];
    if (laterKeys.has(k)) {
      for (const w of later) {
        if (k in w.patch) w.prev[k] = floor; // 稳定底值下传（覆盖中间乐观值）
      }
      continue;
    }
    next[k] = floor;
  }
  removeIntent(intent);
  store.setState({ settings: next as unknown as SettingsState });
  projectSettings();
}

/** 上传等「读取已保存配置」前的前沿等待：只等**捕获时**的写链尾（发起前已
    在途/排队的设置写真都落定），不设门——网络往返不阻塞之后任何设置操作。 */
export async function awaitAppSettingsWriteFrontier(): Promise<void> {
  await appSettingsWriteChain.catch(() => undefined);
}

/** 配置应用写边界（用户确认后调用）：同步捕获「前一边界 + 当时的写链尾」并
    发布 own gate——此后发出的 updateSettings 请求排在应用之后（乐观落地不受
    影响），载荷在发送时从「base + 本笔 patch」生成。成功/失败都在 finally 释放
    自己的门；清门只在自己仍是尾时发生（后发边界的门不被旧边界打开）。 */
export async function runAppSettingsApplyBoundary<T>(fn: () => Promise<T>): Promise<T> {
  const prevGate = appSettingsApplyGate;
  const frontier = appSettingsWriteChain;
  let release!: () => void;
  const ownGate = new Promise<void>((resolve) => { release = resolve; });
  appSettingsApplyGate = ownGate;
  try {
    await Promise.all([prevGate ?? Promise.resolve(), frontier.catch(() => undefined)]);
    return await fn();
  } finally {
    if (appSettingsApplyGate === ownGate) appSettingsApplyGate = null;
    release();
  }
}

/** 配置应用后的 app_settings 真实回读（应用边界内调用）：后端应用已提交，
    读取结果成为新基线；**强制新鲜读取**（不复用应用前在途的读取，并推进应用
    代次使更早的共享读取对后续等待者失效）；窗口内点击由投影保留。读取同样
    占用读取凭据（后发起者优先）。 */
export async function readBackAppliedAppSettings(): Promise<void> {
  appSettingsAppliedEpoch += 1; // 应用代次推进：更早发起的在途读取不得被 join 为新鲜
  const read = readRawShared(true);
  const raw = await read.promise;
  if (appSettingsReadSerial !== read.token) return; // 更晚的读取已发起：本读取过期
  acceptBase(raw, { applyViewFilters: true });
}

/** 应用已提交但设置回读失败：基线未知（不假称可恢复）——此后每笔写发送前
    必须校准；校准失败则不发送（不以旧全量覆盖未读回的远端键）。同时推进应用
    代次：更早发起的在途读取不得对后续等待者冒充新鲜。 */
export function markAppSettingsUnconfirmed(): void {
  confirmedAppSettings = null;
  appSettingsBaseKnown = false;
  confirmedAtWriteSerial = appSettingsWriteSerial;
  appSettingsAppliedEpoch += 1;
}

export const createSettingsSlice: StateCreator<AppState, [], [], SettingsSlice> = (set, get) => ({
  settings: {
    autoRefresh: true,
    refreshInterval: 30,
    fetchConcurrency: 4,
    markReadOnOpen: true,
    markReadOnScrollBottom: false,
    markReadOnScrollOut: false,
    autoStart: false,
    startupView: 'unread',
    hideReadOnStartup: true,
    themeMode: 'dark',
    palette: 'blue',
    fontFamily: '"Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif',
    fontSize: 16,
    lineHeight: 180,
    maxWidth: 860,
    listWidth: 320,
    showReadTime: true,
    defaultOpenMode: 'rss',
    smartDedup: false,
    closeToTray: true,
    closePromptShown: false,
    notifyOnNewArticles: false,
    autoSync: true,
    syncMode: 'direct',
  },

  updateSettings: (partial) => {
    /* D5：写入前做运行时校验（类型 + 取值域），与读回路径 bootstrapSettings
       共用同一张校验表。非法键被丢弃：不进内存、不落库、也不会再出现
       「写得进去但读回来被丢弃」的不对称。范围约束不再只存在于组件的 range。 */
    const accepted = sanitizeSettingsPatch(partial as Record<string, unknown>);
    /* 全部非法 → 纯 no-op：不 set（不产生无意义的重渲染），也不发落库 IPC */
    if (Object.keys(accepted).length === 0) return;
    const gateAtIssue = appSettingsApplyGate;
    const prev: Record<string, unknown> = {};
    {
      const cur = get().settings as unknown as Record<string, unknown>;
      for (const k of Object.keys(accepted)) prev[k] = cur[k];
    }
    /* 乐观应用到内存（保留既有交互手感）；持久化按下方串行链落库。 */
    set((s) => ({ settings: { ...s.settings, ...accepted } }));
    if (!(typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window)) return;
    /* R5：意图登记（patch = 本笔已校验键值；prev 仅显示回退用）。每笔载荷在
       发送时由「base + 本笔 patch」生成——不携带任何其它笔意图。 */
    const serial = ++appSettingsWriteSerial;
    const intent: SettingsWriteIntent = { seq: serial, patch: accepted as Record<string, unknown>, prev };
    pendingIntents.push(intent);
    const dedupChanged = 'smartDedup' in accepted;
    appSettingsWriteChain = appSettingsWriteChain
      .then(async () => {
        if (gateAtIssue) {
          await gateAtIssue;
        }
        /* R5（反例二）：base 未知（从未读到 / 应用回读失败）→ 发送前**有界
           校准**（优先复用/等待在途读取，至多一次强制新鲜读）：失败则本笔不
           发送（返回 null 哨兵，可见失败、不改库、不以旧全量覆盖远端键）。 */
        if (!appSettingsBaseKnown && !(await calibrateBase())) return null;
        const payload = buildIntentPayload(intent);
        appSettingsSentInFlight += 1;
        try {
          await api.setSetting('app_settings', payload);
        } finally {
          appSettingsSentInFlight = Math.max(0, appSettingsSentInFlight - 1);
        }
        return payload;
      })
      .then((payload) => {
        if (payload === null) {
          /* 校准失败：未发送。该笔键回投（base 值优先，无 base 键回 prev；
             更晚意图键跳过）——不残留「永远成功」的乐观值。 */
          revertIntent(intent);
          get().showToast('设置未保存：配置基线未确认（应用回读失败），请稍后重试');
          return;
        }
        /* 成功：意图出队 → 该笔 payload 成为新基线（链上提交序推进；接力场景
           旧笔成功也被记住）→ 整体投影（base + 剩余意图）。 */
        removeIntent(intent);
        confirmedAppSettings = payload;
        appSettingsBaseKnown = true;
        confirmedAtWriteSerial = serial;
        projectSettings();
        /* 本笔含 smartDedup 变更 → 按新口径重取（不被后续非 dedup 写吞掉）。
           reload 自身失败已有可见提示，这里再兜 catch 避免未处理拒绝。 */
        if (dedupChanged) {
          void get().reloadFromBackend().catch(() => { /* 失败可见性由 reload 内部负责 */ });
        }
      })
      .catch((e: unknown) => {
        /* 失败：该笔键回投到 base（无 base 键回 prev），如实 toast——每笔失败
           都可见；更晚意图不受牵连（各自的 patch 由投影保留）。 */
        revertIntent(intent);
        get().showToast(`设置保存失败：${extractError(e)}`);
      });
  },

  /** 启动时从后端恢复设置；并应用 startupView / hideReadOnStartup。
      R4：先等**捕获时**的写前沿落定（读取请求晚于这些写发出，不依赖 IPC FIFO
      顺序；写失败由其自身路径回滚/提示，读取随后取得的是回滚后的真值）。
      随后取读取凭据——读取期间的新写（写序号推进）或更晚发起的读取（读取
      序号推进）使本次读取过期：不回退 UI、不替换基线。无写入交错时正常接受
      并以后端原文成为新基线（外部配置/手改 DB 的新鲜读取由此可被接受）。 */
  bootstrapSettings: async () => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    const frontier = appSettingsWriteChain;
    try {
      await frontier;
    } catch {
      /* 链上错误已在各笔 catch 消化；兜底不阻塞读取 */
    }
    const startSerial = appSettingsWriteSerial;
    const startSentInFlight = appSettingsSentInFlight;
    const startEpoch = appSettingsAppliedEpoch;
    let read = readRawShared();
    /* R6：在途共享读取若发起于更早的写/应用代次，不得当作本次的新鲜读取
       （跨提交不得 join）——强制发起一次新鲜读取。 */
    if (read.issuedSerial !== startSerial || read.issuedEpoch !== startEpoch) {
      read = readRawShared(true);
    }
    try {
      const raw = await read.promise;
      const fresh = appSettingsWriteSerial === startSerial
        && startSentInFlight === 0
        && appSettingsReadSerial === read.token
        && read.issuedSerial === startSerial
        && read.issuedEpoch === startEpoch;
      if (fresh) {
        acceptBase(raw, { applyViewFilters: true });
        return;
      }
      /* 读取被期间写入作废：不替换基线/UI，但等这些写落定后再返回——调用方
         观察到的是与后端一致的终态（而非写入半程的中间值）。 */
      await appSettingsWriteChain.catch(() => undefined);
    } catch (e) {
      console.error('restore settings failed:', e);
    }
  },
});
