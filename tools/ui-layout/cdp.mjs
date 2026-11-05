import fs from 'node:fs/promises';

export class CDP {
  constructor(webSocketUrl) {
    this.ws = new WebSocket(webSocketUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    this.ready = new Promise((resolve, reject) => {
      this.ws.addEventListener('open', resolve, { once: true });
      this.ws.addEventListener('error', reject, { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(`${message.error.message} (${message.error.code})`));
        else pending.resolve(message.result);
        return;
      }
      const handlers = this.listeners.get(message.method);
      if (!handlers) return;
      for (const handler of handlers) handler(message.params);
    });
  }

  async send(method, params = {}) {
    await this.ready;
    const id = this.nextId++;
    const result = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    this.ws.send(JSON.stringify({ id, method, params }));
    return result;
  }

  on(method, handler) {
    const handlers = this.listeners.get(method) ?? [];
    handlers.push(handler);
    this.listeners.set(method, handlers);
    return () => {
      const next = (this.listeners.get(method) ?? []).filter((item) => item !== handler);
      if (next.length) this.listeners.set(method, next);
      else this.listeners.delete(method);
    };
  }

  once(method, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off();
        reject(new Error(`timeout waiting for ${method}`));
      }, timeoutMs);
      const off = this.on(method, (params) => {
        clearTimeout(timer);
        off();
        resolve(params);
      });
    });
  }

  close() {
    this.ws.close();
  }
}

export async function createTarget(debugPort, url = 'about:blank') {
  const endpoint = `http://127.0.0.1:${debugPort}/json/new?${encodeURIComponent(url)}`;
  const response = await fetch(endpoint, { method: 'PUT' });
  if (!response.ok) throw new Error(`createTarget failed: ${response.status} ${await response.text()}`);
  return response.json();
}

export async function connectPage(debugPort, url = 'about:blank') {
  const target = await createTarget(debugPort, url);
  const cdp = new CDP(target.webSocketDebuggerUrl);
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  await cdp.send('Log.enable');
  await cdp.send('Network.enable');
  return { target, cdp };
}

export async function evalValue(cdp, expression, timeoutMs = 20000) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
    timeout: timeoutMs,
  });
  if (result.exceptionDetails) {
    const error = result.exceptionDetails.exception;
    throw new Error(error?.description ?? result.exceptionDetails.text ?? 'evaluation failed');
  }
  return result.result.value;
}

export async function waitFor(cdp, expression, timeoutMs = 20000, intervalMs = 50) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await evalValue(cdp, `Boolean(${expression})`);
      if (last) return;
    } catch (error) {
      last = error.message;
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  throw new Error(`waitFor timeout: ${expression}; last=${last}`);
}

export async function screenshot(cdp, filePath) {
  const { data } = await cdp.send('Page.captureScreenshot', {
    format: 'png',
    fromSurface: true,
    captureBeyondViewport: false,
  });
  await fs.writeFile(filePath, Buffer.from(data, 'base64'));
}

export async function setViewport(cdp, width, height, deviceScaleFactor = 1) {
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor,
    mobile: false,
    screenWidth: width,
    screenHeight: height,
  });
}

export async function navigate(cdp, url) {
  await cdp.send('Page.navigate', { url });
  const origin = new URL(url).origin;
  await waitFor(
    cdp,
    `location.origin === ${JSON.stringify(origin)} && document.readyState !== 'loading'`,
    30000,
  );
}

export async function saveJson(filePath, value) {
  await fs.writeFile(filePath, JSON.stringify(value, null, 2));
}
