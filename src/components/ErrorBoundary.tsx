import { Component, type ErrorInfo, type ReactNode } from 'react';

// Note: 边界放产品树顶层、为什么不在每个卡片包 — 见 .agents/notes/implemented/architecture/2026-11-01-产品树错误边界放顶层.md

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * 产品树顶层错误边界：渲染崩溃不白屏，出全屏兜底态（error-boundary-splash
 * 自带全屏居中，文案/重试样式复用启动失败态的 bootstrap-error）。
 * 重试按钮只清除边界自身状态——React 重新渲染子树；瞬态错误（竞态、时序）
 * 由此恢复，确定性崩溃会再次落入兜底态（不静默吞掉）。
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    /* componentStack 是崩溃归因的唯一现场证据（指出错的是组件树哪一段）——
       顶层单点记录。缺了它，崩溃只能靠复现猜（roadmap NotifCard #185 教训）。 */
    console.error('[ErrorBoundary] 产品树渲染崩溃:', error, '\ncomponentStack:', info.componentStack);
  }

  private reset = () => this.setState({ error: null });

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="error-boundary-splash bootstrap-error" role="alert">
        <div className="app-loading-logo"><img src="/logo.svg" alt="" draggable={false} /></div>
        <p className="bootstrap-error-text">界面渲染出现异常：{error.message || String(error)}</p>
        <button className="bootstrap-error-retry" onClick={this.reset}>重试</button>
      </div>
    );
  }
}
