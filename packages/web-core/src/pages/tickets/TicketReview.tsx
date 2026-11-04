import { useCallback, useEffect, useState } from 'react';
import { Button } from '@kanbain/ui/components/Button';
import type {
  ChangelogEntry,
  ChangelogKind,
  IssueFound,
  ReviewRepo,
  ReviewView,
} from 'shared/types';
import { reviewApi } from '@/shared/lib/ticketsApi';
import { SimpleMarkdown } from '@/shared/components/SimpleMarkdown';
import { ConflictResolutionProgress } from '@/shared/components/ConflictResolutionProgress';
import { ResolveConflictsDialog } from '@/shared/dialogs/tasks/ResolveConflictsDialog';
import { CollapsibleSection } from './CollapsibleSection';

interface TicketReviewProps {
  issueId: string;
  /** Changes when review-related events arrive, to reload. */
  refreshKey: number;
  onOpenWorkspace: (workspaceId: string) => void;
  onError: (message: string) => void;
}

// While something moves on its own (coding, an agent rebase), poll.
const POLL_MS = 4000;

const usd = (n: number) => `$${n.toFixed(n > 0 && n < 0.01 ? 4 : 2)}`;

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

const KIND_LABEL: Record<ChangelogKind, string> = {
  coder: 'Coder',
  tester: 'Tester',
  fix: 'Fix',
  merge: 'Merge',
};

const KIND_CLASS: Record<ChangelogKind, string> = {
  coder: 'bg-brand/15 text-brand',
  tester: 'bg-success/15 text-success',
  fix: 'bg-amber-100/60 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300',
  merge: 'bg-merged/15 text-merged',
};

const textareaClass =
  'w-full rounded-sm border border-border bg-secondary px-base py-half text-sm';

function needsPoll(view: ReviewView | null): boolean {
  if (!view) return false;
  return (
    view.coding_status === 'running' ||
    view.repos.some(
      (r) =>
        r.conflict_resolution &&
        ['queued', 'rebasing', 'syncing'].includes(r.conflict_resolution.stage)
    )
  );
}

function RepoDiff({
  repo,
  workspaceId,
  branch,
  onOpenWorkspace,
}: {
  repo: ReviewRepo;
  workspaceId: string | null;
  branch: string | null;
  onOpenWorkspace: (workspaceId: string) => void;
}) {
  const [showFiles, setShowFiles] = useState(false);
  const behind = repo.commits_behind > 0 && repo.commits_ahead > 0;
  return (
    <div className="flex flex-col gap-half rounded-sm border border-border p-base">
      <div className="flex flex-wrap items-center gap-base text-sm">
        <span className="font-medium text-high">{repo.repo_name}</span>
        <span className="text-low">→ {repo.target_branch}</span>
        <span className="text-success">+{repo.lines_added}</span>
        <span className="text-error">−{repo.lines_removed}</span>
        <span className="text-low">
          {repo.files_changed} file{repo.files_changed === 1 ? '' : 's'},{' '}
          {repo.commits_ahead} commit{repo.commits_ahead === 1 ? '' : 's'}
        </span>
        {behind && (
          <span className="text-amber-700 dark:text-amber-300">
            {repo.commits_behind} behind {repo.target_branch}
          </span>
        )}
        {repo.merged_commit && repo.commits_ahead === 0 && (
          <span className="text-merged">
            Merged {repo.merged_commit.slice(0, 8)}
          </span>
        )}
        <span className="flex-1" />
        {behind && workspaceId && !repo.has_conflicts && (
          <Button
            size="xs"
            variant="outline"
            data-testid="review-rebase"
            onClick={() =>
              ResolveConflictsDialog.show({
                workspaceId,
                repoId: repo.repo_id,
                targetBranch: repo.target_branch,
                conflictedFiles:
                  repo.conflict_resolution?.predicted_files ?? [],
              })
            }
          >
            Rebase with agent
          </Button>
        )}
        {repo.files.length > 0 && (
          <Button
            size="xs"
            variant="outline"
            onClick={() => setShowFiles((v) => !v)}
          >
            {showFiles ? 'Hide files' : 'Files'}
          </Button>
        )}
        {workspaceId && (
          <Button
            size="xs"
            variant="outline"
            onClick={() => onOpenWorkspace(workspaceId)}
          >
            Full diff
          </Button>
        )}
      </div>
      {repo.conflict_resolution && (
        <ConflictResolutionProgress
          resolution={repo.conflict_resolution}
          branch={branch}
        />
      )}
      {showFiles && (
        <ul className="flex flex-col font-mono text-xs">
          {repo.files.map((f) => (
            <li key={f.path} className="flex gap-base">
              <span className="w-10 text-right text-success">
                {f.added === null ? 'bin' : `+${f.added}`}
              </span>
              <span className="w-10 text-right text-error">
                {f.removed === null ? '' : `−${f.removed}`}
              </span>
              <span className="truncate text-normal">{f.path}</span>
            </li>
          ))}
          {repo.files_changed > repo.files.length && (
            <li className="text-low">
              …and {repo.files_changed - repo.files.length} more
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

function EntryRow({ entry }: { entry: ChangelogEntry }) {
  const [open, setOpen] = useState(false);
  return (
    <li className="flex flex-col gap-half border-b border-border py-half last:border-b-0">
      <button
        type="button"
        className="flex items-center gap-base text-left text-sm"
        onClick={() => setOpen((v) => !v)}
      >
        <span
          className={`rounded-sm px-half text-xs font-medium ${KIND_CLASS[entry.kind]}`}
        >
          {KIND_LABEL[entry.kind]}
        </span>
        <span className="flex-1 truncate text-normal">{entry.title}</span>
        <span className="text-xs text-low">
          {new Date(entry.created_at).toLocaleString()}
        </span>
      </button>
      {open && (
        <div className="pl-base text-sm">
          <SimpleMarkdown content={entry.summary} />
          {entry.scribe_error && (
            <p className="text-xs text-low">
              The scribe failed ({entry.scribe_error}); this is the plain
              summary from the commits.
            </p>
          )}
          {entry.commit_refs.length > 0 && (
            <p className="font-mono text-xs text-low">
              {entry.commit_refs
                .map((c) => `${c.repo}@${c.sha.slice(0, 8)}`)
                .join(', ')}
            </p>
          )}
        </div>
      )}
    </li>
  );
}

function IssueList({ issues }: { issues: IssueFound[] }) {
  const fixed = issues.filter((i) => i.status === 'fixed').length;
  const open = issues.filter((i) => i.status === 'open').length;
  const wontfix = issues.length - fixed - open;
  return (
    <CollapsibleSection
      id="review-issues"
      title="Issues found"
      meta={`${fixed} fixed · ${open} open${wontfix ? ` · ${wontfix} won't fix` : ''}`}
      defaultOpen={open > 0}
    >
      {issues.length === 0 ? (
        <p className="text-sm text-low">The tester found nothing.</p>
      ) : (
        <ul className="flex flex-col gap-half text-sm">
          {issues.map((i) => (
            <li key={i.id} className="flex gap-base">
              <span
                className={
                  i.status === 'fixed'
                    ? 'text-success'
                    : i.status === 'open'
                      ? 'text-error'
                      : 'text-low'
                }
              >
                {i.status === 'wontfix' ? "won't fix" : i.status}
              </span>
              <span className="text-low">[{i.severity}]</span>
              <span className="flex-1 text-normal">
                {i.description}
                {i.file && (
                  <span className="font-mono text-xs text-low">
                    {' '}
                    {i.file}
                    {i.line !== null ? `:${i.line}` : ''}
                  </span>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
    </CollapsibleSection>
  );
}

/** The review screen (D26): what the ticket did, and Merge. */
export function TicketReview({
  issueId,
  refreshKey,
  onOpenWorkspace,
  onError,
}: TicketReviewProps) {
  const [view, setView] = useState<ReviewView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [push, setPush] = useState(false);
  const [feedback, setFeedback] = useState('');
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await reviewApi.get(issueId));
    } catch (err) {
      onError(errorMessage(err, 'Failed to load the review'));
    } finally {
      setLoading(false);
    }
  }, [issueId, onError]);

  useEffect(() => {
    load();
  }, [load, refreshKey]);

  const polling = needsPoll(view);
  useEffect(() => {
    if (!polling) return;
    const timer = window.setInterval(load, POLL_MS);
    return () => window.clearInterval(timer);
  }, [polling, load]);

  const run = async (action: () => Promise<string | null>, failed: string) => {
    setBusy(true);
    setNotice(null);
    try {
      setNotice(await action());
      await load();
    } catch (err) {
      onError(errorMessage(err, failed));
    } finally {
      setBusy(false);
    }
  };

  const merge = () =>
    run(async () => {
      const result = await reviewApi.merge(issueId, push);
      const parts = [
        `Merged ${result.merged.length} repo${result.merged.length === 1 ? '' : 's'}`,
      ];
      if (result.pushed.length)
        parts.push(`pushed ${result.pushed.join(', ')}`);
      if (result.push_errors.length)
        parts.push(`push failed: ${result.push_errors.join('; ')}`);
      return parts.join(', ') + '.';
    }, 'Merge failed');

  const requestChanges = (to: 'coders' | 'planner') =>
    run(async () => {
      await reviewApi.requestChanges(issueId, { to, feedback });
      setFeedback('');
      return to === 'coders'
        ? 'Sent to the coders: a fix run works on your feedback, then the tester checks it.'
        : 'Sent to the planner: approve the revised plan to code it.';
    }, 'Sending the feedback failed');

  const exportChangelog = () =>
    run(async () => {
      await reviewApi.exportTicket(issueId);
      return null;
    }, 'Export failed');

  if (loading && !view) {
    return <p className="text-sm text-low">Loading the review…</p>;
  }
  if (!view) return null;

  const canMerge = !view.merged && view.blockers.length === 0;
  const mergedCommits = view.repos
    .filter((r) => r.merged_commit)
    .map((r) => `${r.repo_name}@${(r.merged_commit ?? '').slice(0, 8)}`);
  const cost = view.cost;

  return (
    <div className="flex flex-col gap-double" data-testid="ticket-review">
      <section className="flex flex-col gap-base">
        <div className="flex flex-wrap items-center gap-base">
          <Button
            size="sm"
            disabled={busy || !canMerge}
            data-testid="review-merge"
            onClick={merge}
          >
            {busy ? 'Working…' : 'Merge'}
          </Button>
          <label className="flex items-center gap-half text-sm text-normal">
            <input
              type="checkbox"
              checked={push}
              onChange={(e) => setPush(e.target.checked)}
              data-testid="review-push"
            />
            Push the target afterwards
          </label>
          <span className="flex-1" />
          <Button
            size="xs"
            variant="outline"
            disabled={busy}
            data-testid="review-export"
            onClick={exportChangelog}
          >
            Export changelog
          </Button>
        </div>
        {view.merged && (
          <p className="text-sm text-merged" data-testid="review-merged">
            Merged ({mergedCommits.join(', ')}). Nothing new on the branch.
          </p>
        )}
        {view.blockers.length > 0 && (
          <ul
            className="flex flex-col gap-half text-sm text-error"
            data-testid="review-blockers"
          >
            {view.blockers.map((b) => (
              <li key={b}>{b}</li>
            ))}
          </ul>
        )}
        {view.warnings.length > 0 && (
          <ul className="flex flex-col gap-half text-sm text-amber-700 dark:text-amber-300">
            {view.warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        )}
        {notice && (
          <p className="text-sm text-success" data-testid="review-notice">
            {notice}
          </p>
        )}
      </section>

      <section className="grid grid-cols-2 gap-base text-sm sm:grid-cols-4">
        <div>
          <div className="text-xs text-low">Cost</div>
          <div className="text-high" data-testid="review-cost">
            {usd(cost.total_usd)}
            {cost.budget_usd !== null && (
              <span className="text-low"> of {usd(cost.budget_usd)}</span>
            )}
          </div>
        </div>
        <div>
          <div className="text-xs text-low">Agent runs</div>
          <div className="text-normal">{usd(cost.runs_usd)}</div>
        </div>
        <div>
          <div className="text-xs text-low">Jobs (scribe, history…)</div>
          <div className="text-normal">{usd(cost.jobs_usd)}</div>
        </div>
        <div>
          <div className="text-xs text-low">Branch</div>
          <div className="truncate font-mono text-xs text-normal">
            {view.branch ?? '—'}
          </div>
        </div>
      </section>

      {view.acceptance_criteria.length > 0 && (
        <CollapsibleSection
          id="review-criteria"
          title="Acceptance criteria"
          meta={`plan v${view.plan_version ?? '?'}`}
        >
          <ul className="list-disc pl-double text-sm text-normal">
            {view.acceptance_criteria.map((c) => (
              <li key={c}>{c}</li>
            ))}
          </ul>
        </CollapsibleSection>
      )}

      <CollapsibleSection
        id="review-diff"
        title="Changes"
        meta={`${view.repos.reduce((n, r) => n + r.files_changed, 0)} files`}
      >
        <div className="flex flex-col gap-base">
          {view.repos.length === 0 && (
            <p className="text-sm text-low">No repositories.</p>
          )}
          {view.repos.map((repo) => (
            <RepoDiff
              key={repo.repo_id}
              repo={repo}
              workspaceId={view.workspace_id}
              branch={view.branch}
              onOpenWorkspace={onOpenWorkspace}
            />
          ))}
        </div>
      </CollapsibleSection>

      <IssueList issues={view.issues_found} />

      <CollapsibleSection
        id="review-changelog"
        testId="review-changelog-section"
        title="Changelog"
        meta={`${view.changelog.length} entr${view.changelog.length === 1 ? 'y' : 'ies'}`}
      >
        {view.changelog.length === 0 ? (
          <p className="text-sm text-low">
            No entries yet. The scribe writes one after each coder, tester and
            fix run.
          </p>
        ) : (
          <ul className="flex flex-col" data-testid="review-changelog">
            {[...view.changelog].reverse().map((e) => (
              <EntryRow key={e.id} entry={e} />
            ))}
          </ul>
        )}
      </CollapsibleSection>

      <CollapsibleSection
        id="review-request"
        testId="review-request"
        title="Request changes"
      >
        <div className="flex flex-col gap-base">
          <textarea
            className={`min-h-20 ${textareaClass}`}
            placeholder="What should change?"
            value={feedback}
            data-testid="review-feedback"
            onChange={(e) => setFeedback(e.target.value)}
          />
          <div className="flex flex-wrap gap-base">
            <Button
              size="xs"
              variant="outline"
              disabled={busy || !feedback.trim()}
              data-testid="review-to-coders"
              onClick={() => requestChanges('coders')}
            >
              Send to coders
            </Button>
            <Button
              size="xs"
              variant="outline"
              disabled={busy || !feedback.trim()}
              data-testid="review-to-planner"
              onClick={() => requestChanges('planner')}
            >
              Send to planner
            </Button>
          </div>
          <p className="text-xs text-low">
            Coders: your feedback becomes an open issue at the final checkpoint;
            a fix run works on it and the tester checks it. Planner: the planner
            revises the plan, which you approve again.
          </p>
        </div>
      </CollapsibleSection>
    </div>
  );
}
