export function HydrateFailure({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div className="hydrate-failure" role="group" aria-label="正文加载失败">
      <span className="hydrate-failure-text" title={message}>正文暂时无法加载</span>
      <button
        type="button"
        className="hydrate-retry"
        aria-label="重试加载正文"
        onClick={onRetry}
      >
        重试
      </button>
    </div>
  );
}
