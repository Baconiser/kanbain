import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { PlayIcon, PlusIcon, TrashIcon, XIcon } from '@phosphor-icons/react';
import { AutoResizeTextarea } from '@kanbain/ui/components/AutoResizeTextarea';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { KanbanBadge } from '@kanbain/ui/components/KanbanBadge';
import { PriorityIcon } from '@kanbain/ui/components/PriorityIcon';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@kanbain/ui/components/Select';
import type {
  Issue,
  IssueMode,
  IssuePriority,
  IssueRelationshipType,
  Project,
  UpdateIssue,
  WorkspaceWithStatus,
} from 'shared/types';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { resolveDefaultTargetBranch } from '@/shared/hooks/useProjectRepoDefaults';
import type { ProjectBoard } from '@/shared/hooks/useTicketStreams';
import { attachmentsApi } from '@/shared/lib/api';
import { issuesApi, projectsApi } from '@/shared/lib/ticketsApi';
import { buildWorkspaceCreateInitialState } from '@/shared/lib/workspaceCreateState';
import { setCreateModeSeedState } from '@/features/create-mode/model/createModeSeedStore';
import { useIssueTimeline } from '@/shared/hooks/useTicketStreams';
import { TicketAttachments } from './TicketAttachments';
import { TicketComments } from './TicketComments';
import { TicketHistory } from './TicketHistory';
import { TicketPlan } from './TicketPlan';
import { TicketRelated } from './TicketRelated';
import { TicketTimeline } from './TicketTimeline';
import { internalTicketId, ticketDisplayId } from './ticketIds';

const NONE = '__none__';
const PRIORITIES: IssuePriority[] = ['urgent', 'high', 'medium', 'low'];
const RELATIONSHIP_LABELS: Record<IssueRelationshipType, string> = {
  blocking: 'Blocks',
  related: 'Related to',
  has_duplicate: 'Duplicated by',
};

interface TicketPanelProps {
  project: Project;
  issue: Issue;
  board: ProjectBoard;
  onClose: () => void;
  onSelectIssue: (issueId: string) => void;
  onError: (message: string) => void;
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

function PropertyRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="grid grid-cols-[110px_1fr] items-center gap-base">
      <span className="text-sm text-low">{label}</span>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

function Section({
  title,
  action,
  children,
}: {
  title: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-half">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-normal">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

type WorkspacesState = { workspaces: Record<string, WorkspaceWithStatus> };
const initialWorkspaces = (): WorkspacesState => ({ workspaces: {} });

export function TicketPanel({
  project,
  issue,
  board,
  onClose,
  onSelectIssue,
  onError,
}: TicketPanelProps) {
  const appNavigation = useAppNavigation();
  const [title, setTitle] = useState(issue.title);
  const [externalId, setExternalId] = useState(issue.external_id ?? '');
  const [budget, setBudget] = useState(issue.budget_usd?.toString() ?? '');
  const [description, setDescription] = useState(issue.description ?? '');
  const [newSubTitle, setNewSubTitle] = useState('');
  const [newTagName, setNewTagName] = useState('');
  const [relationType, setRelationType] =
    useState<IssueRelationshipType>('related');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const savedDescription = useRef(issue.description ?? '');

  // The page remounts this panel per ticket (`key`); here we only follow
  // remote changes to fields that are not being edited.
  useEffect(() => {
    setTitle(issue.title);
    setExternalId(issue.external_id ?? '');
    setBudget(issue.budget_usd?.toString() ?? '');
  }, [issue.title, issue.external_id, issue.budget_usd]);

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

  const planningEvents = timeline.events.filter(
    (e) =>
      e.event_type === 'planning_started' ||
      e.event_type === 'plan_questions' ||
      e.event_type === 'plan_version' ||
      e.event_type === 'plan_approved' ||
      e.event_type === 'plan_rejected' ||
      e.event_type === 'planning_failed'
  ).length;

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
  const parent = issue.parent_issue_id
    ? board.issuesById[issue.parent_issue_id]
    : undefined;
  // A ticket cannot be parented under itself or one of its descendants.
  const parentCandidates = useMemo(() => {
    const descendants = new Set<string>([issue.id]);
    let added = true;
    while (added) {
      added = false;
      for (const i of board.issues) {
        if (i.parent_issue_id && descendants.has(i.parent_issue_id)) {
          if (!descendants.has(i.id)) {
            descendants.add(i.id);
            added = true;
          }
        }
      }
    }
    return board.issues.filter((i) => !descendants.has(i.id));
  }, [board.issues, issue.id]);

  const relationships = useMemo(
    () =>
      board.relationships.filter(
        (r) => r.issue_id === issue.id || r.related_issue_id === issue.id
      ),
    [board.relationships, issue.id]
  );
  const selectedTagIds = board.tagIdsByIssue[issue.id] ?? [];

  const { data: workspaceData } = useJsonPatchWsStream<WorkspacesState>(
    '/api/workspaces/streams/ws?archived=false',
    true,
    initialWorkspaces
  );
  const workspaces = useMemo(
    () =>
      Object.values(workspaceData?.workspaces ?? {}).filter(
        (w) => w.issue_id === issue.id
      ),
    [workspaceData, issue.id]
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

  const toggleTag = (tagId: string) => {
    const request = selectedTagIds.includes(tagId)
      ? issuesApi.removeTag(issue.id, tagId)
      : issuesApi.addTag(issue.id, tagId);
    request.catch((err: unknown) =>
      onError(errorMessage(err, 'Failed to update tags'))
    );
  };

  const createTag = async () => {
    const name = newTagName.trim();
    if (!name) return;
    try {
      const hue = Math.floor(Math.random() * 360);
      const tag = await projectsApi.createTag(project.id, {
        name,
        color: `${hue} 70% 50%`,
      });
      await issuesApi.addTag(issue.id, tag.id);
      setNewTagName('');
    } catch (err) {
      onError(errorMessage(err, 'Failed to create tag'));
    }
  };

  const addRelationship = (relatedIssueId: string) => {
    issuesApi
      .createRelationship(issue.id, {
        related_issue_id: relatedIssueId,
        relationship_type: relationType,
      })
      .catch((err: unknown) =>
        onError(errorMessage(err, 'Failed to link tickets'))
      );
  };

  const deleteIssue = async () => {
    try {
      await issuesApi.delete(issue.id);
      onClose();
    } catch (err) {
      onError(errorMessage(err, 'Failed to delete ticket'));
    }
  };

  const issueLabel = (i: Issue) => `${internalTicketId(i, project)} ${i.title}`;

  return (
    <aside className="flex h-full w-full flex-col border-l bg-primary">
      <header className="flex items-center justify-between gap-base border-b px-double py-base">
        <span className="truncate font-mono text-sm text-low">
          {ticketDisplayId(issue, project)}
        </span>
        <div className="flex items-center gap-half">
          <Button size="sm" onClick={startWorkspace}>
            <PlayIcon className="mr-half size-icon-xs" weight="fill" />
            Start workspace
          </Button>
          {confirmDelete ? (
            <Button size="sm" variant="destructive" onClick={deleteIssue}>
              Confirm delete
            </Button>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              aria-label="Delete ticket"
              onClick={() => setConfirmDelete(true)}
            >
              <TrashIcon className="size-icon-sm" />
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            aria-label="Close ticket"
            onClick={onClose}
          >
            <XIcon className="size-icon-sm" />
          </Button>
        </div>
      </header>

      <div className="flex flex-1 flex-col gap-double overflow-y-auto px-double py-double [&>*]:shrink-0">
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

        <div className="flex flex-col gap-base">
          <PropertyRow label="Status">
            <Select
              value={issue.status_id}
              onValueChange={(status_id) => void update({ status_id })}
            >
              <SelectTrigger aria-label="Status">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {board.statuses.map((s) => (
                  <SelectItem key={s.id} value={s.id}>
                    <span className="flex items-center gap-half">
                      <span
                        className="h-2 w-2 rounded-full"
                        style={{ backgroundColor: `hsl(${s.color})` }}
                      />
                      {s.name}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </PropertyRow>
          <PropertyRow label="Priority">
            <Select
              value={issue.priority ?? NONE}
              onValueChange={(v) =>
                void update({
                  priority: v === NONE ? null : (v as IssuePriority),
                })
              }
            >
              <SelectTrigger aria-label="Priority">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE}>No priority</SelectItem>
                {PRIORITIES.map((p) => (
                  <SelectItem key={p} value={p}>
                    <span className="flex items-center gap-half capitalize">
                      <PriorityIcon priority={p} />
                      {p}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </PropertyRow>
          <PropertyRow label="Agent mode">
            <Select
              value={issue.mode}
              onValueChange={(v) => void update({ mode: v as IssueMode })}
            >
              <SelectTrigger aria-label="Agent mode">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="ask">Ask — agents ask questions</SelectItem>
                <SelectItem value="yolo">
                  Yolo — agents state assumptions
                </SelectItem>
              </SelectContent>
            </Select>
          </PropertyRow>
          <PropertyRow label="External ID">
            <Input
              aria-label="External ID"
              placeholder="e.g. PROJ-123"
              value={externalId}
              onChange={(e) => setExternalId(e.target.value)}
              onBlur={() => {
                if (externalId !== (issue.external_id ?? '')) {
                  void update({ external_id: externalId.trim() || null });
                }
              }}
            />
          </PropertyRow>
          <PropertyRow label="Budget (USD)">
            <Input
              aria-label="Budget in USD"
              type="number"
              min={0}
              step={0.5}
              placeholder="Project default"
              value={budget}
              onChange={(e) => setBudget(e.target.value)}
              onBlur={() => {
                const value = budget.trim() === '' ? null : Number(budget);
                if (value !== null && (Number.isNaN(value) || value < 0)) {
                  setBudget(issue.budget_usd?.toString() ?? '');
                  return;
                }
                if (value !== issue.budget_usd) {
                  void update({ budget_usd: value });
                }
              }}
            />
          </PropertyRow>
          <PropertyRow label="Parent">
            <Select
              value={issue.parent_issue_id ?? NONE}
              onValueChange={(v) =>
                void update({ parent_issue_id: v === NONE ? null : v })
              }
            >
              <SelectTrigger aria-label="Parent ticket">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE}>No parent</SelectItem>
                {parentCandidates.map((i) => (
                  <SelectItem key={i.id} value={i.id}>
                    {issueLabel(i)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {parent && (
              <button
                type="button"
                className="mt-half text-xs text-low hover:text-normal"
                onClick={() => onSelectIssue(parent.id)}
              >
                Open {internalTicketId(parent, project)}
              </button>
            )}
          </PropertyRow>
        </div>

        <Section title="Tags">
          <div className="flex flex-wrap gap-half">
            {board.tags.map((tag) => {
              const active = selectedTagIds.includes(tag.id);
              return (
                <button
                  key={tag.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => toggleTag(tag.id)}
                  className={active ? '' : 'opacity-50 hover:opacity-100'}
                >
                  <KanbanBadge name={tag.name} color={tag.color} />
                </button>
              );
            })}
          </div>
          <form
            className="flex gap-half"
            onSubmit={(e) => {
              e.preventDefault();
              void createTag();
            }}
          >
            <Input
              aria-label="New tag"
              placeholder="New tag"
              value={newTagName}
              onChange={(e) => setNewTagName(e.target.value)}
            />
            <Button type="submit" size="sm" variant="outline">
              Add
            </Button>
          </form>
        </Section>

        <Section title="Description">
          <div className="rounded-sm border bg-secondary p-base">
            <WYSIWYGEditor
              placeholder="Describe the work. Paste images or files to attach them."
              value={description}
              onChange={setDescription}
              onPasteFiles={(files) => void handlePasteFiles(files)}
              className="min-h-[160px]"
            />
          </div>
        </Section>

        <Section title={`Sub-tickets (${children.length})`}>
          {children.map((child) => {
            const status = board.statuses.find((s) => s.id === child.status_id);
            return (
              <button
                key={child.id}
                type="button"
                onClick={() => onSelectIssue(child.id)}
                className="flex items-center gap-base rounded-sm px-half py-half text-left text-sm hover:bg-secondary"
              >
                <span
                  className="h-2 w-2 shrink-0 rounded-full"
                  style={{
                    backgroundColor: `hsl(${status?.color ?? '0 0% 50%'})`,
                  }}
                  title={status?.name}
                />
                <span className="font-mono text-low">
                  {internalTicketId(child, project)}
                </span>
                <span className="truncate">{child.title}</span>
              </button>
            );
          })}
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
        </Section>

        <Section title="Relationships">
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
                  size="sm"
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
          <div className="grid grid-cols-[140px_1fr] gap-half">
            <Select
              value={relationType}
              onValueChange={(v) => setRelationType(v as IssueRelationshipType)}
            >
              <SelectTrigger aria-label="Relationship type">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(RELATIONSHIP_LABELS).map(([value, label]) => (
                  <SelectItem key={value} value={value}>
                    {label}
                  </SelectItem>
                ))}
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
          </div>
        </Section>

        <Section title={`Workspaces (${workspaces.length})`}>
          {workspaces.length === 0 && (
            <p className="text-sm text-low">
              No workspaces yet. Start one to work on this ticket with an agent.
            </p>
          )}
          {workspaces.map((w) => (
            <button
              key={w.id}
              type="button"
              onClick={() => appNavigation.goToWorkspace(w.id)}
              className="flex items-center justify-between gap-base rounded-sm px-half py-half text-left text-sm hover:bg-secondary"
            >
              <span className="truncate">{w.name ?? w.branch}</span>
              <span className="shrink-0 text-xs text-low">
                {w.is_running ? 'Running' : w.branch}
              </span>
            </button>
          ))}
        </Section>

        <TicketPlan
          issueId={issue.id}
          refreshKey={planningEvents}
          onOpenWorkspace={(id) => appNavigation.goToWorkspace(id)}
          onError={onError}
        />

        <TicketRelated
          issueId={issue.id}
          refreshKey={issue.updated_at}
          onSelectIssue={onSelectIssue}
        />

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

        <TicketComments issueId={issue.id} onError={onError} />

        <TicketTimeline
          issueId={issue.id}
          createdAt={issue.created_at}
          events={timeline.events}
          isLoading={timeline.isLoading}
        />
      </div>
    </aside>
  );
}
