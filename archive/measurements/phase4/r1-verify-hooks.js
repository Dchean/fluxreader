(() => {
  /* 人工注入的验证钩子（只用于 R1 验收，不属于工具本身）：
     1) F2 复现：进 lockWaitProxy 段时把页面的「同步在飞」标志置真（mock 下该段会 skipped，但真实
        运行里这段会触发一次同步）。后续 longSession / memoryByLayout 的条件快照应各自带上 true，
        结论行也应印 true——而不是借用电池开头的 false。
     2) F6 复现：memoryByLayout 遍历到「通知」时把 #root 卸载（模拟渲染树被打崩），
        该布局的 heapUsedMB 应变为 null 并在结论行印「失效（渲染树被卸载）」。 */
  const origLock = window.__t128.lockSample;
  window.__t128.lockSample = function () {
    window.__t128SyncInFlight = true;
    return origLock.apply(null, arguments);
  };
  const origBtn = window.__t128.layoutButton;
  let notifCalls = 0;
  window.__t128.layoutButton = function (name) {
    if (name === '通知') {
      notifCalls += 1;
      if (notifCalls >= 2) {
        const root = document.getElementById('root');
        if (root) root.replaceChildren();
        window.__t128Errors = window.__t128Errors || [];
        window.__t128Errors.push({ atMs: Math.round(performance.now()), kind: 'test-hook', msg: 'R1-F6 验证钩子：卸载 #root 模拟渲染树崩溃' });
      }
    }
    return origBtn.apply(null, arguments);
  };
  return 't128-r1-verify-hooks-installed';
})()
