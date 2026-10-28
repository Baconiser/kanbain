import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  GearSixIcon,
  KanbanIcon,
  PlusIcon,
  UploadSimpleIcon,
  XIcon,
} from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useProjectBoard, useProjects } from '@/shared/hooks/useTicketStreams';
import { CreateTicketDialog } from './CreateTicketDialog';
import { DropTicketDialog } from './DropTicketDialog';
import { ProjectSettingsDialog } from './ProjectSettingsDialog';
import { TicketBoard } from './TicketBoard';
import { TicketPanel } from './TicketPanel';
import { ticketMatchesQuery } from './ticketIds';
import { onOpenTicketRequest, takeOpenTicketRequest } from './openTicket';

const PROJECT_STORAGE_KEY = 'kanbain.tickets.projectId';

function readStoredProjectId(): string | null {
  try {
    return window.localStorage.getItem(PROJECT_STORAGE_KEY);
  } catch {
    return null;
  }
}

function storeProjectId(id: string) {
  try {
    window.localStorage.setItem(PROJECT_STORAGE_KEY, id);
  } catch {
    // Storage may be unavailable; selection just won't persist.
  }
}

export function TicketsPage() {
  usePageTitle('Tickets');
  const { projects, isLoading: projectsLoading } = useProjects();
  const [projectId, setProjectId] = useState<string | null>(
    readStoredProjectId
  );
  const [selectedIssueId, setSelectedIssueId] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [showHidden, setShowHidden] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Tickets opened from search results elsewhere.
  useEffect(() => {
    const open = () => {
      const request = takeOpenTicketRequest();
      if (!request) return;
      setProjectId(request.projectId);
      storeProjectId(request.projectId);
      setSelectedIssueId(request.issueId);
    };
    open();
    return onOpenTicketRequest(open);
  }, []);

  // Fall back to the first project when the stored one is gone. A project
  // created moments ago may not have arrived on the stream yet; keep the
  // selection and show the board once it does.
  const storedProject = projects.find((p) => p.id === projectId);
  const project = storedProject ?? projects[0] ?? null;
  const board = useProjectBoard(project?.id);
  const selectedIssue = selectedIssueId
    ? board.issuesById[selectedIssueId]
    : undefined;

  const selectProject = (id: string) => {
    setProjectId(id);
    storeProjectId(id);
    setSelectedIssueId(null);
  };

  const filteredIssues = useMemo(
    () =>
      project
        ? board.issues.filter((i) => ticketMatchesQuery(i, project, query))
        : [],
    [board.issues, project, query]
  );

  const showError = useCallback((message: string) => setError(message), []);

  const createProject = async () => {
    const created = await ProjectSettingsDialog.show({});
    if (created) selectProject(created.id);
  };

  const addIssue = async (statusId?: string) => {
    if (!project) return;
    const issue = await CreateTicketDialog.show({
      projectId: project.id,
      statusId,
    });
    if (issue) setSelectedIssueId(issue.id);
  };

  const dropTicket = async () => {
    if (!project) return;
    const issue = await DropTicketDialog.show({ projectId: project.id });
    if (issue) setSelectedIssueId(issue.id);
  };

  if (projectsLoading) {
    return (
      <div className="flex h-full items-center justify-center text-low">
        Loading…
      </div>
    );
  }

  if (!project) {
    return (
      <div className="h-full w-full overflow-auto bg-primary">
        <div className="mx-auto flex min-h-full w-full max-w-xl items-center px-base py-double">
          <div className="w-full rounded-sm border border-border bg-secondary p-double text-center">
            <KanbanIcon className="mx-auto size-10 text-low" weight="bold" />
            <h1 className="mt-base text-xl font-semibold text-high">
              Create your first project
            </h1>
            <p className="mt-half text-sm text-low">
              A project groups tickets and the repositories agents work in.
              Everything is stored locally.
            </p>
            <Button className="mt-double" onClick={() => void createProject()}>
              <PlusIcon className="mr-half size-icon-xs" weight="bold" />
              New project
            </Button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col bg-primary">
      <header className="flex flex-wrap items-center gap-base px-double py-base">
        <select
          aria-label="Project"
          className="h-9 max-w-[240px] rounded-sm border bg-primary px-base text-base font-medium"
          value={project.id}
          onChange={(e) => selectProject(e.target.value)}
        >
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <Button
          size="sm"
          variant="ghost"
          aria-label="Project settings"
          onClick={() => void ProjectSettingsDialog.show({ project })}
        >
          <GearSixIcon className="size-icon-sm" />
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void createProject()}>
          <PlusIcon className="mr-half size-icon-xs" />
          Project
        </Button>
        <div className="ml-auto flex items-center gap-base">
          <Input
            aria-label="Filter tickets"
            className="w-[220px]"
            placeholder="Filter by title or ID"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <label className="flex items-center gap-half text-sm text-low">
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.target.checked)}
            />
            Hidden columns
          </label>
          <Button size="sm" variant="outline" onClick={() => void dropTicket()}>
            <UploadSimpleIcon className="mr-half size-icon-xs" weight="bold" />
            Drop ticket
          </Button>
          <Button size="sm" onClick={() => void addIssue()}>
            <PlusIcon className="mr-half size-icon-xs" weight="bold" />
            New ticket
          </Button>
        </div>
      </header>

      {error && (
        <div
          role="alert"
          className="mx-double mb-base flex items-center justify-between rounded-sm border border-error bg-error/10 px-base py-half text-sm"
        >
          <span>{error}</span>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => setError(null)}
          >
            <XIcon className="size-icon-xs" />
          </button>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1">
          {board.isLoading ? (
            <div className="flex h-full items-center justify-center text-low">
              Loading board…
            </div>
          ) : (
            <TicketBoard
              project={project}
              board={board}
              issues={filteredIssues}
              showHidden={showHidden}
              selectedIssueId={selectedIssueId}
              onSelectIssue={setSelectedIssueId}
              onAddIssue={(statusId) => void addIssue(statusId)}
              onError={showError}
            />
          )}
        </div>
        {selectedIssue && (
          <div className="w-[min(520px,45vw)] shrink-0">
            <TicketPanel
              key={selectedIssue.id}
              project={project}
              issue={selectedIssue}
              board={board}
              onClose={() => setSelectedIssueId(null)}
              onSelectIssue={setSelectedIssueId}
              onError={showError}
            />
          </div>
        )}
      </div>
    </div>
  );
}
