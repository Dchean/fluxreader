(() => {
  /* R1 验收钩子（F6 专用）：让「通知」布局在**测量读数取完之后、健康检查之前**崩掉，
     命中 memoryByLayout 的 appCrashedHere 分支（正是审计 R1-F6 描述的情形）。
     手法：记录最近一次 layoutButton(...) 的布局名；健康检查（appHealth）被调用时，若最近一次
     是「通知」且尚未触发，则先卸载 #root（模拟渲染树被打崩）再回报健康状态。 */
  const origBtn = window.__t128.layoutButton;
  const origHealth = window.__t128.appHealth;
  let lastLayout = null;
  let fired = false;
  window.__t128.layoutButton = function (name) {
    lastLayout = name;
    return origBtn.apply(null, arguments);
  };
  window.__t128.appHealth = function () {
    if (lastLayout === '通知' && !fired) {
      fired = true;
      const root = document.getElementById('root');
      if (root) root.replaceChildren();
      window.__t128Errors = window.__t128Errors || [];
      window.__t128Errors.push({ atMs: Math.round(performance.now()), kind: 'test-hook', msg: 'R1-F6 验证钩子：取完通知布局读数后卸载 #root' });
    }
    return origHealth.apply(null, arguments);
  };
  return { hook: 't128-r1-f6-hook-installed' };
})()
