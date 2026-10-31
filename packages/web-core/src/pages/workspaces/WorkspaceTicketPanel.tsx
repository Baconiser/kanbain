import { useCallback, useEffect, useState } from 'react';
import {
  ArrowLeftIcon,
  ArrowUpIcon,
  KanbanIcon,
  XIcon,
} from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useProjectBoard, useProjects } from '@/shared/hooks/useTicketStreams';
import { codingApi, issuesApi } from '@/shared/lib/ticketsApi';
import { TicketPanel } from '../tickets/TicketPanel';
import { requestOpenTicket } from '../tickets/openTicket';

interface WorkspaceTicketPanelProps {
  /** The ticket this workspace is linked to. Remount (`key`) when it changes. */
  issueId: string;
  workspaceId: string;
}

/**
 * The board's ticket panel, in the workspace sidebar. Same component, same
 * live streams — the board and the workspace edit one ticket, not two
 * copies that drift apart.
 */
export function WorkspaceTicketPanel({
  issueId,
  workspaceId,
}: WorkspaceTicketPanelProps) {
  const appNavigation = useAppNavigation();
  const [projectId, setProjectId] = useState<string | null>(null);
  const [lookupFailed, setLookupFailed] = useState(false);
  // The ticket this workspace actually works on: a coder workspace is linked
  // to the parent (budget, commit prefix) but builds one sub-ticket, and
  // that's the one you want staring back at you. Null until resolved.
  const [homeIssueId, setHomeIssueId] = useState<string | null>(null);
  // Clicking a sub-ticket or relationship walks here, not off to the board.
  const [shownIssueId, setShownIssueId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The workspace only knows the ticket id; the board stream needs the
  // project. One fetch, then the stream keeps everything live.
  useEffect(() => {
    let cancelled = false;
    const settle = (home: string) => {
      if (cancelled) return;
      setHomeIssueId(home);
      setShownIssueId(home);
    };
    issuesApi
      .get(issueId)
      .then((issue) => {
        if (cancelled) return;
        setProjectId(issue.project_id);
        return codingApi
          .get(issueId)
          .then((view) => {
            const subtask = view.subtasks.find(
              (s) => s.workspace_id === workspaceId
            );
            settle(subtask?.sub_issue_id ?? issueId);
          })
          .catch(() => settle(issueId));
      })
      .catch(() => {
        if (!cancelled) setLookupFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [issueId, workspaceId]);

  const { projects } = useProjects();
  const project = projects.find((p) => p.id === projectId);
  const board = useProjectBoard(projectId ?? undefined);
  const issue = shownIssueId ? board.issuesById[shownIssueId] : undefined;
  const parent = issue?.parent_issue_id
    ? board.issuesById[issue.parent_issue_id]
    : undefined;

  const showError = useCallback((message: string) => setError(message), []);

  const openOnBoard = () => {
    if (!project || !issue) return;
    requestOpenTicket({ projectId: project.id, issueId: issue.id });
    appNavigation.goToTickets();
  };

  if (lookupFailed || (homeIssueId && project && !board.isLoading && !issue)) {
    return (
      <p className="p-double text-sm text-low">
        This workspace's ticket is gone.
      </p>
    );
  }

  if (!project || !issue) {
    return <p className="p-double text-sm text-low">Loading ticket…</p>;
  }

  return (
    <div className="flex h-full flex-col">
      {error && (
        <div
          role="alert"
          className="m-base flex items-center justify-between gap-base rounded-sm border border-error bg-error/10 px-base py-half text-sm"
        >
          <span className="min-w-0 break-words">{error}</span>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => setError(null)}
          >
            <XIcon className="size-icon-xs" />
          </button>
        </div>
      )}
      <div className="min-h-0 flex-1">
        <TicketPanel
          key={issue.id}
          project={project}
          issue={issue}
          board={board}
          embedded
          currentWorkspaceId={workspaceId}
          onSelectIssue={setShownIssueId}
          onError={showError}
          headerActions={
            <>
              {/* Wandered off to a sub-ticket: this is the way home. */}
              {shownIssueId !== homeIssueId && (
                <Button
                  size="xs"
                  variant="ghost"
                  aria-label="Back to this workspace's ticket"
                  title="Back to this workspace's ticket"
                  onClick={() => setShownIssueId(homeIssueId)}
                >
                  <ArrowLeftIcon className="size-icon-sm" />
                </Button>
              )}
              {parent && (
                <Button
                  size="xs"
                  variant="ghost"
                  aria-label="Parent ticket"
                  title={`Parent ticket: ${parent.title}`}
                  onClick={() => setShownIssueId(parent.id)}
                >
                  <ArrowUpIcon className="size-icon-sm" />
                </Button>
              )}
              <Button
                size="xs"
                variant="ghost"
                aria-label="Open on board"
                title="Open on board"
                onClick={openOnBoard}
              >
                <KanbanIcon className="size-icon-sm" />
              </Button>
            </>
          }
        />
      </div>
    </div>
  );
}
