import type { CSSProperties, MouseEvent } from 'react';

/* ============================================================
   ReaderProse —— 阅读器正文的「源码 / 渲染」分离呈现（审计 F14 / OPT-012）

   为什么必须是独立组件、且源码态必须离开 dangerouslySetInnerHTML：
   修前「源码」按钮只给同一个 HTML 容器多挂一个 raw-render-mode 类
   （审计 F14 实测：点击前后 innerHTML/textContent 完全一致）——源码从未
   被真正展示。若只改为把 HTML 转义成字符串再塞进 dangerouslySetInnerHTML，
   转义反而是手工易错点；正确的做法是让源码走**文本节点插值**，由 React
   统一转义：源码里的 <p>/<b>/<img onerror=…> 只作为字面文字可见，不创建
   元素、不执行属性。

   三条呈现路径（props 由 Reader.tsx 单点计算，值传递含义见 props 注释）：
   - 源码态   ：<pre class="reader-source-view"> 文本节点（转义展示）；
   - 流式译文 ：纯文本插值——未消毒产物任何模式都不进 HTML 创建路径
                （TASK-065 N11 契约保留，只是从 Reader.tsx 移入本组件）；
   - 渲染态   ：现有 .article-prose + dangerouslySetInnerHTML（消毒后 HTML），
                链接/图片点击代理由调用方传入的 onClick 照旧处理。

   决定与取舍见 .agents/notes/implemented/bug-fix/2026-10-08-阅读器源码显示契约.md；
   真实 SSR 输出回归见 tools/frontend-regression.mjs 的 OPT-012 块。
   ============================================================ */

export interface ReaderProseProps {
  /** 渲染态 HTML：RSS 原文 / 提取全文 / 已消毒译文；图片代理命中时为代理产物（data: URL） */
  renderHtml: string;
  /** 源码态展示的原始文本：与 renderHtml 同源，但**不经图片代理**——
      源码要显示原始 src，不能用代理产物的 base64 大块冒充（流式译文即其本文） */
  sourceText: string;
  /** true = 源码态（转义文本，不创建标签） */
  isSourceMode: boolean;
  /** true = 当前为未消毒流式译文（纯文本路径，任何模式都不进 HTML 渲染） */
  isStreamingTranslation: boolean;
  /** 排版（字体/字号/行高）：与修前内联样式一致，由调用方传入 */
  style: CSSProperties;
  /** 正文点击代理（外链/灯箱）：源码态为纯文本、无可交互元素，挂上仅为与渲染态一致 */
  onClick: (e: MouseEvent<HTMLDivElement>) => void;
}

export function ReaderProse({
  renderHtml,
  sourceText,
  isSourceMode,
  isStreamingTranslation,
  style,
  onClick,
}: ReaderProseProps) {
  if (isSourceMode) {
    /* Note: 源码态必须是文本节点插值（React 负责转义）——改回 dangerouslySetInnerHTML
       即恢复 F14（源码被当 HTML 创建/执行）。见 .agents/notes/implemented/bug-fix/2026-10-08-阅读器源码显示契约.md */
    return (
      <div className="article-prose raw-render-mode" style={style} onClick={onClick}>
        <pre className="reader-source-view">{sourceText}</pre>
      </div>
    );
  }
  if (isStreamingTranslation) {
    /* 未消毒流式译文：纯文本插值（TASK-065 N11），不进 HTML 渲染路径 */
    return (
      <div className="article-prose" style={style} onClick={onClick}>
        {renderHtml}
      </div>
    );
  }
  return (
    <div
      className="article-prose"
      style={style}
      onClick={onClick}
      dangerouslySetInnerHTML={{ __html: renderHtml }}
    />
  );
}
