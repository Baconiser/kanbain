import { useCallback, useMemo, useState, type CSSProperties } from 'react';
import { CaretDownIcon, CaretRightIcon, PlusIcon } from '@phosphor-icons/react';
import {
  KanbanBoard,
  KanbanCard,
  KanbanCards,
  KanbanHeader,
  KanbanProvider,
  type DropResult,
} from '@kanbain/ui/components/KanbanBoard';
import { KanbanCardContent } from '@kanbain/ui/components/KanbanCardContent';
import type {
  BulkUpdateIssueItem,
  Issue,
  PlanningStatus,
  Project,
} from 'shared/types';
import type { ProjectBoard } from '@/shared/hooks/useTicketStreams';
import { projectsApi } from '@/shared/lib/ticketsApi';
import { ticketAccentColor, ticketDisplayId } from './ticketIds';

interface TicketBoardProps {
  project: Project;
  board: ProjectBoard;
  issues: Issue[];
  showHidden: boolean;
  selectedIssueId: string | null;
  onSelectIssue: (issueId: string) => void;
  onAddIssue: (statusId: string) => void;
  onError: (message: string) => void;
}

type Placement = { status_id: string; sort_order: number };

/** Moves `id` from its column into `toStatus` at `toIndex`; returns the new
 * placements of every ticket whose status or position changed. */
const PLANNING_MARKERS: Partial<
  Record<PlanningStatus, { label: string; className: string }>
> = {
  planning: { label: 'Planner working…', className: 'text-low' },
  waiting_for_answer: {
    label: 'Waiting for your answer',
    className: 'border-brand/40 bg-brand/10 text-normal',
  },
  awaiting_approval: { label: 'Plan to approve', className: 'text-normal' },
  failed: { label: 'Planning failed', className: 'text-error' },
};

/** What the planner needs from you, on the card (D20). */
function PlanningMarker({ status }: { status?: PlanningStatus }) {
  const marker = status ? PLANNING_MARKERS[status] : undefined;
  if (!marker) return null;
  return (
    <span
      className={`mt-half inline-block rounded-sm border px-half text-xs ${marker.className}`}
      data-testid={`planning-marker-${status}`}
    >
      {marker.label}
    </span>
  );
}

// Two ghost cards peeking out under a done main ticket whose sub-tickets are
// folded away. Pure box-shadow, so drag-and-drop never knows they exist.
const STACK_SHADOW = [
  '0 4px 0 -1px hsl(var(--bg-primary))',
  '0 4px 0 0 hsl(var(--border))',
  '0 8px 0 -1px hsl(var(--bg-primary))',
  '0 8px 0 0 hsl(var(--border))',
].join(', ');

export function computeMove(
  columns: Record<string, string[]>,
  id: string,
  fromStatus: string,
  toStatus: string,
  toIndex: number
): Record<string, Placement> {
  const source = [...(columns[fromStatus] ?? [])];
  const fromIndex = source.indexOf(id);
  if (fromIndex === -1) return {};
  source.splice(fromIndex, 1);
  const dest =
    fromStatus === toStatus ? source : [...(columns[toStatus] ?? [])];
  dest.splice(toIndex, 0, id);

  const changes: Record<string, Placement> = {};
  dest.forEach((issueId, index) => {
    changes[issueId] = { status_id: toStatus, sort_order: index + 1 };
  });
  if (fromStatus !== toStatus) {
    source.forEach((issueId, index) => {
      changes[issueId] = { status_id: fromStatus, sort_order: index + 1 };
    });
  }
  return changes;
}

export function TicketBoard({
  project,
  board,
  issues,
  showHidden,
  selectedIssueId,
  onSelectIssue,
  onAddIssue,
  onError,
}: TicketBoardProps) {
  // Placements applied locally until the server's patches arrive.
  const [optimistic, setOptimistic] = useState<Record<string, Placement>>({});
  // Done main tickets the user unfolded again.
  const [expandedStacks, setExpandedStacks] = useState<Set<string>>(
    () => new Set()
  );

  const doneStatusIds = useMemo(
    () =>
      new Set(board.statuses.filter((s) => s.kind === 'done').map((s) => s.id)),
    [board.statuses]
  );

  const visibleStatuses = useMemo(
    () => board.statuses.filter((s) => showHidden || !s.hidden),
    [board.statuses, showHidden]
  );

  const columns = useMemo(() => {
    const placed = issues.map((issue) => ({
      ...issue,
      ...(optimistic[issue.id] ?? {}),
    }));
    placed.sort((a, b) => a.sort_order - b.sort_order || a.number - b.number);
    // A done main ticket swallows its sub-tickets into a stack. Hidden cards
    // are left out of the columns entirely so drag indices stay honest.
    const statusById = new Map(placed.map((i) => [i.id, i.status_id]));
    const folded = (issue: Issue) => {
      const parentId = issue.parent_issue_id;
      if (!parentId || expandedStacks.has(parentId)) return false;
      const parentStatus = statusById.get(parentId);
      return !!parentStatus && doneStatusIds.has(parentStatus);
    };
    const result: Record<string, string[]> = {};
    for (const status of board.statuses) result[status.id] = [];
    for (const issue of placed) {
      if (folded(issue)) continue;
      (result[issue.status_id] ??= []).push(issue.id);
    }
    return result;
  }, [issues, optimistic, board.statuses, doneStatusIds, expandedStacks]);

  const toggleStack = useCallback((issueId: string) => {
    setExpandedStacks((prev) => {
      const next = new Set(prev);
      if (!next.delete(issueId)) next.add(issueId);
      return next;
    });
  }, []);

  const handleDragEnd = useCallback(
    (result: DropResult) => {
      const { source, destination, draggableId } = result;
      if (!destination) return;
      if (
        source.droppableId === destination.droppableId &&
        source.index === destination.index
      ) {
        return;
      }

      const changes = computeMove(
        columns,
        draggableId,
        source.droppableId,
        destination.droppableId,
        destination.index
      );
      const updates: BulkUpdateIssueItem[] = Object.entries(changes)
        .filter(([id, placement]) => {
          const issue = board.issuesById[id];
          return (
            issue &&
            (issue.status_id !== placement.status_id ||
              issue.sort_order !== placement.sort_order)
          );
        })
        .map(([id, placement]) => ({ id, ...placement }));
      if (updates.length === 0) return;

      setOptimistic((prev) => ({ ...prev, ...changes }));
      projectsApi
        .bulkUpdateIssues(project.id, updates)
        .catch((err: unknown) =>
          onError(err instanceof Error ? err.message : 'Failed to move ticket')
        )
        .finally(() =>
          setOptimistic((prev) => {
            const next = { ...prev };
            for (const id of Object.keys(changes)) delete next[id];
            return next;
          })
        );
    },
    [columns, board.issuesById, project.id, onError]
  );

  // Sub-ticket progress per parent; Done counts as finished (for orchestrated
  // subtasks: merged into the ticket branch).
  const subIssuesByParent = useMemo(() => {
    const progress: Record<string, { done: number; total: number }> = {};
    for (const issue of board.issues) {
      if (!issue.parent_issue_id) continue;
      const entry = (progress[issue.parent_issue_id] ??= { done: 0, total: 0 });
      entry.total += 1;
      if (doneStatusIds.has(issue.status_id)) entry.done += 1;
    }
    return progress;
  }, [board.issues, doneStatusIds]);

  if (visibleStatuses.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-low">
        This project has no visible statuses.
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto px-double pb-double">
      <KanbanProvider onDragEnd={handleDragEnd}>
        {visibleStatuses.map((status) => {
          const issueIds = columns[status.id] ?? [];
          return (
            <KanbanBoard key={status.id}>
              <KanbanHeader>
                <div className="sticky top-0 z-20 flex shrink-0 items-center justify-between gap-2 border-y bg-secondary p-base">
                  <div className="flex min-w-0 items-center gap-2">
                    <div
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{ backgroundColor: `hsl(${status.color})` }}
                    />
                    <p className="m-0 truncate text-sm">{status.name}</p>
                    <span className="text-xs text-low">{issueIds.length}</span>
                  </div>
                  <button
                    type="button"
                    onClick={() => onAddIssue(status.id)}
                    className="rounded-sm p-half text-low transition-colors hover:bg-primary hover:text-normal"
                    aria-label={`Add ticket to ${status.name}`}
                  >
                    <PlusIcon className="size-icon-xs" weight="bold" />
                  </button>
                </div>
              </KanbanHeader>
              <KanbanCards id={status.id}>
                {issueIds.map((issueId, index) => {
                  const issue = board.issuesById[issueId];
                  if (!issue) return null;
                  const tags = (board.tagIdsByIssue[issue.id] ?? [])
                    .map((tagId) => board.tagsById[tagId])
                    .filter((tag) => !!tag);
                  const parent = issue.parent_issue_id
                    ? board.issuesById[issue.parent_issue_id]
                    : undefined;
                  const subIssues = subIssuesByParent[issue.id];
                  const hasSubIssues = !!subIssues && subIssues.total > 0;
                  const stackable =
                    hasSubIssues && doneStatusIds.has(status.id);
                  const stacked = stackable && !expandedStacks.has(issue.id);
                  // Main tickets wear their colour on top, sub-tickets
                  // inherit it on the left so families are obvious.
                  const style: CSSProperties = {};
                  if (parent) {
                    style.borderLeftWidth = 3;
                    style.borderLeftColor = ticketAccentColor(parent);
                  } else if (hasSubIssues) {
                    style.borderTopWidth = 3;
                    style.borderTopColor = ticketAccentColor(issue);
                  }
                  if (stacked) {
                    style.boxShadow = STACK_SHADOW;
                    style.marginBottom = 8;
                  }
                  return (
                    <KanbanCard
                      key={issue.id}
                      id={issue.id}
                      name={issue.title}
                      index={index}
                      className="group"
                      style={style}
                      onClick={() => onSelectIssue(issue.id)}
                      isOpen={selectedIssueId === issue.id}
                    >
                      <KanbanCardContent
                        displayId={ticketDisplayId(issue, project)}
                        title={issue.title}
                        description={issue.description}
                        priority={issue.priority}
                        tags={tags}
                        assignees={[]}
                        isSubIssue={!!issue.parent_issue_id}
                        parent={
                          parent && {
                            displayId: ticketDisplayId(parent, project),
                            title: parent.title,
                          }
                        }
                        subIssues={subIssues}
                      />
                      <PlanningMarker
                        status={board.planningByIssue[issue.id]?.status}
                      />
                      {stackable && (
                        <button
                          type="button"
                          className="mt-half flex items-center gap-half self-start text-xs text-low hover:text-normal"
                          // The card opens on mouseup, so the toggle has to
                          // eat that event too or every click opens the panel.
                          onMouseUp={(e) => e.stopPropagation()}
                          onClick={(e) => {
                            e.stopPropagation();
                            toggleStack(issue.id);
                          }}
                          data-testid="subticket-stack-toggle"
                        >
                          {stacked ? (
                            <CaretRightIcon className="size-icon-xs" />
                          ) : (
                            <CaretDownIcon className="size-icon-xs" />
                          )}
                          {stacked
                            ? `${subIssues?.total} sub-tickets`
                            : 'Stack sub-tickets'}
                        </button>
                      )}
                    </KanbanCard>
                  );
                })}
              </KanbanCards>
            </KanbanBoard>
          );
        })}
      </KanbanProvider>
    </div>
  );
}
