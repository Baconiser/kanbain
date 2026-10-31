import { useState } from 'react';
import { CaretRightIcon } from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import type {
  CheckpointStatus,
  CheckpointView,
  IssueFound,
  IssueFoundStatus,
  IssueSeverity,
} from 'shared/types';

const CHECKPOINT_STATUS: Record<
  CheckpointStatus,
  { label: string; className: string }
> = {
  pending: { label: 'Waiting', className: 'text-low' },
  testing: { label: 'Testing…', className: 'text-normal' },
  fixing: { label: 'Fixing…', className: 'text-normal' },
  passed: { label: 'Passed', className: 'text-success' },
  failed: { label: 'Failed', className: 'text-error' },
  stopped: { label: 'Stopped', className: 'text-low' },
};

const SEVERITY_CLASS: Record<IssueSeverity, string> = {
  high: 'text-error',
  medium: 'text-normal',
  low: 'text-low',
};

const ISSUE_STATUS: Record<IssueFoundStatus, string> = {
  open: 'Open',
  fixed: 'Fixed',
  wontfix: "Won't fix",
};

const usd = (n: number) => `$${n.toFixed(2)}`;

function where(issue: IssueFound): string | null {
  if (!issue.file) return null;
  return issue.line ? `${issue.file}:${issue.line}` : issue.file;
}

function IssueRow({
  issue,
  busy,
  onSetStatus,
}: {
  issue: IssueFound;
  busy: boolean;
  onSetStatus: (status: IssueFoundStatus) => void;
}) {
  const at = where(issue);
  return (
    <li
      className="flex flex-col gap-px rounded-sm border px-base py-half"
      data-testid="issue-found"
      data-status={issue.status}
      data-severity={issue.severity}
    >
      <div className="flex items-center gap-half text-xs">
        <span className={`font-medium ${SEVERITY_CLASS[issue.severity]}`}>
          {issue.severity}
        </span>
        <span className="text-low">{issue.category.replace('_', ' ')}</span>
        {at && (
          <span className="min-w-0 flex-1 truncate font-mono text-low">
            {at}
          </span>
        )}
        {!at && <span className="flex-1" />}
        <span
          className={issue.status === 'fixed' ? 'text-success' : 'text-low'}
          data-testid="issue-found-status"
        >
          {ISSUE_STATUS[issue.status]}
          {issue.status === 'fixed' && issue.fixed_round
            ? ` in round ${issue.fixed_round}`
            : ''}
        </span>
        {issue.status === 'open' && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="issue-found-wontfix"
            onClick={() => onSetStatus('wontfix')}
          >
            Won&apos;t fix
          </Button>
        )}
        {issue.status === 'wontfix' && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            onClick={() => onSetStatus('open')}
          >
            Reopen
          </Button>
        )}
      </div>
      <p className="text-sm text-normal">{issue.description}</p>
    </li>
  );
}

function CheckpointRow({
  checkpoint,
  issues,
  busy,
  onStop,
  onRetry,
  onSetIssueStatus,
  onOpenWorkspace,
}: {
  checkpoint: CheckpointView;
  issues: IssueFound[];
  busy: boolean;
  onStop: () => void;
  onRetry: () => void;
  onSetIssueStatus: (id: string, status: IssueFoundStatus) => void;
  onOpenWorkspace: (workspaceId: string) => void;
}) {
  const open = issues.filter((i) => i.status === 'open').length;
  const [expanded, setExpanded] = useState(false);
  const showIssues = expanded || open > 0;
  const status = CHECKPOINT_STATUS[checkpoint.status];
  const n = checkpoint.position + 1;
  const rejected = checkpoint.rejected_files ?? [];
  return (
    <li
      className="flex flex-col gap-half rounded-sm border px-base py-half"
      data-testid={`coding-checkpoint-${n}`}
      data-status={checkpoint.status}
    >
      <div className="flex items-center gap-half">
        <button
          type="button"
          aria-label={expanded ? 'Hide details' : 'Show details'}
          aria-expanded={showIssues}
          className="flex min-w-0 flex-1 items-center gap-half text-left"
          title={checkpoint.what}
          onClick={() => setExpanded((e) => !e)}
        >
          <CaretRightIcon
            weight="bold"
            className={`size-icon-xs shrink-0 text-low transition-transform ${showIssues ? 'rotate-90' : ''}`}
          />
          <span className="font-mono text-xs text-low">✓{n}</span>
          <span className="min-w-0 flex-1 truncate text-sm text-normal">
            {checkpoint.is_final ? 'Final check' : checkpoint.what}
          </span>
        </button>
        <span
          className={`text-xs ${status.className}`}
          data-testid="coding-checkpoint-status"
        >
          {status.label}
        </span>
        {(checkpoint.status === 'testing' ||
          checkpoint.status === 'fixing') && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="coding-checkpoint-stop"
            onClick={onStop}
          >
            Stop
          </Button>
        )}
        {(checkpoint.status === 'failed' ||
          checkpoint.status === 'stopped') && (
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="coding-checkpoint-retry"
            onClick={onRetry}
          >
            Retest
          </Button>
        )}
      </div>
      <div className="flex flex-wrap gap-x-base gap-y-half text-xs text-low">
        {checkpoint.after_titles.length > 0 && !checkpoint.is_final && (
          <span>after {checkpoint.after_titles.join(', ')}</span>
        )}
        {checkpoint.round > 0 && (
          <span data-testid="coding-checkpoint-round">
            {checkpoint.round} fix run{checkpoint.round === 1 ? '' : 's'}
          </span>
        )}
        {issues.length > 0 && (
          <span data-testid="coding-checkpoint-issues">
            {open} open / {issues.length} found
          </span>
        )}
        {checkpoint.cost_usd > 0 && <span>{usd(checkpoint.cost_usd)}</span>}
        {checkpoint.tester_workspace_id && (
          <button
            type="button"
            className="hover:text-normal"
            onClick={() => onOpenWorkspace(checkpoint.tester_workspace_id!)}
          >
            Tester
          </button>
        )}
        {checkpoint.fixer_workspace_id && (
          <button
            type="button"
            className="hover:text-normal"
            onClick={() => onOpenWorkspace(checkpoint.fixer_workspace_id!)}
          >
            Fixer
          </button>
        )}
      </div>
      {expanded && (
        <p className="text-xs text-low">
          {checkpoint.is_final && <>{checkpoint.what} </>}
          {checkpoint.tester_branch && (
            <span className="font-mono">{checkpoint.tester_branch}</span>
          )}
        </p>
      )}
      {rejected.length > 0 && (
        <p
          className="text-xs text-normal"
          data-testid="coding-checkpoint-rejected"
        >
          Dropped the tester&apos;s changes outside the test paths:{' '}
          <span className="font-mono">{rejected.join(', ')}</span>
        </p>
      )}
      {checkpoint.conflict &&
        (checkpoint.status === 'testing' || checkpoint.status === 'fixing') && (
          <p className="text-xs text-normal">
            Merge conflict in {checkpoint.conflict.files.join(', ')}; re-running
            on the updated ticket branch.
          </p>
        )}
      {checkpoint.error && (
        <p className="text-xs text-error">{checkpoint.error}</p>
      )}
      {showIssues && issues.length > 0 && (
        <ul className="flex flex-col gap-half">
          {issues.map((issue) => (
            <IssueRow
              key={issue.id}
              issue={issue}
              busy={busy}
              onSetStatus={(s) => onSetIssueStatus(issue.id, s)}
            />
          ))}
        </ul>
      )}
    </li>
  );
}

/**
 * The tester and the fix loop (Phase 6, D23, D24): each checkpoint is tested
 * once its subtasks are merged; open issues go to a fix run, then the
 * tester checks again, for at most three fix rounds.
 */
export function TicketTesting({
  checkpoints,
  issuesFound,
  busy,
  onStop,
  onRetry,
  onSetIssueStatus,
  onOpenWorkspace,
}: {
  checkpoints: CheckpointView[];
  issuesFound: IssueFound[];
  busy: boolean;
  onStop: (checkpointId: string) => void;
  onRetry: (checkpointId: string) => void;
  onSetIssueStatus: (id: string, status: IssueFoundStatus) => void;
  onOpenWorkspace: (workspaceId: string) => void;
}) {
  if (checkpoints.length === 0) return null;
  const passed = checkpoints.filter((c) => c.status === 'passed').length;
  const open = issuesFound.filter((i) => i.status === 'open').length;
  return (
    <div className="flex flex-col gap-half" data-testid="ticket-testing">
      <h4 className="text-xs font-medium text-low">
        Checkpoints · {passed}/{checkpoints.length} passed
        {issuesFound.length > 0 &&
          ` · ${issuesFound.length} issue${issuesFound.length === 1 ? '' : 's'} found, ${open} open`}
      </h4>
      <ul className="flex flex-col gap-half">
        {checkpoints.map((c) => (
          <CheckpointRow
            key={c.id}
            checkpoint={c}
            issues={issuesFound.filter((i) => i.checkpoint === c.position)}
            busy={busy}
            onStop={() => onStop(c.id)}
            onRetry={() => onRetry(c.id)}
            onSetIssueStatus={onSetIssueStatus}
            onOpenWorkspace={onOpenWorkspace}
          />
        ))}
      </ul>
    </div>
  );
}
