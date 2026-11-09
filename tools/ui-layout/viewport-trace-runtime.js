// 可选真实浏览器诊断；仅 UI_LAYOUT_TRACE=viewport 时在应用脚本前注入。
// 透传所有宿主调用，不等待帧、不更改虚拟器/滚动量/断言。style 写入必须自检命中。
(() => {
  const events = [];
  const stats = Object.create(null);
  const cap = 200000;
  let dropped = 0;
  let sequence = 0;
  let nodeSequence = 0;
  let recording = true;
  let phase = 'selfcheck';
  const nodeIds = new WeakMap();
  const nodeId = (node) => {
    if (!nodeIds.has(node)) nodeIds.set(node, ++nodeSequence);
    return nodeIds.get(node);
  };
  const trace = (type, detail) => {
    if (!recording) return;
    stats[type] = (stats[type] ?? 0) + 1;
    if (events.length >= cap) { dropped++; return; }
    events.push({ sequence: ++sequence, at: performance.now(), phase, type, ...detail });
  };
  const relevant = (element) => element?.matches?.(
    '.gallery-virtual-item,.gallery-masonry-grid,.timeline-virtual-item,.timeline-virtual-wrap,#timelineContentScroll,.viewport-trace-selfcheck',
  );
  const describe = (element) => ({
    nodeId: nodeId(element),
    tag: element.tagName,
    className: element.className,
    index: element.dataset?.index,
    cardId: element.dataset?.cardId,
  });
  const stack = () => new Error().stack;

  // Chromium 的 CSS 属性不一定在 CSSStyleDeclaration.prototype 上有 setter；
  // 拦截 style getter 返回的真实对象，才能捕获 style.transform = value 的赋值。
  const styleDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'style');
  if (!styleDescriptor?.get) throw new Error('viewport trace: HTMLElement.style getter unavailable');
  const styleProxies = new WeakMap();
  const rawStyles = new WeakMap();
  const styleOwners = new WeakMap();
  const styleMethods = new WeakMap();
  Object.defineProperty(HTMLElement.prototype, 'style', {
    ...styleDescriptor,
    get() {
      const raw = styleDescriptor.get.call(this);
      if (!styleProxies.has(raw)) {
        const element = this;
        styleOwners.set(raw, element);
        const proxy = new Proxy(raw, {
          get(target, property) {
            const value = Reflect.get(target, property, target);
            if (typeof value !== 'function' || property === 'constructor'
              || value === Object.prototype[property]) return value;
            if (!styleMethods.has(value)) {
              // 不绑定首次读取方法的 style；借用 .call/.apply 时按实际接收者解包。
              // Reflect.apply 保留完整参数列表、缺参数异常、原生返回与抛出对象。
              styleMethods.set(value, new Proxy(value, {
                apply(method, receiver, args) {
                  const actualReceiver = rawStyles.get(receiver) ?? receiver;
                  const result = Reflect.apply(method, actualReceiver, args);
                  const owner = styleOwners.get(actualReceiver);
                  if (property === 'setProperty' && recording && owner && relevant(owner)) {
                    trace('style.setProperty', {
                      ...describe(owner), args, name: args[0], value: args[1], stack: stack(),
                    });
                  }
                  return result;
                },
              }));
            }
            return styleMethods.get(value);
          },
          set(target, property, value) {
            const old = Reflect.get(target, property, target);
            const result = Reflect.set(target, property, value, target);
            if (recording && relevant(element) && ['transform', 'height', 'top', 'left'].includes(property)) {
              // 同值重写也保留：不能先排除「后续重复写才改变 computed geometry」。
              trace('style.set', { ...describe(element), name: property, value, old, stack: stack() });
            }
            return result;
          },
        });
        styleProxies.set(raw, proxy);
        rawStyles.set(proxy, raw);
      }
      return styleProxies.get(raw);
    },
  });

  const nativeScrollTo = Element.prototype.scrollTo;
  Element.prototype.scrollTo = function (...args) {
    const capture = recording && relevant(this);
    // 写入插桩不读 rect/scrollHeight/scrollTop，避免另加一次同步布局读取。
    if (capture) trace('scrollTo.before', { ...describe(this), args, stack: stack() });
    const result = nativeScrollTo.apply(this, args);
    if (capture) trace('scrollTo.after', describe(this));
    return result;
  };
  const scrollTopDescriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
  if (!scrollTopDescriptor?.set) throw new Error('viewport trace: Element.scrollTop setter unavailable');
  Object.defineProperty(Element.prototype, 'scrollTop', {
    ...scrollTopDescriptor,
    set(value) {
      if (recording && relevant(this)) trace('scrollTop.set', { ...describe(this), value, stack: stack() });
      return scrollTopDescriptor.set.call(this, value);
    },
  });

  const NativeResizeObserver = window.ResizeObserver;
  let observerSequence = 0;
  window.ResizeObserver = class extends NativeResizeObserver {
    constructor(...args) {
      const callback = args[0];
      // 让原生构造器在原时刻拒绝缺失/不可调用的 callback，保留其错误类型和文案。
      if (typeof callback !== 'function') {
        super(...args);
        return;
      }
      const observerId = ++observerSequence;
      super(function (...deliveryArgs) {
        const entries = deliveryArgs[0];
        const chosen = recording ? entries.filter((entry) => relevant(entry.target)) : [];
        if (chosen.length) trace('resize.begin', {
          observerId,
          entries: chosen.map((entry) => ({ ...describe(entry.target), width: entry.contentRect.width, height: entry.contentRect.height })),
        });
        const result = Reflect.apply(callback, this, deliveryArgs);
        if (chosen.length) trace('resize.end', { observerId });
        return result;
      }, ...args.slice(1));
    }
  };

  const probe = document.createElement('div');
  probe.className = 'viewport-trace-selfcheck';
  probe.style.transform = 'translateY(42px)';
  probe.style.height = '21px';
  probe.scrollTop = 2;
  probe.scrollTo({ top: 3 });
  const selfcheck = {
    style: events.some((event) => event.type === 'style.set' && event.value === 'translateY(42px)'),
    scrollTop: events.some((event) => event.type === 'scrollTop.set'),
    scrollTo: events.some((event) => event.type === 'scrollTo.before'),
  };
  if (Object.values(selfcheck).some((value) => !value)) {
    throw new Error(`viewport trace selfcheck failed: ${JSON.stringify(selfcheck)}`);
  }
  recording = false;

  // 仅诊断读 React hook 中的当前虚拟器：不调用私有方法、不改其缓存或状态。
  const measurement = (element) => {
    const root = document.getElementById('timelineContentScroll');
    const key = Object.keys(root ?? {}).find((name) => name.startsWith('__reactFiber$'));
    let fiber = key ? root[key] : null;
    for (let depth = 0; fiber && depth < 30; depth++, fiber = fiber.return) {
      let hook = fiber.memoizedState;
      for (let count = 0; hook && count < 200; count++, hook = hook.next) {
        const instance = hook.memoizedState;
        if (!instance?.resizeItem || !instance?.getTotalSize || !instance.options?.enabled) continue;
        const item = instance.measurementsCache[Number(element.dataset.index)];
        return item ? {
          key: item.key, start: item.start, size: item.size, end: item.end, lane: item.lane,
          scrollOffset: instance.scrollOffset,
        } : null;
      }
    }
    return null;
  };

  window.__viewportTrace = {
    selfcheck,
    beginPhase(label) {
      phase = label;
      recording = true;
      trace('phase.begin', {});
    },
    endPhase() {
      trace('phase.end', {});
      recording = false;
    },
    inspect(element) {
      const computed = getComputedStyle(element);
      return {
        nodeId: nodeId(element),
        inlineTransform: element.style.transform,
        computedTransform: computed.transform,
        computedTop: computed.top,
        transition: computed.transition,
        animation: computed.animation,
        measurement: measurement(element),
      };
    },
    dump() {
      return {
        selfcheck, stats, events, dropped,
        userAgent: navigator.userAgent,
        socialSamples: window.__socialAudit?.samples ?? [],
      };
    },
  };
})();
