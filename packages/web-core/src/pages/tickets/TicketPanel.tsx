import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  LinkIcon,
  PlayIcon,
  PlusIcon,
  TrashIcon,
  XIcon,
} from '@phosphor-icons/react';
import { AutoResizeTextarea } from '@kanbain/ui/components/AutoResizeTextarea';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@kanbain/ui/components/Select';
import type {
  Issue,
  IssueRelationshipType,
  Project,
  UpdateIssue,
  WorkspaceWithStatus,
} from 'shared/types';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { resolveDefaultTargetBranch } from '@/shared/hooks/useProjectRepoDefaults';
import type { ProjectBoard } from '@/shared/hooks/useTicketStreams';
import { attachmentsApi } from '@/shared/lib/api';
import { codingApi, issuesApi, projectsApi } from '@/shared/lib/ticketsApi';
import { buildWorkspaceCreateInitialState } from '@/shared/lib/workspaceCreateState';
import { setCreateModeSeedState } from '@/features/create-mode/model/createModeSeedStore';
import { useIssueTimeline } from '@/shared/hooks/useTicketStreams';
import { CollapsibleSection } from './CollapsibleSection';
import { TicketAttachments } from './TicketAttachments';
import { TicketCoding } from './TicketCoding';
import { TicketComments } from './TicketComments';
import { TicketDescription } from './TicketDescription';
import { TicketHistory } from './TicketHistory';
import { TicketPlan } from './TicketPlan';
import { TicketProperties } from './TicketProperties';
import { TicketRelated } from './TicketRelated';
import { TicketTimeline } from './TicketTimeline';
import { internalTicketId, ticketDisplayId } from './ticketIds';

const RELATIONSHIP_LABELS: Record<IssueRelationshipType, string> = {
  blocking: 'Blocks',
  related: 'Related to',
  has_duplicate: 'Duplicated by',
};
// Long sub-ticket lists show this many until expanded.
const SUB_TICKETS_SHOWN = 5;

type TabId = 'overview' | 'plan' | 'coding' | 'activity';
const TAB_STORAGE_KEY = 'kanbain.ticketPanel.tab';

function readStoredTab(): TabId {
  try {
    const stored = window.localStorage.getItem(TAB_STORAGE_KEY);
    if (
      stored === 'overview' ||
      stored === 'plan' ||
      stored === 'coding' ||
      stored === 'activity'
    ) {
      return stored;
    }
  } catch {
    // Storage can be unavailable (private mode); start on the overview.
  }
  return 'overview';
}

interface TicketPanelProps {
  project: Project;
  issue: Issue;
  board: ProjectBoard;
  /** Without it there is no close button (the panel lives in a sidebar). */
  onClose?: () => void;
  onSelectIssue: (issueId: string) => void;
  onError: (message: string) => void;
  /** Extra header buttons, left of delete. */
  headerActions?: ReactNode;
  /** The workspace showing this panel; marked in the workspace list. */
  currentWorkspaceId?: string;
  /** Inside another panel: no left border of its own. */
  embedded?: boolean;
}

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

/** Markdown for an uploaded attachment, rendered by the editor's nodes. */
function attachmentMarkdown(file: {
  id: string;
  original_name: string;
  mime_type: string | null;
}): string {
  const isImage = file.mime_type?.startsWith('image/');
  return `${isImage ? '!' : ''}[${file.original_name}](attachment://${file.id})`;
}

function TabButton({
  active,
  onClick,
  label,
  badge,
  attention,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  badge?: string;
  attention?: boolean;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      data-testid={`ticket-tab-${label.toLowerCase()}`}
      onClick={onClick}
      className={`relative flex items-center gap-half border-b-2 px-base py-base text-sm ${
        active
          ? 'border-brand text-high'
          : 'border-transparent text-low hover:text-normal'
      }`}
    >
      {label}
      {badge && <span className="text-xs text-low">{badge}</span>}
      {attention && (
        <span
          className="size-1.5 rounded-full bg-brand"
          aria-label="Needs your attention"
        />
      )}
    </button>
  );
}

// `hidden` alone loses to Tailwind's `flex`, so the class switches too.
const tabPanelClass = (active: boolean) =>
  active
    ? 'flex flex-col gap-double px-double py-double [&>*]:shrink-0'
    : 'hidden';

type WorkspacesState = { workspaces: Record<string, WorkspaceWithStatus> };
const initialWorkspaces = (): WorkspacesState => ({ workspaces: {} });

export function TicketPanel({
  project,
  issue,
  board,
  onClose,
  onSelectIssue,
  onError,
  headerActions,
  currentWorkspaceId,
  embedded = false,
}: TicketPanelProps) {
  const appNavigation = useAppNavigation();
  const [title, setTitle] = useState(issue.title);
  const [description, setDescription] = useState(issue.description ?? '');
  const [newSubTitle, setNewSubTitle] = useState('');
  const [relationType, setRelationType] =
    useState<IssueRelationshipType>('related');
  const [linking, setLinking] = useState(false);
  const [showAllChildren, setShowAllChildren] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [tab, setTabState] = useState<TabId>(readStoredTab);
  const savedDescription = useRef(issue.description ?? '');

  const setTab = (next: TabId) => {
    setTabState(next);
    try {
      window.localStorage.setItem(TAB_STORAGE_KEY, next);
    } catch {
      // Not remembering the tab is fine.
    }
  };

  // The page remounts this panel per ticket (`key`); here we only follow
  // remote changes to fields that are not being edited.
  useEffect(() => {
    setTitle(issue.title);
  }, [issue.title]);

  // A description changed elsewhere (an intake update) replaces the local
  // one unless it has unsaved edits.
  useEffect(() => {
    const remote = issue.description ?? '';
    setDescription((local) => {
      // The backend trims; that alone is not a change.
      if (local !== savedDescription.current) return local;
      if (local.trim() === remote.trim()) return local;
      savedDescription.current = remote;
      return remote;
    });
  }, [issue.description]);

  const timeline = useIssueTimeline(issue.id);
  const attachmentEvents = timeline.events.filter(
    (e) => e.event_type === 'attachment_added'
  ).length;
  const historyEvents = timeline.events.filter(
    (e) =>
      e.event_type === 'history_check' ||
      e.event_type === 'history_analysis' ||
      e.event_type === 'prior_commit'
  ).length;

  const planningTimeline = timeline.events.filter(
    (e) =>
      e.event_type === 'planning_started' ||
      e.event_type === 'plan_questions' ||
      e.event_type === 'plan_version' ||
      e.event_type === 'plan_approved' ||
      e.event_type === 'plan_rejected' ||
      e.event_type === 'planning_failed'
  );
  const planningEvents = planningTimeline.length;
  // The planner's last word was a question: the answer goes in the comments.
  const planWaiting =
    planningTimeline[planningTimeline.length - 1]?.event_type ===
    'plan_questions';
  const planToApprove =
    planningTimeline[planningTimeline.length - 1]?.event_type ===
    'plan_version';

  const codingEvents = timeline.events.filter(
    (e) =>
      e.event_type === 'plan_approved' ||
      e.event_type === 'coding_started' ||
      e.event_type === 'subtask_started' ||
      e.event_type === 'subtask_merged' ||
      e.event_type === 'merge_conflict' ||
      e.event_type === 'subtask_failed' ||
      e.event_type === 'subtask_stopped' ||
      e.event_type === 'coding_finished'
  ).length;
  const planApproved = timeline.events.some(
    (e) => e.event_type === 'plan_approved'
  );
  // Nothing to code before a plan is approved.
  const hasCoding = codingEvents > 0;
  const activeTab: TabId = tab === 'coding' && !hasCoding ? 'overview' : tab;

  const update = useCallback(
    (data: UpdateIssue) =>
      issuesApi
        .update(issue.id, data)
        .catch((err: unknown) =>
          onError(errorMessage(err, 'Failed to update ticket'))
        ),
    [issue.id, onError]
  );

  // Autosave the description shortly after typing stops.
  useEffect(() => {
    if (description === savedDescription.current) return;
    const timer = window.setTimeout(() => {
      savedDescription.current = description;
      void update({ description: description.trim() ? description : null });
    }, 800);
    return () => window.clearTimeout(timer);
  }, [description, update]);

  const handlePasteFiles = useCallback(
    async (files: File[]) => {
      const uploaded = [];
      for (const file of files) {
        try {
          uploaded.push(await attachmentsApi.upload(file));
        } catch (err) {
          onError(errorMessage(err, 'Failed to upload attachment'));
        }
      }
      if (uploaded.length === 0) return;
      await issuesApi
        .linkAttachments(
          issue.id,
          uploaded.map((f) => f.id)
        )
        .catch((err: unknown) =>
          onError(errorMessage(err, 'Failed to attach file'))
        );
      const markdown = uploaded.map(attachmentMarkdown).join('\n\n');
      setDescription((prev) =>
        prev.trim() ? `${prev}\n\n${markdown}` : markdown
      );
    },
    [issue.id, onError]
  );

  const children = useMemo(
    () => board.issues.filter((i) => i.parent_issue_id === issue.id),
    [board.issues, issue.id]
  );
  const shownChildren = showAllChildren
    ? children
    : children.slice(0, SUB_TICKETS_SHOWN);
  const doneChildren = children.filter((c) => {
    const status = board.statuses.find((s) => s.id === c.status_id);
    return status?.kind === 'done';
  }).length;

  const relationships = useMemo(
    () =>
      board.relationships.filter(
        (r) => r.issue_id === issue.id || r.related_issue_id === issue.id
      ),
    [board.relationships, issue.id]
  );

  const { data: workspaceData } = useJsonPatchWsStream<WorkspacesState>(
    '/api/workspaces/streams/ws?archived=false',
    true,
    initialWorkspaces
  );
  // Coder workspaces are linked to the parent ticket on purpose: its budget
  // caps them and their commits carry its id. Which one is chewing on this
  // sub-ticket is only written down in the parent's orchestration.
  const parentId = issue.parent_issue_id;
  const parentWorkspaceCount = useMemo(
    () =>
      parentId
        ? Object.values(workspaceData?.workspaces ?? {}).filter(
            (w) => w.issue_id === parentId
          ).length
        : 0,
    [workspaceData, parentId]
  );
  const [subtaskWorkspaceIds, setSubtaskWorkspaceIds] = useState<string[]>([]);
  useEffect(() => {
    if (!parentId) return;
    let cancelled = false;
    codingApi
      .get(parentId)
      .then((view) => {
        if (cancelled) return;
        setSubtaskWorkspaceIds(
          view.subtasks.flatMap((s) =>
            s.sub_issue_id === issue.id && s.workspace_id
              ? [s.workspace_id]
              : []
          )
        );
      })
      .catch(() => {
        // Parent never got coded; nothing to find.
      });
    return () => {
      cancelled = true;
    };
    // A new coder workspace on the parent may be this sub-ticket's.
  }, [parentId, issue.id, parentWorkspaceCount]);

  const workspaces = useMemo(
    () =>
      Object.values(workspaceData?.workspaces ?? {}).filter(
        (w) => w.issue_id === issue.id || subtaskWorkspaceIds.includes(w.id)
      ),
    [workspaceData, issue.id, subtaskWorkspaceIds]
  );

  const startWorkspace = async () => {
    let preferredRepos: { repo_id: string; target_branch: string | null }[] =
      [];
    try {
      const repos = await projectsApi.listRepos(project.id);
      preferredRepos = await Promise.all(
        repos.map(async (repo) => ({
          repo_id: repo.id,
          target_branch: (await resolveDefaultTargetBranch(repo)) || null,
        }))
      );
    } catch {
      // Fall back to choosing repos in the create screen.
    }
    const prompt = [
      `${ticketDisplayId(issue, project)}: ${issue.title}`,
      issue.description?.trim(),
    ]
      .filter(Boolean)
      .join('\n\n');
    setCreateModeSeedState(
      buildWorkspaceCreateInitialState({
        prompt,
        defaults: { preferredRepos },
        linkedIssue: {
          issueId: issue.id,
          simpleId: issue.external_id ?? internalTicketId(issue, project),
          title: issue.title,
          projectId: project.id,
        },
      })
    );
    appNavigation.goToWorkspacesCreate();
  };

  const addSubTicket = async () => {
    const subTitle = newSubTitle.trim();
    if (!subTitle) return;
    try {
      await projectsApi.createIssue(project.id, {
        title: subTitle,
        parent_issue_id: issue.id,
        status_id: issue.status_id,
      });
      setNewSubTitle('');
    } catch (err) {
      onError(errorMessage(err, 'Failed to create sub-ticket'));
    }
  };

  const addRelationship = (relatedIssueId: string) => {
    issuesApi
      .createRelationship(issue.id, {
        related_issue_id: relatedIssueId,
        relationship_type: relationType,
      })
      .then(() => setLinking(false))
      .catch((err: unknown) =>
        onError(errorMessage(err, 'Failed to link tickets'))
      );
  };

  const deleteIssue = async () => {
    try {
      await issuesApi.delete(issue.id);
      onClose?.();
    } catch (err) {
      onError(errorMessage(err, 'Failed to delete ticket'));
    }
  };

  const issueLabel = (i: Issue) => `${internalTicketId(i, project)} ${i.title}`;
  const openWorkspace = (id: string) => appNavigation.goToWorkspace(id);
  const runningWorkspaces = workspaces.filter((w) => w.is_running).length;

  return (
    <aside
      className={`flex h-full w-full flex-col bg-primary ${
        embedded ? '' : 'border-l'
      }`}
    >
      <header className="flex items-center justify-between gap-base border-b px-double py-half">
        <span className="min-w-0 truncate font-mono text-sm text-low">
          {ticketDisplayId(issue, project)}
        </span>
        <div className="flex shrink-0 items-center gap-half">
          <Button size="xs" variant="outline" onClick={startWorkspace}>
            <PlayIcon className="mr-half size-icon-xs" weight="fill" />
            Start workspace
          </Button>
          {headerActions}
          {confirmDelete ? (
            <Button
              size="xs"
              variant="destructive"
              onClick={deleteIssue}
              onBlur={() => setConfirmDelete(false)}
              autoFocus
            >
              Confirm delete
            </Button>
          ) : (
            <Button
              size="xs"
              variant="ghost"
              aria-label="Delete ticket"
              onClick={() => setConfirmDelete(true)}
            >
              <TrashIcon className="size-icon-sm" />
            </Button>
          )}
          {onClose && (
            <Button
              size="xs"
              variant="ghost"
              aria-label="Close ticket"
              onClick={onClose}
            >
              <XIcon className="size-icon-sm" />
            </Button>
          )}
        </div>
      </header>

      <div className="flex flex-1 flex-col overflow-y-auto">
        <div className="flex flex-col gap-base px-double pt-double">
          <AutoResizeTextarea
            aria-label="Ticket title"
            className="w-full resize-none bg-transparent text-xl font-semibold text-high outline-none"
            value={title}
            onChange={setTitle}
            onBlur={() => {
              if (title.trim() && title !== issue.title) void update({ title });
              else setTitle(issue.title);
            }}
          />
          <TicketProperties
            project={project}
            issue={issue}
            board={board}
            update={update}
            onSelectIssue={onSelectIssue}
            onError={onError}
          />
        </div>

        <nav
          role="tablist"
          className="sticky top-0 z-10 mt-base flex border-b bg-primary px-base"
        >
          <TabButton
            label="Overview"
            active={activeTab === 'overview'}
            onClick={() => setTab('overview')}
          />
          <TabButton
            label="Plan"
            active={activeTab === 'plan'}
            attention={planWaiting || planToApprove}
            onClick={() => setTab('plan')}
          />
          {hasCoding && (
            <TabButton
              label="Coding"
              active={activeTab === 'coding'}
              onClick={() => setTab('coding')}
            />
          )}
          <TabButton
            label="Activity"
            active={activeTab === 'activity'}
            attention={planWaiting}
            onClick={() => setTab('activity')}
          />
        </nav>

        {/* Inactive tabs stay mounted so unsaved plan edits and live
            polling survive switching. */}
        <div
          role="tabpanel"
          hidden={activeTab !== 'overview'}
          className={tabPanelClass(activeTab === 'overview')}
        >
          <TicketDescription
            title={issue.title}
            value={description}
            onChange={setDescription}
            onPasteFiles={(files) => void handlePasteFiles(files)}
          />

          <CollapsibleSection
            id="sub-tickets"
            title="Sub-tickets"
            meta={
              children.length > 0
                ? `${doneChildren}/${children.length} done`
                : 'none'
            }
          >
            {children.length > 0 && (
              <ul className="flex flex-col">
                {shownChildren.map((child) => {
                  const status = board.statuses.find(
                    (s) => s.id === child.status_id
                  );
                  return (
                    <li key={child.id}>
                      <button
                        type="button"
                        onClick={() => onSelectIssue(child.id)}
                        title={child.title}
                        className="flex w-full items-center gap-base rounded-sm px-half py-half text-left text-sm hover:bg-secondary"
                      >
                        <span
                          className="h-2 w-2 shrink-0 rounded-full"
                          style={{
                            backgroundColor: `hsl(${status?.color ?? '0 0% 50%'})`,
                          }}
                          title={status?.name}
                        />
                        <span className="shrink-0 whitespace-nowrap font-mono text-xs text-low">
                          {internalTicketId(child, project)}
                        </span>
                        <span className="min-w-0 truncate">{child.title}</span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
            {children.length > SUB_TICKETS_SHOWN && (
              <button
                type="button"
                className="self-start px-half text-xs text-low hover:text-normal"
                onClick={() => setShowAllChildren((s) => !s)}
              >
                {showAllChildren ? 'Show fewer' : `Show all ${children.length}`}
              </button>
            )}
            <form
              className="flex gap-half"
              onSubmit={(e) => {
                e.preventDefault();
                void addSubTicket();
              }}
            >
              <Input
                aria-label="New sub-ticket title"
                placeholder="New sub-ticket"
                value={newSubTitle}
                onChange={(e) => setNewSubTitle(e.target.value)}
              />
              <Button type="submit" size="sm" variant="outline">
                <PlusIcon className="size-icon-xs" />
              </Button>
            </form>
          </CollapsibleSection>

          <CollapsibleSection
            id="relationships"
            title="Relationships"
            meta={relationships.length || 'none'}
            action={
              !linking && (
                <button
                  type="button"
                  className="flex items-center gap-half text-xs text-low hover:text-normal"
                  onClick={() => setLinking(true)}
                >
                  <LinkIcon className="size-icon-xs" /> Link ticket
                </button>
              )
            }
          >
            {relationships.map((r) => {
              const outgoing = r.issue_id === issue.id;
              const otherId = outgoing ? r.related_issue_id : r.issue_id;
              const other = board.issuesById[otherId];
              const label = outgoing
                ? RELATIONSHIP_LABELS[r.relationship_type]
                : r.relationship_type === 'blocking'
                  ? 'Blocked by'
                  : r.relationship_type === 'has_duplicate'
                    ? 'Duplicates'
                    : 'Related to';
              return (
                <div
                  key={r.id}
                  className="flex items-center justify-between gap-base text-sm"
                >
                  <button
                    type="button"
                    className="truncate text-left hover:underline"
                    onClick={() => other && onSelectIssue(other.id)}
                  >
                    <span className="text-low">{label} </span>
                    {other ? issueLabel(other) : 'Unknown ticket'}
                  </button>
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label="Remove relationship"
                    onClick={() =>
                      issuesApi
                        .deleteRelationship(r.id)
                        .catch((err: unknown) =>
                          onError(errorMessage(err, 'Failed to unlink'))
                        )
                    }
                  >
                    <XIcon className="size-icon-xs" />
                  </Button>
                </div>
              );
            })}
            {linking && (
              <div className="grid grid-cols-[140px_1fr_auto] gap-half">
                <Select
                  value={relationType}
                  onValueChange={(v) =>
                    setRelationType(v as IssueRelationshipType)
                  }
                >
                  <SelectTrigger aria-label="Relationship type">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {Object.entries(RELATIONSHIP_LABELS).map(
                      ([value, label]) => (
                        <SelectItem key={value} value={value}>
                          {label}
                        </SelectItem>
                      )
                    )}
                  </SelectContent>
                </Select>
                <Select value="" onValueChange={addRelationship}>
                  <SelectTrigger aria-label="Related ticket">
                    <SelectValue placeholder="Link a ticket…" />
                  </SelectTrigger>
                  <SelectContent>
                    {board.issues
                      .filter((i) => i.id !== issue.id)
                      .map((i) => (
                        <SelectItem key={i.id} value={i.id}>
                          {issueLabel(i)}
                        </SelectItem>
                      ))}
                  </SelectContent>
                </Select>
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label="Cancel linking"
                  onClick={() => setLinking(false)}
                >
                  <XIcon className="size-icon-xs" />
                </Button>
              </div>
            )}
          </CollapsibleSection>

          <CollapsibleSection
            id="workspaces"
            title="Workspaces"
            meta={`${workspaces.length || 'none'}${
              runningWorkspaces ? ` · ${runningWorkspaces} running` : ''
            }`}
          >
            {workspaces.length === 0 && (
              <p className="text-sm text-low">
                Start one from the header to work on this with an agent.
              </p>
            )}
            {workspaces.map((w) => (
              <button
                key={w.id}
                type="button"
                onClick={() => openWorkspace(w.id)}
                title={`${w.name ?? w.branch}\n${w.branch}`}
                className="flex items-center justify-between gap-base rounded-sm px-half py-half text-left text-sm hover:bg-secondary"
              >
                <span className="truncate">{w.name ?? w.branch}</span>
                <span className="flex shrink-0 items-center gap-half text-xs">
                  {w.id === currentWorkspaceId && (
                    <span className="text-low">This workspace</span>
                  )}
                  {w.is_running && <span className="text-brand">Running</span>}
                </span>
              </button>
            ))}
          </CollapsibleSection>

          <TicketRelated
            issueId={issue.id}
            refreshKey={issue.updated_at}
            onSelectIssue={onSelectIssue}
          />
        </div>

        <div
          role="tabpanel"
          hidden={activeTab !== 'plan'}
          className={tabPanelClass(activeTab === 'plan')}
        >
          <TicketPlan
            issueId={issue.id}
            refreshKey={planningEvents}
            onOpenWorkspace={openWorkspace}
            onShowComments={() => setTab('activity')}
            onError={onError}
          />
        </div>

        <div
          role="tabpanel"
          hidden={activeTab !== 'coding'}
          className={tabPanelClass(activeTab === 'coding')}
        >
          <TicketCoding
            issueId={issue.id}
            refreshKey={codingEvents}
            planApproved={planApproved}
            onOpenWorkspace={openWorkspace}
            onError={onError}
          />
        </div>

        <div
          role="tabpanel"
          hidden={activeTab !== 'activity'}
          className={tabPanelClass(activeTab === 'activity')}
        >
          <TicketComments issueId={issue.id} onError={onError} />

          <TicketHistory
            issueId={issue.id}
            refreshKey={historyEvents}
            onError={onError}
          />

          <TicketAttachments
            issueId={issue.id}
            refreshKey={attachmentEvents}
            onError={onError}
          />

          <TicketTimeline
            issueId={issue.id}
            createdAt={issue.created_at}
            events={timeline.events}
            isLoading={timeline.isLoading}
          />
        </div>
      </div>
    </aside>
  );
}
