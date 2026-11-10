import type { ArticleEntry } from '../types';

/** 列表只给首图；完整集合只属于一次灯箱会话，不进入正文/阅读会话缓存。 */
export interface GalleryLightboxState {
  entryId: string;
  sourceKey: string;
  requestId: number;
  mediaRevision: number;
  firstImageUrl: string | null;
  pageUrl?: string;
  urls: string[];
  index: number;
  status: 'loading' | 'ready' | 'empty' | 'error';
  /** 无首图的条目须取到真实图片后才打开和触发 markReadOnOpen。 */
  opened: boolean;
}

/** 媒体修订之外也检查来源与首图元数据，防止条目替换或旧列表误复用集合。 */
export function galleryMediaKey(entry: ArticleEntry): string {
  return JSON.stringify([entry.id, entry.feedId, entry.mediaRevision ?? 0,
    entry.imageUrl ?? null, entry.imageCount ?? 0, entry.url ?? null, !!entry.unavailable]);
}

/** 保留后端顺序；过滤无图片地址的空项，不拿封面替代权威的空集合。 */
export function galleryImageUrls(images: readonly string[]): string[] {
  return images.filter((url) => typeof url === 'string' && url.trim().length > 0);
}

/** 集合更新优先保留当前 URL；已移除时把原下标夹到新集合内。 */
export function galleryUpdatedIndex(urls: readonly string[], currentUrl: string | undefined, index: number): number {
  const retained = currentUrl ? urls.indexOf(currentUrl) : -1;
  return retained >= 0 ? retained : Math.max(0, Math.min(index, urls.length - 1));
}

export function galleryStepIndex(index: number, delta: -1 | 1, length: number): number {
  return length > 0 ? (index + delta + length) % length : 0;
}
