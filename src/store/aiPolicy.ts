// Note: 三层解析与费用边界 — 见 .agents/notes/implemented/architecture/2026-11-09-三层AI策略与阅读触发.md
import type { CategoryGroup, ContentLayoutType, FeedAiMode } from '../types';
import { normalizeFeedAiMode, resolveFeedAiMode } from '../types';
import type { SettingsState } from './types';

/** 对象的实际布局决定能力；导航所在分区、auto 开关均不能扩大手动能力。 */
export function aiCapabilities(layout: ContentLayoutType | undefined) {
  return {
    summary: layout === 'article' || layout === 'notification',
    translation: layout === 'article' || layout === 'social' || layout === 'notification',
  };
}

export function layoutAiDefaults(settings: SettingsState, layout: ContentLayoutType) {
  return {
    autoSummary: layout === 'article' ? settings.articleAutoSummary
      : layout === 'notification' ? settings.notificationAutoSummary : false,
    autoTranslate: layout === 'article' ? settings.articleAutoTranslate
      : layout === 'social' ? settings.socialAutoTranslate
        : layout === 'notification' ? settings.notificationAutoTranslate : false,
  };
}

/** inherit 以源的实际布局取默认，即使该源覆盖了分组的布局也不误套。 */
export function categoryAiDefaults(
  settings: SettingsState,
  layout: ContentLayoutType,
  category?: Pick<CategoryGroup, 'summaryMode' | 'translateMode'>,
) {
  const defaults = layoutAiDefaults(settings, layout);
  return {
    autoSummary: resolveFeedAiMode(normalizeFeedAiMode(category?.summaryMode), defaults.autoSummary),
    autoTranslate: resolveFeedAiMode(normalizeFeedAiMode(category?.translateMode), defaults.autoTranslate),
  };
}

export function resolveAiPolicy(
  settings: SettingsState,
  layout: ContentLayoutType,
  category: Pick<CategoryGroup, 'summaryMode' | 'translateMode'> | undefined,
  feed: { summaryMode?: FeedAiMode; translateMode?: FeedAiMode },
) {
  const defaults = categoryAiDefaults(settings, layout, category);
  const capability = aiCapabilities(layout);
  const summaryMode = normalizeFeedAiMode(feed.summaryMode);
  const translateMode = normalizeFeedAiMode(feed.translateMode);
  return {
    layout,
    canSummarize: capability.summary,
    canTranslate: capability.translation,
    autoSummary: capability.summary && resolveFeedAiMode(summaryMode, defaults.autoSummary),
    autoTranslate: capability.translation && resolveFeedAiMode(translateMode, defaults.autoTranslate),
    summaryMode,
    translateMode,
  };
}
