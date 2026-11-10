// Note: 中心700ms停顿、撤销资格与单控制器 — 见 .agents/notes/implemented/architecture/2026-11-09-三层AI策略与阅读触发.md
import { useEffect, type RefObject } from 'react';
import { useAppStore, selectArticleBody, selectFeedConfig, selectAiGenerationInput, sameAiGenerationInput } from '../store';

/** 连续700ms不滚动、卡片覆盖列表视觉中心才算停顿阅读。时间线共享一个候选、
 * timer和滚动监听；挂载/水合不会对每卡创建计时器，也不发起任何水合请求。 */
export const AUTO_AI_DWELL_MS = 700;
type Registration = { id: string; node: HTMLElement; reader: boolean };
type Input = ReturnType<typeof selectAiGenerationInput>;
type Lease = { registration: Registration; input: Input; scope: string; summary: boolean; translation: boolean; alive: boolean };
const registrations = new Set<Registration>();
let candidate: Lease | null = null;
let readerLease: Lease | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let frame = 0;
let focused = true;
let stopStore: (() => void) | undefined;
let resizeObserver: ResizeObserver | undefined;

function scope() {
  const s = useAppStore.getState();
  return JSON.stringify([s.readingSession?.generation, s.activeContentLayout, s.activeFeedFilter,
    s.activeViewFilter, s.timelineFilter, s.timelineSort]);
}

function unobstructed() {
  const s = useAppStore.getState();
  return focused && document.visibilityState !== 'hidden' && !s.settingsOpen && !s.searchOpen
    && !s.contextMenuOpen && !s.closeAskVisible && !s.lightboxUrl && !s.playerExpanded
    && !s.newCategoryModalOpen && !s.addFeedModalOpen && !s.editFeedModalOpen
    && !s.renameCatModalOpen && !s.errorCenterOpen;
}

function displayed(node: HTMLElement) {
  if (!node.isConnected) return false;
  for (let el: HTMLElement | null = node; el; el = el.parentElement) {
    const style = getComputedStyle(el);
    if (el.hidden || el.inert || style.display === 'none' || style.visibility === 'hidden') return false;
  }
  return true;
}

function ready(reg: Registration): boolean {
  const s = useAppStore.getState();
  if (s.dataMode !== 'tauri' || !unobstructed() || !registrations.has(reg) || !displayed(reg.node)) return false;
  const entry = s.entries.find((e) => e.id === reg.id);
  if (!entry || entry.unavailable) return false;
  const policy = selectFeedConfig(s, entry.feedId);
  if (reg.reader ? s.activeContentLayout !== 'article' || policy.layout !== 'article' || s.activeArticleId !== reg.id
    : policy.layout !== s.activeContentLayout || (policy.layout !== 'social' && policy.layout !== 'notification')) return false;
  if (!policy.autoSummary && !policy.autoTranslate) return false;
  const body = selectArticleBody(s, reg.id);
  return (body.state === 'ready' || body.state === 'cleared') && body.content.trim() !== '';
}

function centered(reg: Registration): boolean {
  const root = reg.node.closest<HTMLElement>('#timelineContentScroll');
  if (!root) return false;
  const viewport = root.getBoundingClientRect();
  const rect = reg.node.getBoundingClientRect();
  const top = Math.max(0, viewport.top);
  const bottom = Math.min(window.innerHeight, viewport.bottom);
  const center = (top + bottom) / 2;
  return bottom > top && rect.width > 0 && rect.height > 0 && rect.top <= center && rect.bottom > center;
}

function newLease(registration: Registration): Lease {
  const s = useAppStore.getState();
  const entry = s.entries.find((e) => e.id === registration.id)!;
  const policy = selectFeedConfig(s, entry.feedId);
  return { registration, input: selectAiGenerationInput(s, registration.id), scope: scope(),
    summary: policy.autoSummary, translation: policy.autoTranslate, alive: true };
}

function valid(lease: Lease): boolean {
  if (!lease.alive || !ready(lease.registration) || lease.scope !== scope()) return false;
  const s = useAppStore.getState();
  if (!sameAiGenerationInput(lease.input, selectAiGenerationInput(s, lease.registration.id))) return false;
  const policy = selectFeedConfig(s, lease.input.feedId);
  return policy.autoSummary === lease.summary && policy.autoTranslate === lease.translation
    && (lease.registration.reader || centered(lease.registration));
}

function revokeCandidate() {
  if (candidate) candidate.alive = false;
  candidate = null;
  if (timer !== null) clearTimeout(timer);
  timer = null;
}

function trigger(lease: Lease) {
  const current = () => valid(lease);
  const s = useAppStore.getState();
  if (lease.summary) s.startAutomaticAi(lease.registration.id, 'summary', current);
  if (lease.translation) s.startAutomaticAi(lease.registration.id, 'translation', current);
}

function evaluate() {
  frame = 0;
  if (readerLease && !valid(readerLease)) { readerLease.alive = false; readerLease = null; }
  const reader = [...registrations].find((r) => r.reader && ready(r));
  if (reader && !readerLease) {
    readerLease = newLease(reader);
    trigger(readerLease);
  }
  if (candidate && !valid(candidate)) revokeCandidate();
  if (candidate) return;
  const reg = [...registrations].find((r) => !r.reader && ready(r) && centered(r));
  if (!reg) return;
  const lease = newLease(reg);
  candidate = lease;
  timer = setTimeout(() => {
    timer = null;
    if (candidate === lease && valid(lease)) trigger(lease);
    else revokeCandidate();
  }, AUTO_AI_DWELL_MS);
}

function schedule() {
  // 同步撤销保证关→开、A→B→A也不能复活旧验签链；普通属性更新不重置停顿。
  if (candidate && !valid(candidate)) revokeCandidate();
  if (readerLease && !valid(readerLease)) { readerLease.alive = false; readerLease = null; }
  if (!frame) frame = requestAnimationFrame(evaluate);
}
function movement(event: Event) {
  if (event.type === 'scroll' && !(event.target instanceof Element && event.target.id === 'timelineContentScroll')) return;
  revokeCandidate();
  schedule();
}
function blur() { focused = false; schedule(); }
function focus() { focused = true; schedule(); }

function register(registration: Registration) {
  registrations.add(registration);
  if (registrations.size === 1) {
    focused = document.hasFocus();
    stopStore = useAppStore.subscribe(schedule);
    document.addEventListener('scroll', movement, true);
    window.addEventListener('resize', movement);
    window.addEventListener('blur', blur);
    window.addEventListener('focus', focus);
    document.addEventListener('visibilitychange', schedule);
    resizeObserver = new ResizeObserver(schedule);
  }
  resizeObserver?.observe(registration.node);
  schedule();
  return () => {
    registrations.delete(registration);
    resizeObserver?.unobserve(registration.node);
    if (candidate?.registration === registration) revokeCandidate();
    if (readerLease?.registration === registration) { readerLease.alive = false; readerLease = null; }
    if (registrations.size) { schedule(); return; }
    stopStore?.(); stopStore = undefined;
    resizeObserver?.disconnect(); resizeObserver = undefined;
    document.removeEventListener('scroll', movement, true);
    window.removeEventListener('resize', movement);
    window.removeEventListener('blur', blur);
    window.removeEventListener('focus', focus);
    document.removeEventListener('visibilitychange', schedule);
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
  };
}

export function useCenterDwellAi(id: string, ref: RefObject<HTMLElement | null>) {
  useEffect(() => ref.current ? register({ id, node: ref.current, reader: false }) : undefined, [id, ref]);
}

export function useReaderAutomaticAi(id: string | undefined, ref: RefObject<HTMLElement | null>) {
  useEffect(() => id && ref.current ? register({ id, node: ref.current, reader: true }) : undefined, [id, ref]);
}
