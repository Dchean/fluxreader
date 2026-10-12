// Note: AI 配置的「写入 × 快照对账」协议单点（OPT-011 R4）——
// 三态语义与 resolver 见 types.ts / selectors.ts；原子索引落地见 internals.ts；
// 后台配置读取（list_folders/list_feeds）的接受点接线见 slices/bootstrap.ts。
//
// 分开管理三种状态（《AI配置写入与快照对账》提案落地）：
// - 确认态 confirmed：最近一次可视为「后端已持久化」的值。来源：写请求回应成功
//   （回应成功按既有 IPC 契约视为已提交），或通过凭据比较的配置读取快照。
// - 待写意图 intents：按键有序的本地写入意图（乐观值），随请求发出入列、落定
//   （成功/失败）出列。键的可视值 = 最新未落定意图 ?? 确认态。
// - 读取凭据：beginConfigRead() 必须在 list_folders/list_feeds 发出**前**捕获；
//   applyConfigRead() 接受快照时凭它判定每个键的快照值是否可用。
//
// 新鲜度规则（只用逻辑序号，不用墙钟；不声称纯前端序号提供 DB 线性一致性）：
// - 写成功晚于读取起点（lastSuccessSeq > 读取起点）→ 该快照对该键是旧数据，
//   不得把已成功的较新确认值倒灌回旧值；
// - 更早起点、更晚接受的读取不得覆盖更晚接受的读取（lastAcceptedReadSeq）；
// - 接受快照时保留未落定意图的可视值（尤其读取发起后的新点击）；
// - 权威快照（未过时）确认实体缺失 → 确认态置空（脱链）：失败不回写、不声称
//   已恢复；实体由后续接受点按新值重新接链；
// - 写回应顺序 ≠ DB 提交时刻：若确切提交先后无法从现有接口证明，不做推断、
//   不用全量 reload 覆写 pending；需要时按提案用有界新鲜确认（现有只读 API、
//   保护期间新意图）。
//
// 生命周期：意图随落定出列；状态与队列按会话存活（键数有界，会话内不回收）——
// 队列绝不在有在途请求时更换，任何清理都不会让同键旧新请求并行。空闲期的
// 「外部漂移」自愈：无未落定意图时若写入入口读到的当前值与确认态不一致
// （测试夹具直接 seed categories / mock 装载等未经接受点的替换），以当前值为
// 确认态——否则陈旧确认态会污染新夹具；有意图在途时不采纳（那正是协议的
// 「已确认值」，不得被乐观值覆盖）。stale（配置应用回读失败标记，见下）的键
// 不参与自愈——它的确认态不可信，须由成功写或成功读取重接。
//
// R5 扩展（**所有实际 AI 写入口**接入同一协议）：
// - 编辑（editFeed 含 AI 更改）是**一笔多键操作**：issueAiMultiWrite 只登记被
//   改的 AI 键、一次原子乐观落地、完整 update_feed 只发一次，请求排在涉及键
//   队列尾之后（同域有序、跨域独立）；不含 AI 更改的纯元数据编辑不触碰 AI 键。
// - 配置应用写屏障 runConfigApplyBarrier（下载并应用，用户确认后进入）：先同步
//   捕获「进入时已存在的写前沿」并设门，等前沿落定后执行 fn；设门后发出的 AI
//   写乐观可见、请求排在门后（应用之后）——屏障不等待后来写，无自锁；成功/
//   失败都在 finally 释放，错误不吞。
// - 上传前沿等待 awaitAiWriteFrontier：只等捕获时的在途/已排队写（快照不漏
//   已点写），不设门——上传的网络往返不阻塞之后任何 AI 操作。
// - 配置应用回读失败 markAiConfigUnconfirmed：应用已整包覆写后端 AI 配置而本地
//   没有读回实际值——受影响键的确认态标记为不可信（stale）、清空确认值；后续
//   失败不再声称「已恢复原值」（不假称），由下一次成功写/成功读取重接。

import type { CategoryGroup, FeedAiMode, FeedItem } from '../types';
import { normalizeFeedAiMode } from '../types';
import { appStore } from './internals';
import { extractError } from '../lib/api';

/** 协议逻辑序号：读取起点与写成功事件共用（只做先后比较）。 */
let seq = 0;
/** 最近一次被接受的读取起点序号——更早的读取不得覆盖它。 */
let lastAcceptedReadSeq = 0;

/** 读取凭据：beginConfigRead 在 list_folders/list_feeds 发出前捕获。
    读取失败或查询代际淘汰 = 不调用 applyConfigRead——凭据为纯值，无需清理。 */
export interface AiConfigReadCredential {
  startSeq: number;
}

interface AiKeyState {
  kind: 'feed' | 'cat';
  id: string;
  field: 'summary' | 'translate';
  /** 最近可确认的后端值；undefined = 确认态不可用（脱链 / stale 未确认） */
  confirmed: unknown;
  /** 未落定写意图（按发出顺序）；可视值 = 末位意图 ?? confirmed */
  intents: Array<{ value: unknown }>;
  /** 最近写成功事件的序号（读取接受时判定该键快照是否已过时） */
  lastSuccessSeq: number;
  /** 确认值被显式标记为不可信（配置应用回读失败）——不参与空闲自愈，
      须由成功写或成功读取重接。 */
  stale: boolean;
  /** 可视值落地（乐观 / 失败恢复 / 成功对齐共用；由写入口按当前实体闭包） */
  apply: (value: unknown) => void;
  label: string;
}

const states = new Map<string, AiKeyState>();
/** per-键 wire 队列：同键严格串行、跨键独立；会话内存活（键数有界）。 */
const queues = new Map<string, Promise<unknown>>();

/** 配置应用写屏障（OPT-011 R5/R6）：非空 = **最新一个**「下载并应用」的
    own gate——在其发布之后发出的新 AI 写请求等待该屏障释放才发送（乐观落地
    不受影响）。屏障只等待**进入时已存在**的写前沿与前一屏障；之后发出的写
    只被门控、不被等待——不动态等待依赖本屏障的请求，无自锁。
    多屏障语义（OPT-011 R6：组件的 busy 是本地 state——切 tab 卸载重挂载后
    可再发起，屏障安全不依赖组件挂载/busy 存活）：入口同步捕获「前一屏障的
    gate + 当时的写前沿」，等二者落定后执行 fn（后发屏障不落后于前发屏障）；
    门清空有所有权：finally 只在自己仍是尾（writeGate === ownGate）时清空——
    后发屏障已发布时不得被旧屏障清掉（否则旧屏障完成会打开新屏障的门，新
    屏障在途时发出的写漏过门、被整包应用覆盖）；own gate 始终释放（成功/
    失败一致）。无自锁核对（两屏障之间已排一笔 AI 写 W）：W 等 gateA；B 等
    [gateA, frontier（含 W 的尾）]；A 不等 B/W——拓扑序 A 先于 W 先于 B，无环。 */
let writeGate: Promise<void> | null = null;

/** 同步捕获当前写前沿：各键队列尾（已在途 + 已排队，未落定与已落定均兼容）。
    调用与设门之间无异步窗口（同一同步块内），不漏任何时点上的在途写。 */
function captureWriteFrontier(): Promise<unknown> {
  return Promise.all([...queues.values()].map((p) => p.catch(() => undefined)));
}

/** 上传等「读取已保存配置」前的前沿等待：让发起时已存在的 AI 写落定
    （成功/失败都算落定；失败说明该值本就不在库中）。不设门、不登记任何
    在途标记——上传的网络往返不阻塞之后任何操作。 */
export async function awaitAiWriteFrontier(): Promise<void> {
  await captureWriteFrontier();
}

/** 受控写屏障（配置应用，用户确认后调用）：同步捕获「前一屏障的 gate + 当时
    的写前沿」，发布 own gate，等二者落定后执行 fn；成功/失败都释放自己的
    gate（错误向上抛、不吞），清空只在自己仍是尾时发生。 */
export async function runConfigApplyBarrier<T>(fn: () => Promise<T>): Promise<T> {
  const prevGate = writeGate;
  const frontier = captureWriteFrontier();
  let release!: () => void;
  const ownGate = new Promise<void>((resolve) => { release = resolve; });
  writeGate = ownGate; // 发布 own gate：此后发出的写捕获它、排在本屏障之后
  try {
    await Promise.all([prevGate ?? Promise.resolve(), frontier]);
    return await fn();
  } finally {
    if (writeGate === ownGate) writeGate = null;
    release();
  }
}

/** 配置应用回读失败（应用已提交但本地没有读回实际配置）：把所有已知键的
    确认态标记为不可信（stale + 清空确认值）——后续失败不回写、不声称「已恢复
    原值」，由下一次成功写/成功读取按新值重接。 */
export function markAiConfigUnconfirmed(): void {
  for (const st of states.values()) {
    st.confirmed = undefined;
    st.stale = true;
  }
}

const AI_KEY_RE = /^(feed|cat):(.+):(summary|translate)$/;

/** 读取起点捕获（必须早于 list_folders/list_feeds 发出）。 */
export function beginConfigRead(): AiConfigReadCredential {
  return { startSeq: ++seq };
}

export interface AiWriteSpec<T> {
  key: string;
  /** 本笔目标值（乐观值与持久化值同一份） */
  value: T;
  /** 本笔发起前该键的当前值（仅在该键尚无协议状态时作为确认态播种） */
  confirmed: T;
  /** 乐观落地 / 失败恢复 / 成功对齐共用的原子索引更新 */
  apply: (value: T) => void;
  /** 真实持久化请求 */
  request: () => Promise<unknown>;
  /** 失败提示文案前缀 */
  label: string;
}

/** 发起一笔 AI 写入：入列意图 + 乐观落地 + 挂 per-键队列。 */
export function issueAiWrite<T>(spec: AiWriteSpec<T>): void {
  const { key, value, confirmed, apply, request, label } = spec;
  let st = states.get(key);
  if (!st) {
    const m = AI_KEY_RE.exec(key);
    if (!m) return; // 键由四个 AI 写入口构造，形态受控；不匹配则不进入协议
    st = {
      kind: m[1] as 'feed' | 'cat',
      id: m[2],
      field: m[3] as 'summary' | 'translate',
      confirmed,
      intents: [],
      lastSuccessSeq: 0,
      stale: false,
      apply: apply as (value: unknown) => void,
      label,
    };
    states.set(key, st);
  } else if (st.intents.length === 0 && !st.stale && st.confirmed !== confirmed) {
    /* 空闲期外部漂移自愈（见模块头注）：无意图在途时以入口读到的当前值为
       确认态；有意图在途时保留协议内的确认态（那是读快照对账的基准）；
       stale（回读失败未确认）不采纳当前值——它未经任何确认。 */
    st.confirmed = confirmed;
  }
  st.apply = apply as (value: unknown) => void; // 同键闭包语义等值，逐笔刷新引用
  st.label = label;
  const intent = { value };
  st.intents.push(intent);
  apply(value); // 乐观落地：UI 不等队列
  const gateAtIssue = writeGate; // 设门后发出的写：请求排在门后（进入时已存在的写不受门控）
  const prev = queues.get(key) ?? Promise.resolve();
  const next = prev.catch(() => undefined).then(
    gateAtIssue ? () => gateAtIssue!.then(() => request()) : request,
  );
  queues.set(key, next);
  void next.then(
    () => settle(key, intent, true),
    () => settle(key, intent, false),
  );
}

/** 落定收尾（同键按队列顺序）：出列意图 → 成功推进确认态 → 最新一笔对齐/恢复。 */
function settle(key: string, intent: { value: unknown }, ok: boolean): void {
  const st = states.get(key);
  if (!st) return;
  const idx = st.intents.indexOf(intent);
  if (idx >= 0) st.intents.splice(idx, 1);
  if (ok) {
    st.confirmed = intent.value;
    st.stale = false; // 成功写重接确认态（回读失败的「未确认」就此解除）
    st.lastSuccessSeq = ++seq; // 写成功事件：序号大于任何已捕获的读取起点
  }
  const { showToast } = appStore().getState();
  if (st.intents.length > 0) {
    /* 更晚的同键意图未落定：迟到失败不回滚（提示保持现状）；成功也不改写可视
       值（可视值 = 最新意图，乐观时已落地）。 */
    if (!ok) showToast(`${st.label}未能保存（较新的修改已生效，保持现状）`);
    return;
  }
  if (st.confirmed !== undefined) {
    st.apply(st.confirmed); // 成功对齐 / 失败恢复：最新一笔与确认态对齐
    if (!ok) showToast(`${st.label}未能保存，已恢复原值`);
  } else if (!ok) {
    /* 脱链（实体缺席权威快照）或回读失败未确认：不回写、不声称已恢复。 */
    showToast(`${st.label}未能保存`);
  }
}

/** 多键写入条目（编辑中的 AI 字段：一笔 update_feed 同时持久化多个键）。 */
export interface AiMultiWriteEntry {
  key: string;
  value: unknown;
  /** 本笔发起前该键的当前值（仅在该键尚无协议状态、且非 stale 时作确认态播种） */
  confirmed: unknown;
  /** 单键落地（失败恢复 / 成功对齐用；乐观落地走 applyAll 一次原子更新） */
  apply: (value: unknown) => void;
  label: string;
}

/** 多键写入规格：一次乐观原子落地 + 一次持久化请求 + 一次落定（避免重复失败
    toast）。请求排在**涉及键各自的队列尾之后**（同域有序），跨键独立不受影响。 */
export interface AiMultiWriteSpec {
  writes: AiMultiWriteEntry[];
  /** 多键乐观落地（categories + feedIndex 同一次 setState）；缺省逐键 apply */
  applyAll?: () => void;
  /** 单一持久化请求（完整 update_feed 只发一次，不拆事务） */
  request: () => Promise<unknown>;
  /** 失败提示前缀（如 '保存'） */
  label: string;
  /** 可选成功提示（落库成功后出现；与编辑既有 UX 对齐） */
  successToast?: string;
}

/** 发起一笔多键 AI 写入（编辑入口）：入列意图（每个键一条）+ 一次原子乐观落地
    + 单请求挂涉及键队列 + 单次落定。 */
export function issueAiMultiWrite(spec: AiMultiWriteSpec): void {
  const entries: Array<{ st: AiKeyState; intent: { value: unknown }; entry: AiMultiWriteEntry }> = [];
  for (const w of spec.writes) {
    let st = states.get(w.key);
    if (!st) {
      const m = AI_KEY_RE.exec(w.key);
      if (!m) continue; // 键形态受控；不匹配的条目不进入协议（不阻塞其余条目）
      st = {
        kind: m[1] as 'feed' | 'cat',
        id: m[2],
        field: m[3] as 'summary' | 'translate',
        confirmed: w.confirmed,
        intents: [],
        lastSuccessSeq: 0,
        stale: false,
        apply: w.apply,
        label: w.label,
      };
      states.set(w.key, st);
    } else if (st.intents.length === 0 && !st.stale && st.confirmed !== w.confirmed) {
      st.confirmed = w.confirmed; // 空闲期外部漂移自愈（同 issueAiWrite）
    }
    st.apply = w.apply;
    st.label = w.label;
    const intent = { value: w.value };
    st.intents.push(intent);
    entries.push({ st, intent, entry: w });
  }
  if (entries.length === 0) return;
  /* 乐观落地：多键一次原子更新（缺省逐键）；UI 不等队列。 */
  if (spec.applyAll) spec.applyAll();
  else for (const { entry } of entries) entry.apply(entry.value);

  const keys = [...new Set(entries.map(({ st }) => keyOf(st)))];
  const gateAtIssue = writeGate;
  const prevs = keys.map((k) => queues.get(k) ?? Promise.resolve());
  const next = Promise.all(prevs.map((p) => p.catch(() => undefined))).then(
    gateAtIssue ? () => gateAtIssue!.then(() => spec.request()) : spec.request,
  );
  for (const k of keys) queues.set(k, next);
  void next.then(
    () => settleMulti(spec, entries, true),
    (e) => settleMulti(spec, entries, false, e),
  );
}

function keyOf(st: AiKeyState): string {
  return `${st.kind}:${st.id}:${st.field}`;
}

/** 多键落定（单次收尾，一体提示——不为每个键各弹一条失败 toast）：
    逐键出列意图、成功推进确认态/清 stale；最新一笔（无更晚意图的键）与确认态
    对齐或恢复；有更晚意图的键保持现状。失败提示合并为一条。 */
function settleMulti(
  spec: AiMultiWriteSpec,
  entries: Array<{ st: AiKeyState; intent: { value: unknown }; entry: AiMultiWriteEntry }>,
  ok: boolean,
  err?: unknown,
): void {
  for (const { st, intent } of entries) {
    const idx = st.intents.indexOf(intent);
    if (idx >= 0) st.intents.splice(idx, 1);
    if (ok) {
      st.confirmed = intent.value;
      st.stale = false;
      st.lastSuccessSeq = ++seq;
    }
  }
  let rolledBack = false;
  let heldForLater = false;
  for (const { st } of entries) {
    if (st.intents.length > 0) { heldForLater = true; continue; }
    if (st.confirmed !== undefined) {
      st.apply(st.confirmed); // 成功对齐 / 失败恢复（最近一次可确认值）
      if (!ok) rolledBack = true;
    }
    /* confirmed undefined（脱链 / 回读失败未确认）：不回写、不声称已恢复 */
  }
  const { showToast } = appStore().getState();
  if (ok) {
    if (spec.successToast) showToast(spec.successToast);
    return;
  }
  const msg = err !== undefined ? extractError(err) : '';
  const base = `${spec.label}失败${msg ? `：${msg}` : ''}`;
  if (rolledBack) showToast(`${base}（已恢复原值）`);
  else if (heldForLater) showToast(`${base}（较新的修改已生效，保持现状）`);
  else showToast(base);
}

/** 读取快照接受（reloadFromBackend 在 set 之前调用）：
    更新确认态 + 保留未落定意图的可视值（覆盖快照里的旧值）。 */
export function applyConfigRead(categories: CategoryGroup[], cred: AiConfigReadCredential): void {
  const sr = cred.startSeq;
  const freshForAll = sr > lastAcceptedReadSeq;
  if (freshForAll) lastAcceptedReadSeq = sr;
  for (const st of states.values()) {
    const entity = findEntity(categories, st);
    if (!entity) {
      /* 权威快照确认实体缺失 → 脱链；过时快照（期间有写成功）不改确认态。
         新鲜快照同时解除 stale（缺失是权威结论，不再处于「未确认」）。 */
      if (freshForAll && st.lastSuccessSeq <= sr) {
        st.confirmed = undefined;
        st.stale = false;
      }
      continue;
    }
    if (freshForAll && st.lastSuccessSeq <= sr) {
      st.confirmed = readEntityValue(entity, st); // 快照可作确认态
      st.stale = false; // 成功读取重接（回读失败的「未确认」解除）
    }
    const vis = st.intents.length > 0 ? st.intents[st.intents.length - 1].value : st.confirmed;
    if (vis !== undefined) writeEntityValue(entity, st, vis);
  }
}

/** 按键身份在当前快照里找实体（既覆盖 store，也覆盖「即将 set」的 categories）。 */
function findEntity(categories: CategoryGroup[], st: AiKeyState): FeedItem | CategoryGroup | undefined {
  if (st.kind === 'feed') {
    for (const c of categories) {
      const f = c.feeds.find((x) => x.id === st.id);
      if (f) return f;
    }
    return undefined;
  }
  return categories.find((x) => x.id === st.id);
}

function readEntityValue(entity: FeedItem | CategoryGroup, st: AiKeyState): unknown {
  if (st.kind === 'feed') {
    const f = entity as FeedItem;
    return st.field === 'summary'
      ? normalizeFeedAiMode(f.summaryMode, f.autoSummary)
      : normalizeFeedAiMode(f.translateMode, f.autoTranslate);
  }
  const c = entity as CategoryGroup;
  return st.field === 'summary' ? c.autoSummary : c.autoTranslate;
}

function writeEntityValue(entity: FeedItem | CategoryGroup, st: AiKeyState, value: unknown): void {
  if (st.kind === 'feed') {
    const f = entity as FeedItem;
    if (st.field === 'summary') f.summaryMode = value as FeedAiMode;
    else f.translateMode = value as FeedAiMode;
  } else {
    const c = entity as CategoryGroup;
    if (st.field === 'summary') c.autoSummary = value as boolean;
    else c.autoTranslate = value as boolean;
  }
}
