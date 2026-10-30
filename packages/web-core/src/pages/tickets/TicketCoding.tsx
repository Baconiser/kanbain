import { useCallback, useEffect, useState } from 'react';
import { Button } from '@kanbain/ui/components/Button';
import type {
  OrchestrationStatus,
  OrchestrationView,
  SubtaskStatus,
  SubtaskView,
} from 'shared/types';
import { codingApi } from '@/shared/lib/ticketsApi';

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
  running: 'Coders working',
  stopped: 'Stopped',
  blocked: 'Blocked',
  done: 'All subtasks merged',
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

const usd = (n: number) => `$${n.toFixed(2)}`;

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
  const status = SUBTASK_STATUS[subtask.status];
  const waiting = subtask.status === 'pending' && subtask.waits_for.length > 0;
  return (
    <li
      className="flex flex-col gap-half rounded-sm border p-base"
      data-testid={`coding-subtask-${subtask.plan_subtask_id}`}
      data-status={subtask.status}
    >
      <div className="flex items-center gap-half">
        <span className="font-mono text-xs text-low">
          {subtask.plan_subtask_id}.
        </span>
        <span className="min-w-0 flex-1 truncate text-sm text-normal">
          {subtask.title}
        </span>
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
        {subtask.branch && (
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
      {subtask.files.length > 0 && (
        <p className="truncate font-mono text-xs text-low">
          {subtask.files.join(', ')}
        </p>
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
 * Stop and Retry per subtask and for the whole ticket.
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
  const cost = view.subtasks.reduce((sum, s) => sum + s.cost_usd, 0);

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
        <ul className="flex flex-col gap-half">
          {view.subtasks.map((subtask) => (
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
    </section>
  );
}
