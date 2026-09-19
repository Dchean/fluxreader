// TASK-059 实机验证的 CDP 公共层：连接、求值、等待，以及页面内操作助手。
// 被 t059_ui_probe.mjs（探测）与 t059_ui_e2e.mjs（端到端）共用，避免两份实现漂移。

export const CDP_PORT = Number(process.env.T059_CDP_PORT || 9222);

export async function findPageTarget(port = CDP_PORT) {
  const res = await fetch(`http://127.0.0.1:${port}/json/list`);
  const targets = await res.json();
  const page = targets.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
  if (!page) {
    throw new Error(`未找到 page target：${JSON.stringify(targets.map((t) => t.type))}`);
  }
  return page;
}

export function connect(wsUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && pending.has(msg.id)) {
        const { resolve: res, reject: rej } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) rej(new Error(JSON.stringify(msg.error)));
        else res(msg.result);
      }
    });
    ws.addEventListener('error', reject);
    ws.addEventListener('open', () => {
      const send = (method, params = {}) =>
        new Promise((res, rej) => {
          const mid = ++id;
          pending.set(mid, { resolve: res, reject: rej });
          ws.send(JSON.stringify({ id: mid, method, params }));
        });
      resolve({
        send,
        close: () => ws.close(),
        async evaluate(expression) {
          const r = await send('Runtime.evaluate', {
            expression,
            returnByValue: true,
            awaitPromise: true,
          });
          if (r.exceptionDetails) {
            throw new Error(
              `页面内异常：${r.exceptionDetails.exception?.description || r.exceptionDetails.text}`,
            );
          }
          return r.result.value;
        },
      });
    });
  });
}

/** 轮询等待条件成立（页面内表达式返回真值）。 */
export async function waitFor(cdp, expression, { timeoutMs = 15000, label = expression } = {}) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await cdp.evaluate(
      `(() => { try { return ${expression}; } catch (e) { return 'ERR:' + e.message; } })()`,
    );
    if (last && last !== false) return last;
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`等待超时（${label}），最后一次取值：${JSON.stringify(last)}`);
}

/** 页面内通用工具：按文本点击、按卡片填值、读 toast。 */
export const PAGE_HELPERS = `
window.__t059 = {
  byText(selector, text) {
    return [...document.querySelectorAll(selector)].find((el) => (el.textContent || '').includes(text)) || null;
  },
  clickText(selector, text) {
    const el = this.byText(selector, text);
    if (!el) return 'NOT_FOUND:' + selector + ' / ' + text;
    el.click();
    return 'clicked';
  },
  toasts() { return [...document.querySelectorAll('.toast-pill')].map((e) => e.textContent.trim()); },
  theme() { return document.documentElement.getAttribute('data-theme'); },
  card(title) { return this.byText('.setting-card', title); },
  endpointCard() {
    const card = this.card('后端 Endpoint');
    if (!card) return null;
    const input = card.querySelector('input');
    return {
      desc: (card.querySelector('.setting-card-text p') || {}).textContent || null,
      placeholder: input ? input.placeholder : null,
      value: input ? input.value : null,
    };
  },
  setCardInput(cardTitle, value) {
    const card = this.card(cardTitle);
    if (!card) return 'NOT_FOUND:card[' + cardTitle + ']';
    const el = card.querySelector('input');
    if (!el) return 'NOT_FOUND:input in [' + cardTitle + ']';
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return el.value;
  },
  /** 按「字段身份」而非卡片文案定位输入框。
   *  为什么不用卡片文案：用户名卡片的说明里含「账号密码」三字，
   *  用文案找「密码」卡会命中用户名卡片（实测踩到过，导致密码框始终为空）。 */
  fieldEl(kind) {
    if (kind === 'endpoint') {
      const card = this.card('后端 Endpoint');
      return card ? card.querySelector('input') : null;
    }
    if (kind === 'username') {
      return [...document.querySelectorAll('input')].find((i) => i.placeholder === '集成用户名') || null;
    }
    if (kind === 'password') {
      return document.querySelector('.settings-content-pane input[type="password"]') || null;
    }
    return null;
  },
  setField(kind, value) {
    const el = this.fieldEl(kind);
    if (!el) return 'NOT_FOUND:' + kind;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return el.value;
  },
  fieldValue(kind) {
    const el = this.fieldEl(kind);
    return el ? el.value : null;
  },
  openProtocolMenu() {
    const card = this.card('同步协议');
    if (!card) return 'NOT_FOUND:同步协议卡片';
    const trigger = card.querySelector('.flux-dropdown-trigger');
    if (!trigger) return 'NOT_FOUND:flux-dropdown-trigger';
    trigger.click();
    return 'opened';
  },
  chooseOption(label) {
    const opt = this.byText('.flux-dropdown-option', label);
    if (!opt) return 'NOT_FOUND:option[' + label + ']';
    opt.click();
    return 'chosen:' + opt.textContent.trim();
  },
  clickAction(label) {
    const el = this.byText('.toggle-action-btn', label);
    if (!el) return 'NOT_FOUND:' + label;
    if (el.disabled) return 'DISABLED:' + label;
    el.click();
    return 'clicked:' + el.textContent.trim();
  },
  actionLabels() { return [...document.querySelectorAll('.toggle-action-btn')].map(e => e.textContent.trim() + (e.disabled ? '(disabled)' : '')); },
};
'helpers-ready';
`;
