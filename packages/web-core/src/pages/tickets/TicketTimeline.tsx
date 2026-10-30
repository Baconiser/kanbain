import { useEffect, useState } from 'react';
import type { IssueDescription, IssueEvent } from 'shared/types';
import { issuesApi } from '@/shared/lib/ticketsApi';
import { TextDiffView } from './TextDiffView';

interface TicketTimelineProps {
  issueId: string;
  /** Creation time of the ticket; earlier events are prior work. */
  createdAt: string;
  events: IssueEvent[];
  isLoading: boolean;
}

type Payload = Record<string, unknown>;

function str(value: unknown): string {
  if (value === null || value === undefined) return '';
  return String(value);
}

function num(value: unknown): number {
  return typeof value === 'number' ? value : Number(value ?? 0) || 0;
}

function usd(value: unknown): string {
  const v = num(value);
  return `$${v.toFixed(v > 0 && v < 0.01 ? 4 : 2)}`;
}

function duration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  return m < 60
    ? `${m}m ${seconds % 60}s`
    : `${Math.floor(m / 60)}h ${m % 60}m`;
}

function actorLabel(actor: string): string {
  if (actor === 'user') return 'You';
  if (actor === 'system') return 'KanbAIn';
  if (actor.startsWith('agent:')) return 'Agent';
  return actor;
}

/** One line describing an event. */
export function describeEvent(event: IssueEvent): string {
  const p = event.payload as Payload;
  switch (event.event_type) {
    case 'created':
      return `Created in ${str(p.status) || 'the backlog'}`;
    case 'status_changed':
      return `Moved from ${str(p.from)} to ${str(p.to)}`;
    case 'title_changed':
      return `Renamed to “${str(p.to)}”`;
    case 'priority_changed':
      return p.to ? `Priority set to ${str(p.to)}` : 'Priority cleared';
    case 'external_id_changed':
      return p.to ? `External ID set to ${str(p.to)}` : 'External ID cleared';
    case 'mode_changed':
      return `Agent mode set to ${str(p.to)}`;
    case 'description_version':
      return `Description version ${num(p.version)} (${str(p.kind)})`;
    case 'comment':
      return `${actorLabel(event.actor)} commented: ${str(p.excerpt)}`;
    case 'attachment_added':
      return `Attached ${str(p.name)}`;
    case 'intake': {
      const action = p.action === 'updated' ? 'Updated' : 'Created';
      if (!p.enhanced) return `${action} from a dropped ticket`;
      return `${action} from a dropped ticket, structured by ${str(p.agent)} (${str(p.model)}, ${num(p.tokens_in) + num(p.tokens_out)} tokens, ${usd(p.cost_usd)})`;
    }
    case 'history_check':
      return `Found ${num(p.commit_count)} commit${num(p.commit_count) === 1 ? '' : 's'} for ${str(p.id)} in the git history`;
    case 'history_analysis':
      return p.error
        ? `Prior-work analysis failed: ${str(p.error)}`
        : `Prior work summarized by ${str(p.agent)} (${num(p.commit_count)} commits, ${usd(p.cost_usd)})`;
    case 'prior_commit':
      return `Commit ${str(p.short_sha)} in ${str(p.repo)}: ${str(p.subject)}${p.author ? ` (${str(p.author)})` : ''}`;
    case 'planning_started': {
      const why: Record<string, string> = {
        assigned: 'Assigned to the planner',
        answer: 'Planner resumed with your answers',
        feedback: 'Planner revising the plan with your feedback',
        repair: 'Planner asked to fix an invalid plan',
      };
      return why[str(p.reason)] ?? 'Planner started';
    }
    case 'plan_questions':
      return `The planner asked ${num(p.count) || 'some'} question${num(p.count) === 1 ? '' : 's'}`;
    case 'plan_version':
      return `Plan version ${num(p.version)} (${p.source === 'edited' ? 'edited by you' : 'by the planner'}, ${num(p.subtasks)} subtask${num(p.subtasks) === 1 ? '' : 's'})`;
    case 'plan_approved':
      return `Plan version ${num(p.version)} approved`;
    case 'plan_rejected':
      return `Plan version ${num(p.version)} sent back: ${str(p.feedback)}`;
    case 'planning_failed':
      return `Planning failed: ${str(p.error)}`;
    case 'coding_started':
      if (p.reason === 'retry') return 'Coding retried';
      if (p.reason === 'restart') return 'Coding resumed after a restart';
      return `Coding started from plan version ${num(p.version)} (${num(p.subtasks)} subtask${num(p.subtasks) === 1 ? '' : 's'}) on ${str(p.branch)}`;
    case 'subtask_started':
      return `Subtask ${str(p.subtask)} started: ${str(p.title)}${num(p.fix_round) ? ` (fix round ${num(p.fix_round)})` : ''}`;
    case 'subtask_merged':
      return `Subtask ${str(p.subtask)} merged into ${str(p.into)}: ${str(p.title)}`;
    case 'merge_conflict':
      return `Subtask ${str(p.subtask)} conflicted in ${Array.isArray(p.files) ? p.files.join(', ') : 'some files'}; re-running (fix round ${num(p.fix_round)})`;
    case 'subtask_failed':
      return `Subtask ${str(p.subtask)} failed: ${str(p.error)}`;
    case 'subtask_stopped':
      return `Subtask ${str(p.subtask)} stopped`;
    case 'coding_finished': {
      if (p.status === 'done') return 'All subtasks merged; ready for review';
      if (p.status === 'blocked') return `Coding blocked: ${str(p.error)}`;
      if (p.status === 'not_started')
        return `Coding could not start: ${str(p.error)}`;
      return 'Coding stopped';
    }
    case 'run_started':
      return `Run started${p.agent ? ` (${str(p.agent)})` : ''}`;
    case 'run_finished':
      return `Run ${str(p.status)}${p.agent ? ` (${str(p.agent)})` : ''}: ${duration(num(p.duration_seconds))}, ${num(p.tokens_in) + num(p.tokens_out)} tokens, ${usd(p.cost_usd)}`;
    default:
      return str(event.event_type);
  }
}

function VersionDiff({
  issueId,
  version,
}: {
  issueId: string;
  version: number;
}) {
  const [versions, setVersions] = useState<IssueDescription[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    issuesApi
      .listDescriptions(issueId)
      .then((v) => {
        if (!cancelled) setVersions(v);
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(err instanceof Error ? err.message : 'Failed to load');
      });
    return () => {
      cancelled = true;
    };
  }, [issueId, version]);

  if (error) return <p className="text-xs text-error">{error}</p>;
  if (!versions) return <p className="text-xs text-low">Loading…</p>;
  const current = versions.find((v) => v.version === version);
  const previous = versions.find((v) => v.version === version - 1);
  if (!current) return null;
  return (
    <div className="flex flex-col gap-half">
      {current.original_text && current.kind !== 'original' && (
        <details className="text-xs">
          <summary className="cursor-pointer text-low">Dropped text</summary>
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-sm border bg-secondary p-half">
            {current.original_text}
          </pre>
        </details>
      )}
      <TextDiffView
        oldText={previous?.description ?? ''}
        newText={current.description ?? ''}
      />
    </div>
  );
}

/** A ticket's history (D19), live. */
export function TicketTimeline({
  issueId,
  createdAt,
  events,
  isLoading,
}: TicketTimelineProps) {
  const [open, setOpen] = useState<string | null>(null);
  const created = Date.parse(createdAt);

  return (
    <section className="flex flex-col gap-half" data-testid="ticket-timeline">
      <h3 className="text-sm font-medium text-normal">
        Timeline ({events.length})
      </h3>
      {isLoading ? (
        <p className="text-sm text-low">Loading…</p>
      ) : (
        <ol className="flex flex-col">
          {events.map((event) => {
            const before = Date.parse(event.occurred_at) < created - 1000;
            const version =
              event.event_type === 'description_version'
                ? num((event.payload as Payload).version)
                : null;
            return (
              <li
                key={event.id}
                className="flex gap-base py-half text-sm"
                data-event-type={event.event_type}
              >
                <span
                  className={`mt-[0.4rem] size-2 shrink-0 rounded-full ${
                    before ? 'bg-low' : 'bg-brand'
                  }`}
                />
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-base">
                    <span className="text-normal">{describeEvent(event)}</span>
                    {version !== null && version > 1 && (
                      <button
                        type="button"
                        className="text-xs text-brand hover:underline"
                        onClick={() =>
                          setOpen(open === event.id ? null : event.id)
                        }
                      >
                        {open === event.id ? 'Hide changes' : 'Show changes'}
                      </button>
                    )}
                  </div>
                  <div className="text-xs text-low">
                    {new Date(event.occurred_at).toLocaleString()}
                    {before && ' · before KanbAIn'}
                  </div>
                  {open === event.id && version !== null && (
                    <div className="mt-half">
                      <VersionDiff issueId={issueId} version={version} />
                    </div>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}
