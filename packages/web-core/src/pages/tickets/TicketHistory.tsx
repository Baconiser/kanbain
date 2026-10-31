import { useCallback, useEffect, useState } from 'react';
import { Button } from '@kanbain/ui/components/Button';
import type { IssueHistoryView } from 'shared/types';
import { MarkdownPreview } from '@/shared/components/MarkdownPreview';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { historyApi } from '@/shared/lib/ticketsApi';
import { getActualTheme } from '@/shared/lib/theme';
import { CollapsibleSection } from './CollapsibleSection';

interface TicketHistoryProps {
  issueId: string;
  /** Changes when history events arrive, to reload. */
  refreshKey: number;
  onError: (message: string) => void;
}

const POLL_MS = 2000;
const AGENT_AUTHOR = 'KanbAIn agent';
// Longer file lists show this many until expanded.
const FILES_SHOWN = 8;

function usd(value: number): string {
  return `$${value.toFixed(value > 0 && value < 0.01 ? 4 : 2)}`;
}

/**
 * Prior work found in the git history (D18): the permission prompt, the
 * history agent's summary, and the commits and files linked to the ticket
 * (E19).
 */
export function TicketHistory({
  issueId,
  refreshKey,
  onError,
}: TicketHistoryProps) {
  const { config } = useUserSystem();
  const [view, setView] = useState<IssueHistoryView | null>(null);
  const [busy, setBusy] = useState(false);
  const [showCommits, setShowCommits] = useState(false);
  const [showAllFiles, setShowAllFiles] = useState(false);

  const load = useCallback(
    () =>
      historyApi
        .get(issueId)
        .then(setView)
        .catch((err: unknown) =>
          onError(err instanceof Error ? err.message : 'Failed to load history')
        ),
    [issueId, onError]
  );

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const status = view?.history?.status;
  useEffect(() => {
    if (status !== 'analyzing') return;
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [status, load]);

  const act = async (action: 'allow' | 'deny' | 'check') => {
    setBusy(true);
    try {
      setView(await historyApi[action](issueId));
    } catch (err) {
      onError(err instanceof Error ? err.message : 'History action failed');
    } finally {
      setBusy(false);
    }
  };

  if (!view) return null;
  const history = view.history;
  const priorCount = history?.prior_count ?? 0;
  const needsPermission =
    !!history &&
    priorCount > 0 &&
    history.permission === 'pending' &&
    !view.always_allow &&
    !history.prior_work;

  const meta =
    !history || history.commit_count === 0
      ? 'none found'
      : [
          `${view.commits.length} commit${view.commits.length === 1 ? '' : 's'}`,
          view.files.length > 0 &&
            `${view.files.length} file${view.files.length === 1 ? '' : 's'}`,
          history.prior_work && 'summarized',
        ]
          .filter(Boolean)
          .join(' · ');
  const shownFiles = showAllFiles
    ? view.files
    : view.files.slice(0, FILES_SHOWN);

  return (
    <CollapsibleSection
      id="prior-work"
      title="Prior work"
      meta={meta}
      defaultOpen={false}
      // Asking, working and failing all want the user's eyes.
      forceOpen={
        needsPermission || status === 'analyzing' || status === 'failed'
      }
      testId="ticket-history"
      action={
        <button
          type="button"
          className="text-xs text-low hover:text-normal"
          disabled={busy}
          onClick={() => void act('check')}
        >
          Check git history
        </button>
      }
    >
      {!history || history.commit_count === 0 ? (
        <p className="text-sm text-low">
          No commits mention {history?.searched_id ?? 'this ticket'} yet.
        </p>
      ) : null}

      {needsPermission && (
        <div
          className="flex flex-col gap-half rounded-sm border border-brand/40 bg-brand/10 p-base text-sm"
          data-testid="history-permission"
        >
          <p>
            Found {priorCount} commit{priorCount === 1 ? '' : 's'} for{' '}
            <strong>{history.searched_id}</strong>. Allow analysis? The history
            agent reads their messages and diffs (up to about 200 KB).
          </p>
          <div className="flex gap-half">
            <Button size="sm" disabled={busy} onClick={() => void act('allow')}>
              Allow
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void act('deny')}
            >
              Not now
            </Button>
          </div>
        </div>
      )}

      {history?.permission === 'denied' && !history.prior_work && (
        <p className="text-sm text-low">
          Analysis not allowed.{' '}
          <button
            type="button"
            className="text-brand hover:underline"
            onClick={() => void act('allow')}
          >
            Allow now
          </button>
        </p>
      )}

      {status === 'analyzing' && (
        <p className="text-sm text-low" data-testid="history-analyzing">
          The history agent is summarizing {priorCount} commit
          {priorCount === 1 ? '' : 's'}…
        </p>
      )}

      {status === 'failed' && (
        <p className="text-sm text-error">
          Analysis failed{history?.error ? `: ${history.error}` : ''}.{' '}
          <button
            type="button"
            className="text-brand hover:underline"
            onClick={() => void act('allow')}
          >
            Retry
          </button>
        </p>
      )}

      {history?.prior_work && (
        <div
          className="rounded-sm border bg-secondary p-base"
          data-testid="prior-work"
        >
          <MarkdownPreview
            content={history.prior_work}
            theme={getActualTheme(config?.theme)}
            className="text-sm"
          />
          {view.job && (
            <p className="mt-half text-xs text-low">
              {view.job.agent_def_id} · {view.job.model} ·{' '}
              {view.job.tokens_in + view.job.tokens_out} tokens ·{' '}
              {usd(view.job.cost_usd)}
              {history.analyzed_hash !== history.prior_hash &&
                ' · new commits since this summary'}
            </p>
          )}
        </div>
      )}

      {view.files.length > 0 && (
        <div className="flex flex-col gap-half" data-testid="ticket-files">
          <h4 className="text-xs font-medium uppercase text-low">
            Files changed ({view.files.length})
          </h4>
          <ul className="flex flex-col text-sm">
            {shownFiles.map((f) => (
              <li
                key={`${f.repo_id}:${f.path}:${f.symbol}`}
                className="flex items-baseline gap-base"
              >
                <span className="truncate font-mono text-xs" title={f.path}>
                  {f.path}
                </span>
                <span className="shrink-0 text-xs text-success">
                  +{f.additions}
                </span>
                <span className="shrink-0 text-xs text-error">
                  −{f.deletions}
                </span>
                <span className="shrink-0 text-xs text-low">
                  {f.commits} commit{f.commits === 1 ? '' : 's'} · {f.repo_name}
                </span>
              </li>
            ))}
          </ul>
          {view.files.length > FILES_SHOWN && (
            <button
              type="button"
              className="self-start text-xs text-low hover:text-normal"
              onClick={() => setShowAllFiles((v) => !v)}
            >
              {showAllFiles ? 'Show fewer' : `Show all ${view.files.length}`}
            </button>
          )}
        </div>
      )}

      {view.commits.length > 0 && (
        <div className="flex flex-col gap-half">
          <button
            type="button"
            className="self-start text-xs text-brand hover:underline"
            onClick={() => setShowCommits((v) => !v)}
          >
            {showCommits ? 'Hide' : 'Show'} {view.commits.length} commit
            {view.commits.length === 1 ? '' : 's'}
          </button>
          {showCommits && (
            <ul
              className="flex flex-col gap-half text-sm"
              data-testid="ticket-commits"
            >
              {view.commits.map((c) => (
                <li key={`${c.repo_id}:${c.sha}`} className="flex flex-col">
                  <span>
                    <span className="font-mono text-xs text-low">
                      {c.sha.slice(0, 10)}
                    </span>{' '}
                    {c.subject}
                  </span>
                  <span className="text-xs text-low">
                    {c.author === AGENT_AUTHOR ? 'agent' : c.author} ·{' '}
                    {new Date(c.committed_at).toLocaleDateString()} ·{' '}
                    {c.repo_name}
                    {c.branches.length > 0 && ` · ${c.branches.join(', ')}`}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </CollapsibleSection>
  );
}
