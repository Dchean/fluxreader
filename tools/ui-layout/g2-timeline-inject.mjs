// G2 诊断插桩（临时，随修复提交一并撤除）：对 vite dev 服务的 Timeline 模块
// 文本注入 trace 事件。产品源码零改动；事件 schema 与本地批次
// tmp/frontend/gallery-ui-r1/g2-trace-r1 一致。锚点基于 dev 转换输出
// （LF、tab 缩进、双引号），任何一处计数不符立即抛错，不做静默降级。

const TRACE_PUSH = 'window.__g2Trace?.push?.(';
const TRACE_STACK = 'window.__g2Stack?.()';

export function injectTimelineTrace(source) {
  let out = source;
  const apply = (label, pattern, replace, expected = 1) => {
    const count = (out.match(new RegExp(pattern.source, `${pattern.flags}g`)) ?? []).length;
    if (count !== expected) {
      throw new Error(`g2 模块注入 ${label}：期望 ${expected} 处，实际 ${count} 处，模块文本与锚点不符`);
    }
    out = out.replace(pattern, replace);
  };

  // TimelineRowItem layout effect：行虚拟项 transform 直写。
  apply(
    'row.itemEffect',
    /(\t+if \(element\) element\.style\.transform = `translateY\(\$\{vi\.start\}px\)`;)/,
    (match, line) => `${line}\n\t\t\t${TRACE_PUSH}{ k: "row.itemEffect", index: vi.index, cardId: item.id, start: vi.start, wrote: Boolean(element), s: ${TRACE_STACK} });`,
  );

  // 两个 scrollToFn：补偿生产入口（row 在前、gallery 在后，以 ref 名区分）。
  apply(
    'row.scrollToFn',
    /(scrollToFn: \(offset, options, instance\) => \{\n)(\t+if \(options\.adjustments\) \{\n\t+rowPendingScrollRef)/,
    (match, head, tail) => `${head}\t\t\t${TRACE_PUSH}{ k: "row.scrollToFn", offset, adjustments: options.adjustments ?? 0, behavior: options.behavior ?? null, s: ${TRACE_STACK} });\n${tail}`,
  );
  apply(
    'gallery.scrollToFn',
    /(scrollToFn: \(offset, options, instance\) => \{\n)(\t+if \(options\.adjustments\) \{\n\t+galleryPendingScrollRef)/,
    (match, head, tail) => `${head}\t\t\t${TRACE_PUSH}{ k: "gallery.scrollToFn", offset, adjustments: options.adjustments ?? 0, behavior: options.behavior ?? null, scrollOffset: instance.scrollOffset ?? null, s: ${TRACE_STACK} });\n${tail}`,
  );

  // onChange 链入口（row 链以 syncRowGeometryNow + 行 pending 消费为特征）。
  apply(
    'row.onChange',
    /(\t+)(syncRowGeometryNow\(\);\n\t+consumeDeferredScroll\(rowPendingScrollRef, instance\);)/,
    (match, indent, tail) => `${indent}${TRACE_PUSH}{ k: "row.onChange", s: ${TRACE_STACK} });\n${indent}${tail}`,
  );
  apply(
    'gallery.onChange',
    /(\t+)(syncGalleryGeometry\(instance\);\n\t+const next = galleryWindowKey\()/,
    (match, indent, tail) => `${indent}${TRACE_PUSH}{ k: "gallery.onChange", scrollOffset: instance.scrollOffset ?? null, s: ${TRACE_STACK} });\n${indent}${tail}`,
  );

  // consumeDeferredScroll：pending 消费判定（rejected-guard / published）。
  apply(
    'pending.consume rejected',
    /(\t+)(if \(element && Math\.abs\(pending\.offset - element\.scrollTop\) > 1\.5\) return;)/,
    (_match, indent) => `${indent}if (element && Math.abs(pending.offset - element.scrollTop) > 1.5) {\n`
      + `${indent}\t${TRACE_PUSH}{ k: "pending.consume", lane: pendingRef === galleryPendingScrollRef ? "gallery" : "row", verdict: "rejected-guard", offset: pending.offset, adjustments: pending.adjustments, scrollTop: element.scrollTop, s: ${TRACE_STACK} });\n`
      + `${indent}\treturn;\n${indent}}`,
  );
  apply(
    'pending.consume published',
    /(\t+)(instance\.scrollElement\?\.scrollTo\?\.\(\{)/,
    (match, indent, tail) => `${indent}${TRACE_PUSH}{ k: "pending.consume", lane: pendingRef === galleryPendingScrollRef ? "gallery" : "row", verdict: "published", offset: pending.offset, adjustments: pending.adjustments, scrollTop: element?.scrollTop ?? null, s: ${TRACE_STACK} });\n${indent}${tail}`,
  );

  // syncGalleryGeometry：容器高 + 全部已挂节点 transform 直写（含 unmatched 记录）。
  apply(
    'gallery.geometry decl',
    /(\t+)(if \(container\) container\.style\.height = `\$\{height\}px`;)/,
    (match, indent, line) => `${indent}${line}\n${indent}const traceWrites = [];`,
  );
  apply(
    'gallery.geometry loop',
    /\t+for \(const element of instance\.elementsCache\.values\(\)\) \{\n\t+const measurement = instance\.measurementsCache\[Number\(element\.getAttribute\("data-index"\)\)\];\n\t+if \(measurement && String\(measurement\.key\) === element\.dataset\.cardId\) \{\n\t+element\.style\.transform = `translateY\(\$\{measurement\.start\}px\)`;\n\t+\}\n\t+\}/,
    () => `\t\tfor (const element of instance.elementsCache.values()) {\n`
      + `\t\t\tconst measurement = instance.measurementsCache[Number(element.getAttribute("data-index"))];\n`
      + `\t\t\tconst matched = Boolean(measurement && String(measurement.key) === element.dataset.cardId);\n`
      + `\t\t\tif (matched) {\n`
      + `\t\t\t\telement.style.transform = \`translateY(\${measurement.start}px)\`;\n`
      + `\t\t\t}\n`
      + `\t\t\ttraceWrites.push({ index: element.getAttribute("data-index"), cardId: element.dataset.cardId ?? null, start: measurement ? measurement.start : null, matched });\n`
      + `\t\t}`,
  );
  apply(
    'gallery.geometry push',
    /(\t+)(consumeDeferredScroll\(galleryPendingScrollRef, instance\);)/,
    (match, indent, tail) => `${indent}${TRACE_PUSH}{ k: "gallery.geometry", height, scrollOffset: instance.scrollOffset ?? null, writes: traceWrites, s: ${TRACE_STACK} });\n${indent}${tail}`,
  );

  // shouldAdjustScrollPositionOnItemSizeChange：补偿判定三分支。
  apply(
    'adjustDecision inactive',
    /(\t+)(if \(!root \|\| root\.scrollTop <= 0 \|\| activeContentLayout !== "image"\) return false;)/,
    (match, indent, line) => `${indent}${TRACE_PUSH}{ k: "gallery.adjustDecision", index: item.index, key: String(item.key), lane: item.lane, delta, scrollTop: root?.scrollTop ?? null, layout: activeContentLayout, adjust: false, reason: "inactive-or-top" });\n${indent}${line}`,
  );
  apply(
    'adjustDecision no-anchor',
    /(\t+)(if \(!element\?\.dataset\.cardId\) return false;)/,
    (match, indent, line) => `${indent}${TRACE_PUSH}{ k: "gallery.adjustDecision", index: item.index, key: String(item.key), lane: item.lane, delta, scrollTop: root?.scrollTop ?? null, layout: activeContentLayout, adjust: false, reason: "no-anchor-candidate" });\n${indent}${line}`,
  );
  apply(
    'adjustDecision ok',
    /(\t+)(const adjust = item\.lane === anchor\.lane && item\.index < anchor\.index;)(\n\t+if \(adjust\) anchor\.scrollTop \+= delta;)/,
    (match, indent, decision, tail) => `${indent}${decision}\n`
      + `${indent}${TRACE_PUSH}{ k: "gallery.adjustDecision", index: item.index, key: String(item.key), lane: item.lane, delta, scrollTop: root?.scrollTop ?? null, layout: activeContentLayout, adjust, reason: "ok", anchorId: anchor.id, anchorIndex: anchor.index, anchorLane: anchor.lane });`
      + `${tail}`,
  );

  // measureGalleryElement：ref attach/detach 与当次测量。
  apply(
    'gallery.refDetach',
    /(\t+)(galleryVirtualizer\.measureElement\(element\);\n\t+if \(!element\) return;)/,
    (_match, indent) => `${indent}galleryVirtualizer.measureElement(element);\n`
      + `${indent}if (!element) ${TRACE_PUSH}{ k: "gallery.refDetach", s: ${TRACE_STACK} });\n`
      + `${indent}if (!element) return;`,
  );
  apply(
    'gallery.refAttach',
    /(\t+)(const measurement = galleryVirtualizer\.measurementsCache\[Number\(element\.dataset\.index\)\];)/,
    (match, indent, line) => `${indent}${line}\n`
      + `${indent}${TRACE_PUSH}{ k: "gallery.refAttach", index: element.dataset.index, cardId: element.dataset.cardId ?? null, node: window.__g2NodeId?.(element) ?? null, measurementStart: measurement ? measurement.start : null, wroteTransform: Boolean(measurement), s: ${TRACE_STACK} });`,
  );

  // 布局切换兜底消费 effect 入口。
  apply(
    'parent.layoutEffect',
    /(useLayoutEffect\(\(\) => \{\n)(\t+)\/\* 两个虚拟化实例共用同一滚动容器/,
    (match, head, indent) => `${head}${indent}${TRACE_PUSH}{ k: "parent.layoutEffect", layout: activeContentLayout, sameLayout: activeLayoutRef.current === activeContentLayout });\n${indent}/* 两个虚拟化实例共用同一滚动容器`,
  );

  return out;
}
