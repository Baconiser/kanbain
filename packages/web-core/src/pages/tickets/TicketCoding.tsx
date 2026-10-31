import { useCallback, useEffect, useState } from 'react';
import { CaretRightIcon } from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import type {
  OrchestrationStatus,
  OrchestrationView,
  SubtaskStatus,
  SubtaskView,
} from 'shared/types';
import { codingApi } from '@/shared/lib/ticketsApi';
import { TicketTesting } from './TicketTesting';

interface TicketCodingProps {
  issueId: string;
  /** Changes when orchestrator or plan events arrive, to reload. */
  refreshKey: number;
  /** An approved plan exists, so coding can start. */
  planApproved: boolean;
  onOpenWorkspace: (workspaceId: string) => void;
  onError: (message: string) => void;
}

const POLL_MS = 3000;

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

const TICKET_STATUS: Record<OrchestrationStatus, string> = {
  running: 'Working',
  stopped: 'Stopped',
  blocked: 'Blocked',
  done: 'Merged and tested',
};

const SUBTASK_STATUS: Record<
  SubtaskStatus,
  { label: string; className: string }
> = {
  pending: { label: 'Waiting', className: 'text-low' },
  running: { label: 'Coding…', className: 'text-normal' },
  merged: { label: 'Merged', className: 'text-success' },
  failed: { label: 'Failed', className: 'text-error' },
  stopped: { label: 'Stopped', className: 'text-low' },
};

const PROGRESS_COLORS: Record<SubtaskStatus, string> = {
  pending: 'bg-low/20',
  running: 'animate-pulse bg-brand',
  merged: 'bg-success',
  failed: 'bg-error',
  stopped: 'bg-low/50',
};

const usd = (n: number) => `$${n.toFixed(2)}`;

type Filter = 'all' | 'open' | 'failed' | 'merged';
const FILTERS: {
  id: Filter;
  label: string;
  matches: (s: SubtaskStatus) => boolean;
}[] = [
  { id: 'all', label: 'All', matches: () => true },
  {
    id: 'open',
    label: 'Open',
    matches: (s) => s === 'pending' || s === 'running',
  },
  {
    id: 'failed',
    label: 'Failed',
    matches: (s) => s === 'failed' || s === 'stopped',
  },
  { id: 'merged', label: 'Merged', matches: (s) => s === 'merged' },
];

function SubtaskRow({
  subtask,
  busy,
  onStop,
  onRetry,
  onOpenWorkspace,
}: {
  subtask: SubtaskView;
  busy: boolean;
  onStop: () => void;
  onRetry: () => void;
  onOpenWorkspace: (workspaceId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const status = SUBTASK_STATUS[subtask.status];
  const waiting = subtask.status === 'pending' && subtask.waits_for.length > 0;
  return (
    <li
      className="flex flex-col gap-half rounded-sm border px-base py-half"
      data-testid={`coding-subtask-${subtask.plan_subtask_id}`}
      data-status={subtask.status}
    >
      <div className="flex items-center gap-half">
        <button
          type="button"
          aria-label={open ? 'Hide details' : 'Show details'}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-half text-left"
          title={subtask.title}
          onClick={() => setOpen((o) => !o)}
        >
          <CaretRightIcon
            weight="bold"
            className={`size-icon-xs shrink-0 text-low transition-transform ${open ? 'rotate-90' : ''}`}
          />
          <span className="font-mono text-xs text-low">
            {subtask.plan_subtask_id}.
          </span>
          <span className="min-w-0 flex-1 truncate text-sm text-normal">
            {subtask.title}
          </span>
        </button>
        <span
          className={`text-xs ${status.className}`}
          data-testid="coding-subtask-status"
        >
          {status.label}
        </span>
        {(subtask.status === 'pending' || subtask.status === 'running') && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="coding-subtask-stop"
            onClick={onStop}
          >
            Stop
          </Button>
        )}
        {(subtask.status === 'failed' || subtask.status === 'stopped') && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="coding-subtask-retry"
            onClick={onRetry}
          >
            Retry
          </Button>
        )}
      </div>
      <div className="flex flex-wrap gap-x-base gap-y-half text-xs text-low">
        {subtask.sub_issue && <span>{subtask.sub_issue}</span>}
        {open && subtask.branch && (
          <span className="font-mono" data-testid="coding-subtask-branch">
            {subtask.branch}
          </span>
        )}
        {waiting && <span>after {subtask.waits_for.join(', ')}</span>}
        {subtask.fix_round > 0 && (
          <span data-testid="coding-subtask-fix-round">
            fix round {subtask.fix_round}
          </span>
        )}
        {subtask.cost_usd > 0 && <span>{usd(subtask.cost_usd)}</span>}
        {subtask.workspace_id && (
          <button
            type="button"
            className="hover:text-normal"
            onClick={() => onOpenWorkspace(subtask.workspace_id!)}
          >
            Workspace
          </button>
        )}
      </div>
      {open && subtask.files.length > 0 && (
        <ul className="flex flex-col font-mono text-xs text-low">
          {subtask.files.map((f) => (
            <li key={f} className="truncate">
              {f}
            </li>
          ))}
        </ul>
      )}
      {subtask.conflict && subtask.status !== 'merged' && (
        <p
          className="text-xs text-normal"
          data-testid="coding-subtask-conflict"
        >
          Merge conflict in {subtask.conflict.files.join(', ')}; re-running on
          the updated ticket branch.
        </p>
      )}
      {subtask.error && <p className="text-xs text-error">{subtask.error}</p>}
    </li>
  );
}

/**
 * The orchestrator on a ticket (Phase 5, D22): the approved plan's subtasks,
 * each coded on its own branch and merged into the ticket branch, with
 * Stop and Retry per subtask and for the whole ticket. Below them, the
 * checkpoints with their tester and fix runs (Phase 6).
 */
export function TicketCoding({
  issueId,
  refreshKey,
  planApproved,
  onOpenWorkspace,
  onError,
}: TicketCodingProps) {
  const [view, setView] = useState<OrchestrationView | null>(null);
  const [busy, setBusy] = useState(false);
  const [filter, setFilter] = useState<Filter>('all');

  const load = useCallback(
    () =>
      codingApi
        .get(issueId)
        .then(setView)
        .catch((err: unknown) =>
          onError(errorMessage(err, 'Failed to load the coding state'))
        ),
    [issueId, onError]
  );

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const status = view?.orchestration?.status;
  useEffect(() => {
    if (status !== 'running') return;
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [status, load]);

  const act = async (
    action: () => Promise<OrchestrationView>,
    fallback: string
  ) => {
    setBusy(true);
    try {
      setView(await action());
    } catch (err) {
      onError(errorMessage(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  if (!view) return null;
  const orchestration = view.orchestration;
  if (!orchestration && !planApproved) return null;
  const merged = view.subtasks.filter((s) => s.status === 'merged').length;
  const cost =
    view.subtasks.reduce((sum, s) => sum + s.cost_usd, 0) +
    view.checkpoints.reduce((sum, c) => sum + c.cost_usd, 0);
  const matches = FILTERS.find((f) => f.id === filter)!.matches;
  const shown = view.subtasks.filter((s) => matches(s.status));

  return (
    <section className="flex flex-col gap-half" data-testid="ticket-coding">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-normal">Coding</h3>
        <div className="flex items-center gap-base">
          {orchestration?.status === 'running' && (
            <Button
              size="xs"
              variant="outline"
              disabled={busy}
              data-testid="coding-stop"
              onClick={() =>
                void act(
                  () => codingApi.stop(issueId),
                  'Failed to stop the coders'
                )
              }
            >
              Stop all
            </Button>
          )}
          {(orchestration?.status === 'stopped' ||
            orchestration?.status === 'blocked') && (
            <Button
              size="xs"
              disabled={busy}
              data-testid="coding-retry"
              onClick={() =>
                void act(
                  () => codingApi.retry(issueId),
                  'Failed to retry the coders'
                )
              }
            >
              Retry
            </Button>
          )}
          {!orchestration && planApproved && (
            <Button
              size="xs"
              disabled={busy}
              data-testid="coding-start"
              onClick={() =>
                void act(
                  () => codingApi.start(issueId),
                  'Failed to start the coders'
                )
              }
            >
              Start coding
            </Button>
          )}
        </div>
      </div>

      {view.subtasks.length > 0 && (
        // One segment per subtask, colored by where it stands.
        <div
          className="flex h-2 gap-px overflow-hidden rounded-full bg-secondary"
          aria-hidden
        >
          {view.subtasks.map((s) => (
            <span
              key={s.id}
              title={`${s.plan_subtask_id}. ${s.title}: ${SUBTASK_STATUS[s.status].label}`}
              className={`flex-1 ${PROGRESS_COLORS[s.status]}`}
            />
          ))}
        </div>
      )}
      {orchestration && (
        <p
          className={`text-sm ${orchestration.status === 'blocked' ? 'text-error' : 'text-low'}`}
          data-testid="coding-status"
          data-status={orchestration.status}
        >
          {TICKET_STATUS[orchestration.status]} · {merged}/
          {view.subtasks.length} merged
          {cost > 0 ? ` · ${usd(cost)}` : ''}
          {orchestration.error ? ` · ${orchestration.error}` : ''}
        </p>
      )}
      {view.ticket_branch && (
        <p className="text-xs text-low">
          Ticket branch{' '}
          <span className="font-mono" data-testid="coding-ticket-branch">
            {view.ticket_branch}
          </span>
          {orchestration?.workspace_id && (
            <>
              {' · '}
              <button
                type="button"
                className="hover:text-normal"
                onClick={() => onOpenWorkspace(orchestration.workspace_id!)}
              >
                Ticket workspace
              </button>
            </>
          )}
        </p>
      )}
      {!orchestration && (
        <p className="text-sm text-low">
          The plan is approved. Start coding to run its subtasks.
        </p>
      )}

      {view.subtasks.length > 0 && (
        <div className="flex flex-wrap gap-half" role="group">
          {FILTERS.map((f) => {
            const count = view.subtasks.filter((s) =>
              f.matches(s.status)
            ).length;
            if (f.id !== 'all' && count === 0) return null;
            return (
              <button
                key={f.id}
                type="button"
                aria-pressed={filter === f.id}
                className={`rounded-sm border px-base py-px text-xs ${
                  filter === f.id
                    ? 'border-brand text-normal'
                    : 'text-low hover:text-normal'
                }`}
                onClick={() => setFilter(f.id)}
              >
                {f.label} {count}
              </button>
            );
          })}
        </div>
      )}

      {view.subtasks.length > 0 && (
        <ul className="flex flex-col gap-half">
          {shown.length === 0 && (
            <li className="text-sm text-low">No subtasks here.</li>
          )}
          {shown.map((subtask) => (
            <SubtaskRow
              key={subtask.id}
              subtask={subtask}
              busy={busy}
              onOpenWorkspace={onOpenWorkspace}
              onStop={() =>
                void act(
                  () => codingApi.stopSubtask(subtask.id),
                  'Failed to stop the subtask'
                )
              }
              onRetry={() =>
                void act(
                  () => codingApi.retrySubtask(subtask.id),
                  'Failed to retry the subtask'
                )
              }
            />
          ))}
        </ul>
      )}

      <TicketTesting
        checkpoints={view.checkpoints}
        issuesFound={view.issues_found}
        busy={busy}
        onOpenWorkspace={onOpenWorkspace}
        onStop={(id) =>
          void act(
            () => codingApi.stopCheckpoint(id),
            'Failed to stop the checkpoint'
          )
        }
        onRetry={(id) =>
          void act(
            () => codingApi.retryCheckpoint(id),
            'Failed to retest the checkpoint'
          )
        }
        onSetIssueStatus={(id, status) =>
          void act(
            () => codingApi.setIssueFoundStatus(id, status),
            'Failed to update the issue'
          )
        }
      />
    </section>
  );
}
