import {
  DEFAULT_OPEN_MODES,
  PALETTE_THEMES,
  STARTUP_VIEWS,
  SYNC_MODES,
  THEME_MODES,
} from '../types';
import type { SettingsState } from './types';

/* ============================================================
   设置项的运行时校验（D5）。

   缺陷背景：updateSettings 无任何校验，非法值（fontSize:-5 /
   refreshInterval:0 / fetchConcurrency:99 / listWidth:99999）全部进内存并落库；
   而读回路径（bootstrapSettings）只做 typeof 拦截 —— 形成「写入不校验 /
   读回丢弃」的不对称：越界值能写进 SQLite，读回来却被静默丢弃，用户看到
   「设置保存了但下次打开变回默认」。范围约束此前只存在于组件的 range min/max。

   修法：每个键一张校验器（类型 + 取值域），写路径与读回路径共用同一张表；
   区间与设置页滑杆的 min/max 同源（ReadingTab/GeneralTab/App 拖拽）。
   ============================================================ */

/** 数值设置项的允许区间 [min, max]，与设置页 <input type="range"> 一致 */
const NUMERIC_RANGES: Record<string, readonly [number, number]> = {
  refreshInterval: [5, 120],   // GeneralTab: min 5 / max 120
  fetchConcurrency: [1, 16],   // GeneralTab: min 1 / max 16
  fontSize: [13, 24],          // ReadingTab: min 13 / max 24
  lineHeight: [130, 240],      // ReadingTab: min 130 / max 240
  maxWidth: [560, 1100],       // ReadingTab: min 560 / max 1100
  listWidth: [280, 560],       // App.tsx 拖拽夹取范围
};

const isBool = (v: unknown): boolean => typeof v === 'boolean';
const oneOf = (allowed: readonly unknown[]) => (v: unknown): boolean => allowed.includes(v);
const inRange = (range: readonly [number, number]) => (v: unknown): boolean =>
  typeof v === 'number' && Number.isFinite(v) && v >= range[0] && v <= range[1];

/** fontFamily 只要求「非空字符串」：字体族是自由文本（FONT_OPTIONS 可被覆盖） */
const isNonEmptyString = (v: unknown): boolean => typeof v === 'string' && v.trim().length > 0;

/** 每个设置键的校验器。用 Record<keyof SettingsState, …> 声明：
   SettingsState 新增键而这里漏写时 tsc -b 直接报错，不会静默漏校验。 */
export const SETTINGS_VALIDATORS: Record<keyof SettingsState, (v: unknown) => boolean> = {
  /* 通用 */
  autoRefresh: isBool,
  refreshInterval: inRange(NUMERIC_RANGES.refreshInterval),
  fetchConcurrency: inRange(NUMERIC_RANGES.fetchConcurrency),
  markReadOnOpen: isBool,
  markReadOnScrollBottom: isBool,
  markReadOnScrollOut: isBool,
  autoStart: isBool,
  startupView: oneOf(STARTUP_VIEWS),
  hideReadOnStartup: isBool,
  /* 外观 */
  themeMode: oneOf(THEME_MODES),
  palette: oneOf(PALETTE_THEMES),
  /* 阅读 */
  fontFamily: isNonEmptyString,
  fontSize: inRange(NUMERIC_RANGES.fontSize),
  lineHeight: inRange(NUMERIC_RANGES.lineHeight),
  maxWidth: inRange(NUMERIC_RANGES.maxWidth),
  listWidth: inRange(NUMERIC_RANGES.listWidth),
  showReadTime: isBool,
  defaultOpenMode: oneOf(DEFAULT_OPEN_MODES),
  smartDedup: isBool,
  closeToTray: isBool,
  closePromptShown: isBool,
  notifyOnNewArticles: isBool,
  autoSync: isBool,
  syncMode: oneOf(SYNC_MODES),
};

/** 单个键值是否合法；未知键一律非法（不认识的键不进 settings） */
export function isValidSetting(key: string, value: unknown): boolean {
  const validate = Object.prototype.hasOwnProperty.call(SETTINGS_VALIDATORS, key)
    ? SETTINGS_VALIDATORS[key as keyof SettingsState]
    : undefined;
  return validate ? validate(value) : false;
}

/** 过滤一份设置补丁：只保留「已知键 + 校验通过」的项。
    写路径（updateSettings）与读回路径（bootstrapSettings）共用，保证
    「能写进去的一定能被读回来」。 */
export function sanitizeSettingsPatch(patch: Record<string, unknown>): Partial<SettingsState> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(patch)) {
    if (isValidSetting(k, v)) out[k] = v;
  }
  return out as Partial<SettingsState>;
}
