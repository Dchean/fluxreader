// G2 画廊插桩运行时（临时，仅诊断轮使用，随本轮撤除）。
// 在任何应用代码之前经 Page.addScriptToEvaluateOnNewDocument 注入：
// - window.__g2Trace：事件/帧双缓冲 + 命中统计 + mark/dump/clear；
// - 节点 identity：WeakMap 自增序号，跨事件串联同一 DOM 节点；
// - 拦截 Element.scrollTo / Element.scrollTop setter / style.transform+height
//   setter / ResizeObserver，记录写入值与来源栈；
// - document 捕获监听 scroll / img.load，childList MutationObserver 记录
//   gallery/timeline 虚拟项挂载卸载；
// - rAF + setInterval(12ms) 双口径页面内采样快照。
// - 帧采样由 window.__g2Sampling 门控（工具在 G2 波次窗口置 true），
//   避免全量回归的非画廊阶段耗尽帧缓冲；事件缓冲容量放大到 120000。
// 只读观察：所有拦截仅透传原实现，不改任何行为。
(function () {
  if (window.__g2Trace) return;
  var events = [];
  var frames = [];
  var stats = Object.create(null);
  var dropped = 0;
  var seq = 0;
  var EVENT_CAP = 120000;
  var FRAME_CAP = 4000;
  var nodeIds = new WeakMap();
  var nodeSeq = 0;

  function short(url) {
    return String(url).replace(/^https?:\/\/[^/]+/, '').replace(/.*\/node_modules\/(.vite\/deps\/[^/?]+)/, '/$1');
  }

  function stackSum() {
    var lines = (new Error().stack || '').split('\n').slice(2);
    return lines.slice(0, 14).map(function (line) { return short(line.trim()); });
  }

  function desc(node) {
    if (!node || node.nodeType !== 1) return null;
    var id = nodeIds.get(node);
    if (!id) { nodeSeq += 1; id = nodeSeq; nodeIds.set(node, id); }
    return {
      id: id,
      tag: node.tagName,
      cls: String(node.className && node.className.baseVal === undefined ? node.className : '').slice(0, 60),
      di: node.dataset ? (node.dataset.index ?? null) : null,
      dc: node.dataset ? (node.dataset.cardId ?? null) : null,
      lane: node.dataset ? (node.dataset.galleryLane ?? null) : null,
      dvs: node.dataset ? (node.dataset.virtualStart ?? null) : null,
    };
  }

  window.__g2NodeId = desc;
  window.__g2Stack = stackSum;
  window.__g2Mark = function (label) { window.__g2Trace.mark(label); };

  window.__g2Trace = {
    events: events,
    frames: frames,
    stats: stats,
    push: function (event) {
      var kind = event.k || 'unknown';
      stats[kind] = (stats[kind] || 0) + 1;
      if (events.length >= EVENT_CAP) { dropped += 1; return; }
      event.i = ++seq;
      event.t = performance.now();
      events.push(event);
    },
    mark: function (label) {
      window.__g2PhaseLabel = label;
      window.__g2Trace.push({ k: 'mark', label: label });
    },
    clear: function () {
      events.length = 0;
      frames.length = 0;
      for (var key in stats) delete stats[key];
      dropped = 0;
      seq = 0;
    },
    dump: function () {
      return {
        stats: stats,
        eventCount: events.length,
        dropped: dropped,
        events: events,
        frameCount: frames.length,
        frames: frames,
        meta: {
          href: location.href,
          ua: navigator.userAgent,
          dpr: window.devicePixelRatio,
          resources: performance.getEntriesByType('resource')
            .filter(function (entry) { return /\/src\/|\.vite\/deps\//.test(entry.name); })
            .slice(0, 400)
            .map(function (entry) {
              return { name: short(entry.name), start: +entry.startTime.toFixed(1), dur: +entry.duration.toFixed(1), size: entry.transferSize };
            }),
        },
      };
    },
  };

  // Element.scrollTo：virtual-core elementScroll 与业务消费发布都经过这里。
  var protoScrollTo = Element.prototype.scrollTo;
  Element.prototype.scrollTo = function () {
    var args = Array.prototype.slice.call(arguments);
    var options = args[0] && typeof args[0] === 'object' ? args[0] : {};
    window.__g2Trace.push({
      k: 'dom.scrollTo',
      node: desc(this),
      top: options.top !== undefined ? options.top : null,
      left: options.left !== undefined ? options.left : null,
      behavior: options.behavior ?? null,
      s: stackSum(),
    });
    return protoScrollTo.apply(this, args);
  };

  // Element.scrollTop setter：工具夹具直写与任何绕过 scrollTo 的写入。
  var scrollTopDesc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
  if (scrollTopDesc && scrollTopDesc.set) {
    Object.defineProperty(Element.prototype, 'scrollTop', {
      get: function () { return scrollTopDesc.get.call(this); },
      set: function (value) {
        window.__g2Trace.push({ k: 'dom.scrollTopSet', node: desc(this), v: value, s: stackSum() });
        return scrollTopDesc.set.call(this, value);
      },
      configurable: true,
    });
  }

  // style.transform / style.height setter：geometry 直写通道（adapter/业务/兜底）。
  function patchStyleProp(prop) {
    var styleDesc = Object.getOwnPropertyDescriptor(CSSStyleDeclaration.prototype, prop);
    if (!styleDesc || !styleDesc.set) return;
    Object.defineProperty(CSSStyleDeclaration.prototype, prop, {
      get: function () { return styleDesc.get.call(this); },
      set: function (value) {
        var owner = null;
        try { owner = desc(this.ownerElement); } catch { owner = null; }
        window.__g2Trace.push({ k: 'style.set.' + prop, node: owner, v: String(value).slice(0, 80), s: stackSum() });
        return styleDesc.set.call(this, value);
      },
      configurable: true,
    });
  }
  patchStyleProp('transform');
  patchStyleProp('height');

  // ResizeObserver：virtual-core 的 resizeItem 触发源（图片解码尺寸变化）。
  var RealRO = window.ResizeObserver;
  function WrappedResizeObserver(callback) {
    var ro = new RealRO(function (entries, observer) {
      window.__g2Trace.push({
        k: 'ro.cb',
        e: entries.map(function (entry) {
          return { node: desc(entry.target), w: entry.contentRect.width, h: entry.contentRect.height };
        }),
      });
      return callback(entries, observer);
    });
    var origObserve = ro.observe.bind(ro);
    ro.observe = function (target, options) {
      window.__g2Trace.push({ k: 'ro.observe', node: desc(target), s: stackSum() });
      return origObserve(target, options);
    };
    var origUnobserve = ro.unobserve.bind(ro);
    ro.unobserve = function (target) {
      window.__g2Trace.push({ k: 'ro.unobserve', node: desc(target) });
      return origUnobserve(target);
    };
    var origDisconnect = ro.disconnect.bind(ro);
    ro.disconnect = function () {
      window.__g2Trace.push({ k: 'ro.disconnect' });
      return origDisconnect();
    };
    return ro;
  }
  WrappedResizeObserver.prototype = RealRO.prototype;
  window.ResizeObserver = WrappedResizeObserver;

  // 原生 scroll 事件与图片 load 事件（load 不冒泡，document 捕获可达）。
  document.addEventListener('scroll', function (event) {
    var target = event.target;
    if (!target || target.id !== 'timelineContentScroll') return;
    window.__g2Trace.push({
      k: 'scroll.event',
      trusted: event.isTrusted,
      scrollTop: target.scrollTop,
      scrollHeight: target.scrollHeight,
    });
  }, true);
  document.addEventListener('load', function (event) {
    var target = event.target;
    if (!target || target.tagName !== 'IMG') return;
    var scroller = document.getElementById('timelineContentScroll');
    if (!scroller || !scroller.contains(target)) return;
    var src = String(target.currentSrc || '');
    window.__g2Trace.push({
      k: 'img.load',
      node: desc(target.closest('.gallery-virtual-item, .timeline-virtual-item')),
      naturalWidth: target.naturalWidth,
      naturalHeight: target.naturalHeight,
      src: src,
    });
  }, true);

  // 虚拟项挂载/卸载（childList）。
  var mo = new MutationObserver(function (records) {
    for (var i = 0; i < records.length; i++) {
      var record = records[i];
      for (var j = 0; j < record.addedNodes.length; j++) {
        var added = record.addedNodes[j];
        if (added.nodeType !== 1) continue;
        if (added.matches && added.matches('.gallery-virtual-item, .timeline-virtual-item, .gallery-masonry-grid, .timeline-virtual-wrap')) {
          window.__g2Trace.push({ k: 'dom.added', node: desc(added) });
        }
      }
      for (var k = 0; k < record.removedNodes.length; k++) {
        var removed = record.removedNodes[k];
        if (removed.nodeType !== 1) continue;
        if (removed.matches && removed.matches('.gallery-virtual-item, .timeline-virtual-item, .gallery-masonry-grid, .timeline-virtual-wrap')) {
          window.__g2Trace.push({ k: 'dom.removed', node: desc(removed) });
        }
      }
    }
  });
  mo.observe(document, { childList: true, subtree: true });

  // 页面内双口径采样快照（rAF 与 timer 各自独立推进，互不等待）。
  function snapshot(src) {
    var root = document.getElementById('timelineContentScroll');
    if (!root) return { src: src, t: performance.now(), phase: window.__g2PhaseLabel ?? null, root: null };
    var rootRect = root.getBoundingClientRect();
    var cards = [];
    var items = root.querySelectorAll('.gallery-virtual-item, .timeline-virtual-item');
    for (var i = 0; i < items.length; i++) {
      var element = items[i];
      var rect = element.getBoundingClientRect();
      var img = element.querySelector('img');
      cards.push({
        id: desc(element).id,
        di: element.dataset.index ?? null,
        dc: element.dataset.cardId ?? null,
        lane: element.dataset.galleryLane ?? null,
        top: +(rect.top - rootRect.top).toFixed(2),
        h: +rect.height.toFixed(2),
        tf: element.style.transform,
        nw: img ? img.naturalWidth : null,
      });
    }
    var grid = root.querySelector('.gallery-masonry-grid');
    return {
      src: src,
      t: performance.now(),
      phase: window.__g2PhaseLabel ?? null,
      st: root.scrollTop,
      sh: root.scrollHeight,
      ch: root.clientHeight,
      gridH: grid ? grid.style.height : null,
      cards: cards,
    };
  }
  function pushFrame(src) {
    if (!window.__g2Sampling) return false;
    if (frames.length >= FRAME_CAP) return false;
    frames.push(snapshot(src));
    return true;
  }
  function rafLoop() {
    pushFrame('raf');
    requestAnimationFrame(rafLoop);
  }
  requestAnimationFrame(rafLoop);
  setInterval(function () {
    pushFrame('timer');
  }, 12);
})();
