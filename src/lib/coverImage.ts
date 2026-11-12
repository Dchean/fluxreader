// Note: Only qualified permanent failures may clear persistent cover metadata — see
// .agents/notes/implemented/feature/2026-10-07-封面代理与失败占位.md
import { api } from './api';
import { imageEpoch, imagesSuspended, IMAGE_LIMITS, resetImagesForTest, type ImageLease } from './imageProxy';
const reported = new Map<string, number>();
let reportTail: Promise<void> = Promise.resolve();
let pendingReports = 0;
/** Invocation is registered synchronously; restore awaits this frontier before replacing the DB. */
export function reportCoverFailure(lease: ImageLease, articleId: string | null | undefined,
  isCurrent: () => boolean): boolean {
  const snapshot = lease.snapshot();
  const id = Number(articleId);
  if (pendingReports >= IMAGE_LIMITS.pending || !articleId || !Number.isSafeInteger(id) || id <= 0 || imagesSuspended() || lease.epoch !== imageEpoch()
    || snapshot.status !== 'failed' || snapshot.reason !== 'permanent' || !isCurrent()) return false;
  const now = Date.now();
  for (const [key, until] of reported) if (until <= now) reported.delete(key);
  const key = JSON.stringify([lease.epoch, id, lease.url]);
  if (reported.has(key)) return false;
  while (reported.size >= IMAGE_LIMITS.failures) reported.delete(reported.keys().next().value!);
  reported.set(key, now + IMAGE_LIMITS.permanentTtl);
  ++pendingReports;
  reportTail = reportTail.then(async () => {
    if (imagesSuspended() || lease.epoch !== imageEpoch() || !isCurrent()) return;
    try { await api.reportBrokenCover(id, lease.url); }
    catch (error) { console.warn('[cover] report_broken_cover 失败（已忽略）', error); }
  }).finally(() => { --pendingReports; });
  return true;
}
export function awaitCoverReportFrontier(): Promise<void> { return reportTail; }
export function resetCoverReports(): void { reported.clear(); }
export function coverReportCount(): number { return reported.size; }
export function resetCoverCacheForTest(): void { resetCoverReports(); resetImagesForTest(); }

