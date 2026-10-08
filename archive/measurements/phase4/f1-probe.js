(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const input = () => document.querySelector('.search-modal-input');
  const overlay = () => { const i = input(); return i ? i.closest('.modal-overlay') : null; };
  const open = () => { const i = input(); const o = overlay(); return !!(i && o && o.classList.contains('open')); };
  const allItems = () => { const o = overlay(); return o ? [].slice.call(o.querySelectorAll('.cp-item')).map((e) => (e.textContent || '').trim()) : []; };
  const groups = () => { const o = overlay(); return o ? [].slice.call(o.querySelectorAll('[role="group"]')).map((g) => g.getAttribute('aria-label')) : []; };
  const articleItems = () => window.__t128.articleGroupItems();
  const esc = () => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', bubbles: true, cancelable: true }));
  if (open()) { esc(); await sleep(400); }
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', code: 'KeyK', ctrlKey: true, bubbles: true, cancelable: true }));
  for (let i = 0; i < 60 && !open(); i += 1) await sleep(50);
  const baselineAll = allItems();
  const baselineArticle = articleItems();
  const groupsBefore = groups();
  const token = 'APOD';
  const el = input();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  const t0 = performance.now();
  setter.call(el, token);
  el.dispatchEvent(new Event('input', { bubbles: true }));
  let oldHit = null; let oldMs = null; let newHit = null; let newMs = null;
  while (performance.now() - t0 < 3000) {
    await sleep(16);
    const all = allItems();
    const art = articleItems();
    if (oldHit === null && all.some((t) => t.indexOf(token) >= 0) && all.length !== baselineAll.length) {
      oldHit = all.filter((t) => t.indexOf(token) >= 0); oldMs = +(performance.now() - t0).toFixed(1);
    }
    if (newHit === null && art.some((t) => t.indexOf(token) >= 0) && art.length !== baselineArticle.length) {
      newHit = art.filter((t) => t.indexOf(token) >= 0); newMs = +(performance.now() - t0).toFixed(1);
    }
  }
  const out = {
    token: token,
    tokenIsFeedNameSubstring: true,
    groupsBeforeTyping: groupsBefore,
    allItemsBefore: baselineAll.length,
    articleItemsBefore: baselineArticle.length,
    groupsAfterTyping: groups(),
    allItemsAfter: allItems().length,
    articleItemsAfter: articleItems().length,
    oldCriterion_allCpItems: { hit: oldHit, resultMs: oldMs },
    newCriterion_articleGroupOnly: { hit: newHit, resultMs: newMs },
  };
  if (open()) { esc(); await sleep(400); }
  return out;
})()
