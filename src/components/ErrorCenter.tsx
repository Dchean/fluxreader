import { useAppStore, ERROR_CENTER_MAX } from '../store';
import { Icons } from './icons';
import { ModalOverlay } from './primitives';

/* ============================================================
   OPT-017E：错误中心面板 —— 重试入口持久可达的 UI 落点

   入口在 Sidebar 底部固定区（errors 非空时可见）；本面板列出错误中心条目，
   逐条可重试（retryError 派发 + 清除）/忽略（dismissError）/全部清除。
   普通 toast 不经过这里；面板开态 errorCenterOpen 已登记进浮层让路清单
   （shortcutYield.ts）并由 App 的 Esc 链关闭。
   上限/去重键/重试消费语义与取舍见
   .agents/notes/implemented/feature/2026-10-16-重试入口持久可达与错误中心.md
   ============================================================ */

export function ErrorCenterDialog() {
  const open = useAppStore((s) => s.errorCenterOpen);
  const errors = useAppStore((s) => s.errors);
  const closeErrorCenter = useAppStore((s) => s.closeErrorCenter);
  const retryError = useAppStore((s) => s.retryError);
  const dismissError = useAppStore((s) => s.dismissError);
  const clearErrors = useAppStore((s) => s.clearErrors);

  return (
    <ModalOverlay open={open} onClose={closeErrorCenter}>
      <div className="mini-dialog error-center-dialog">
        <div className="error-center-head">
          <div className="mini-dialog-title">错误中心</div>
          <button className="error-center-close" onClick={closeErrorCenter} title="关闭" aria-label="关闭错误中心">
            <Icons.x />
          </button>
        </div>
        <div className="mini-dialog-hint" style={{ marginTop: 4 }}>
          失败的操作保留在这里，可随时重试；同一错误合并为一条（保留最新），最多保留 {ERROR_CENTER_MAX} 条。
        </div>

        {errors.length === 0 ? (
          <div className="error-center-empty">暂无待处理的错误</div>
        ) : (
          /* 最新在前：与「同 key 合并最新」的顺序口径一致（store 内最早的在前） */
          <div className="error-center-list" role="list">
            {[...errors].reverse().map((e) => (
              <div className="error-center-item" key={e.id} role="listitem">
                <span className="error-center-text" title={e.text}>
                  {e.text}
                  {e.count > 1 ? `（已失败 ${e.count} 次）` : ''}
                </span>
                <button className="error-center-retry" onClick={() => retryError(e.id)}>
                  {e.action.label}
                </button>
                <button
                  className="error-center-dismiss"
                  onClick={() => dismissError(e.id)}
                  title="忽略此条"
                  aria-label="忽略此条"
                >
                  <Icons.x />
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="mini-dialog-actions">
          <button className="toggle-action-btn" disabled={errors.length === 0} onClick={clearErrors}>
            全部清除
          </button>
          <button className="toggle-action-btn btn-primary" onClick={closeErrorCenter}>
            关闭
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}
