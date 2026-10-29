import type { SearchHit, TicketRef } from 'shared/types';
import { repoApi } from '@/shared/lib/api';

interface CodeResultLineProps {
  hit: SearchHit;
  repoName?: string | null;
  /** Tickets whose commits changed these lines (E19). */
  changedBy?: TicketRef[];
  onOpenTicket?: (ticket: TicketRef) => void;
  onError?: (message: string) => void;
}

/** A code location: path, lines and symbol; opens the file in the editor. */
export function CodeResultLine({
  hit,
  repoName,
  changedBy = [],
  onOpenTicket,
  onError,
}: CodeResultLineProps) {
  const lines =
    hit.start_line != null && hit.end_line != null
      ? `:${hit.start_line}-${hit.end_line}`
      : '';
  const symbol = hit.symbol?.split(' › ').pop();
  const open = () => {
    if (!hit.repo_id || !hit.path) return;
    repoApi
      .openEditor(hit.repo_id, { editor_type: null, file_path: hit.path })
      .then((r) => {
        if (r.url) window.open(r.url, '_blank');
      })
      .catch((err: unknown) =>
        onError?.(err instanceof Error ? err.message : 'Failed to open editor')
      );
  };
  return (
    <div
      className="flex flex-col gap-half rounded-sm px-base py-half hover:bg-secondary"
      data-testid="code-result"
    >
      <div className="flex flex-wrap items-baseline gap-x-base text-sm">
        <button
          type="button"
          className="font-mono text-xs text-brand hover:underline"
          onClick={open}
          title="Open in editor"
        >
          {repoName ? `${repoName}/` : ''}
          {hit.path}
          {lines}
        </button>
        {hit.symbol && (
          <span className="font-mono text-xs text-normal">{hit.symbol}</span>
        )}
      </div>
      {changedBy.length > 0 && (
        <div className="flex flex-wrap gap-x-base text-xs text-low">
          {changedBy.map((t) => (
            <button
              key={t.id}
              type="button"
              className="hover:text-normal"
              onClick={() => onOpenTicket?.(t)}
            >
              {t.internal_id} changed {symbol ? `\`${symbol}\`` : 'this'}
            </button>
          ))}
        </div>
      )}
      <pre className="line-clamp-4 whitespace-pre-wrap font-mono text-xs text-low">
        {hit.excerpt}
      </pre>
    </div>
  );
}
