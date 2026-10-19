// OPT-016B：ReaderProse 源码态/渲染态 DOM 契约的可复用断言。
//
// 为什么要独立成模块：同一组断言必须同时服务两处——
//   1) 主用例：挂载真实 Reader（生产消费路径），切到源码态后对 DOM 取证；
//   2) 变异取证：对 src/components/ReaderProse.tsx 的 tmp 副本做「源码态改回
//      dangerouslySetInnerHTML」的条件变异，用**同一组检查函数**证明断言能判红。
// 两处用同一份实现，变异取证的结论才对主用例有效（不是两套平行逻辑）。

/** 源码态契约检查（返回全部结果，而不是抛出——变异用例要逐条报告哪些被抓到）。 */
export function sourceModeChecks(scope, expectedText) {
  const prose = scope.querySelector('.article-prose');
  return [
    {
      name: 'source-mode: <script> 元素未创建（危险标签不进 HTML 创建路径）',
      pass: !!prose && prose.querySelector('script') === null,
      detail: `script 元素数=${prose ? prose.querySelectorAll('script').length : 'no-prose'}`,
    },
    {
      name: 'source-mode: <img> 元素未创建（onerror 属性不成为可执行属性）',
      pass: !!prose && prose.querySelector('img') === null,
      detail: `img 元素数=${prose ? prose.querySelectorAll('img').length : 'no-prose'}`,
    },
    {
      name: 'source-mode: <a> 元素未创建（源码是文本，不是可点击链接）',
      pass: !!prose && prose.querySelector('a') === null,
      detail: `a 元素数=${prose ? prose.querySelectorAll('a').length : 'no-prose'}`,
    },
    {
      name: 'source-mode: 源码以字面文本可见（textContent 含 <script> 与 onerror 原文）',
      pass: !!prose && prose.textContent.includes('<script>') && prose.textContent.includes('onerror'),
      detail: `textContent=${prose ? JSON.stringify(prose.textContent.slice(0, 60)) : 'no-prose'}`,
    },
    {
      name: 'source-mode: 源码文本与传入 sourceText 逐字一致（无转义漂移/无代理替换）',
      pass: !!prose && prose.textContent === expectedText,
      detail: `相等=${!!prose && prose.textContent === expectedText}`,
    },
    {
      name: 'source-mode: 宿主容器为 .article-prose.raw-render-mode（样式/点击宿主不丢）',
      pass: !!prose && prose.classList.contains('article-prose') && prose.classList.contains('raw-render-mode'),
      detail: prose ? prose.className : 'no-prose',
    },
  ];
}

/** 渲染态契约检查：合法元素真实存在（与源码态的「不存在」形成判别对）。 */
export function renderModeChecks(scope) {
  const prose = scope.querySelector('.article-prose');
  return [
    {
      name: 'render-mode: 合法 <p> 存在',
      pass: !!prose?.querySelector('p.safe-p'),
      detail: `p.safe-p=${!!prose?.querySelector('p.safe-p')}`,
    },
    {
      name: 'render-mode: 消毒后内容里的 <script> 节点按 innerHTML 语义创建（不等于执行）',
      pass: !!prose?.querySelector('script'),
      detail: `script 节点=${!!prose?.querySelector('script')}`,
    },
    {
      name: 'render-mode: <img> 与 <a> 存在（渲染态是可交互 HTML）',
      pass: !!prose?.querySelector('img') && !!prose?.querySelector('a'),
      detail: `img=${!!prose?.querySelector('img')} a=${!!prose?.querySelector('a')}`,
    },
    {
      name: 'render-mode: 不再是 raw-render-mode 类（已切回渲染态）',
      pass: !!prose && !prose.classList.contains('raw-render-mode'),
      detail: prose ? prose.className : 'no-prose',
    },
  ];
}
