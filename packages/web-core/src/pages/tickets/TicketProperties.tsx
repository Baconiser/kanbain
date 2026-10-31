import { useEffect, useMemo, useState } from 'react';
import {
  ChatCircleDotsIcon,
  CheckIcon,
  GearSixIcon,
  LightningIcon,
  PlusIcon,
  TagIcon,
  TreeStructureIcon,
} from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { KanbanBadge } from '@kanbain/ui/components/KanbanBadge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@kanbain/ui/components/Popover';
import { PriorityIcon } from '@kanbain/ui/components/PriorityIcon';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@kanbain/ui/components/Select';
import { Tooltip } from '@kanbain/ui/components/Tooltip';
import type { Issue, IssuePriority, Project, UpdateIssue } from 'shared/types';
import type { ProjectBoard } from '@/shared/hooks/useTicketStreams';
import { issuesApi, projectsApi } from '@/shared/lib/ticketsApi';
import { internalTicketId } from './ticketIds';

const NONE = '__none__';
const PRIORITIES: IssuePriority[] = ['urgent', 'high', 'medium', 'low'];

/** A small pill-shaped trigger, so properties sit on one wrapping row. */
export const CHIP =
  'inline-flex h-7 items-center gap-half rounded-sm border px-base text-xs text-normal hover:bg-secondary';
// `cn` doesn't merge classes, so the trigger's own sizes need `!`.
const CHIP_SELECT = `${CHIP} !h-7 !w-auto !px-base !py-0 focus:!ring-0 focus:!ring-offset-0 [&>svg]:!h-3 [&>svg]:!w-3`;

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

interface TicketPropertiesProps {
  project: Project;
  issue: Issue;
  board: ProjectBoard;
  update: (data: UpdateIssue) => Promise<unknown>;
  onSelectIssue: (issueId: string) => void;
  onError: (message: string) => void;
}

/**
 * Everything about a ticket that isn't its content: status, priority, agent
 * mode and tags as chips, the rarely touched fields behind a Details popup.
 */
export function TicketProperties({
  project,
  issue,
  board,
  update,
  onSelectIssue,
  onError,
}: TicketPropertiesProps) {
  const parent = issue.parent_issue_id
    ? board.issuesById[issue.parent_issue_id]
    : undefined;
  const yolo = issue.mode === 'yolo';

  return (
    <div className="flex flex-col gap-half">
      <div className="flex flex-wrap items-center gap-half">
        <Select
          value={issue.status_id}
          onValueChange={(status_id) => void update({ status_id })}
        >
          <SelectTrigger aria-label="Status" className={CHIP_SELECT}>
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

        <Select
          value={issue.priority ?? NONE}
          onValueChange={(v) =>
            void update({
              priority: v === NONE ? null : (v as IssuePriority),
            })
          }
        >
          <SelectTrigger aria-label="Priority" className={CHIP_SELECT}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NONE}>
              <span className="text-low">No priority</span>
            </SelectItem>
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

        <Tooltip
          content={
            yolo
              ? 'Yolo: agents state assumptions and keep going. Click for Ask.'
              : 'Ask: agents stop and ask questions. Click for Yolo.'
          }
        >
          <button
            type="button"
            aria-label="Agent mode"
            aria-pressed={yolo}
            data-mode={issue.mode}
            className={`${CHIP} ${yolo ? 'border-brand/50 text-brand' : ''}`}
            onClick={() => void update({ mode: yolo ? 'ask' : 'yolo' })}
          >
            {yolo ? (
              <LightningIcon className="size-icon-xs" weight="fill" />
            ) : (
              <ChatCircleDotsIcon className="size-icon-xs" />
            )}
            {yolo ? 'Yolo' : 'Ask'}
          </button>
        </Tooltip>

        {parent && (
          <Tooltip content={`Sub-ticket of ${parent.title}`}>
            <button
              type="button"
              className={CHIP}
              onClick={() => onSelectIssue(parent.id)}
            >
              <TreeStructureIcon className="size-icon-xs text-low" />
              <span className="font-mono">
                {internalTicketId(parent, project)}
              </span>
            </button>
          </Tooltip>
        )}

        {issue.budget_usd != null && (
          <span className={`${CHIP} hover:bg-transparent text-low`}>
            ${issue.budget_usd.toFixed(2)} budget
          </span>
        )}

        <DetailsPopover
          project={project}
          issue={issue}
          board={board}
          update={update}
        />
      </div>

      <TagsRow
        issue={issue}
        project={project}
        board={board}
        onError={onError}
      />
    </div>
  );
}

function DetailsPopover({
  project,
  issue,
  board,
  update,
}: {
  project: Project;
  issue: Issue;
  board: ProjectBoard;
  update: (data: UpdateIssue) => Promise<unknown>;
}) {
  const [externalId, setExternalId] = useState(issue.external_id ?? '');
  const [budget, setBudget] = useState(issue.budget_usd?.toString() ?? '');

  useEffect(() => {
    setExternalId(issue.external_id ?? '');
    setBudget(issue.budget_usd?.toString() ?? '');
  }, [issue.external_id, issue.budget_usd]);

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

  return (
    <Popover>
      <Tooltip content="External ID, budget, parent">
        <PopoverTrigger asChild>
          <button type="button" aria-label="Ticket details" className={CHIP}>
            <GearSixIcon className="size-icon-xs text-low" />
            Details
          </button>
        </PopoverTrigger>
      </Tooltip>
      <PopoverContent align="start" className="flex w-80 flex-col gap-base">
        <label className="flex flex-col gap-half">
          <span className="text-xs text-low">External ID</span>
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
        </label>
        <label className="flex flex-col gap-half">
          <span className="text-xs text-low">Budget (USD)</span>
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
        </label>
        <div className="flex flex-col gap-half">
          <span className="text-xs text-low">Parent</span>
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
                  {internalTicketId(i, project)} {i.title}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** The ticket's tags, with a popup to toggle or create them. */
function TagsRow({
  issue,
  project,
  board,
  onError,
}: {
  issue: Issue;
  project: Project;
  board: ProjectBoard;
  onError: (message: string) => void;
}) {
  const [newTagName, setNewTagName] = useState('');
  const selectedTagIds = board.tagIdsByIssue[issue.id] ?? [];
  const selected = board.tags.filter((t) => selectedTagIds.includes(t.id));

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

  return (
    <div className="flex flex-wrap items-center gap-half">
      {selected.map((tag) => (
        <KanbanBadge key={tag.id} name={tag.name} color={tag.color} />
      ))}
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            aria-label="Edit tags"
            className="inline-flex h-6 items-center gap-half rounded-sm px-half text-xs text-low hover:bg-secondary hover:text-normal"
          >
            {selected.length === 0 ? (
              <>
                <TagIcon className="size-icon-xs" /> Add tags
              </>
            ) : (
              <PlusIcon className="size-icon-xs" />
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="flex w-72 flex-col gap-base">
          {board.tags.length > 0 && (
            <ul className="flex max-h-56 flex-col overflow-y-auto">
              {board.tags.map((tag) => {
                const active = selectedTagIds.includes(tag.id);
                return (
                  <li key={tag.id}>
                    <button
                      type="button"
                      aria-pressed={active}
                      onClick={() => toggleTag(tag.id)}
                      className="flex w-full items-center gap-base rounded-sm px-half py-half text-left hover:bg-secondary"
                    >
                      <CheckIcon
                        className={`size-icon-xs ${active ? 'text-brand' : 'invisible'}`}
                      />
                      <KanbanBadge name={tag.name} color={tag.color} />
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
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
        </PopoverContent>
      </Popover>
    </div>
  );
}
