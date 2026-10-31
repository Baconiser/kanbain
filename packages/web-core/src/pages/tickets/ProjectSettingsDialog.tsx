import { useEffect, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { useQuery } from '@tanstack/react-query';
import {
  ArrowDownIcon,
  ArrowUpIcon,
  EyeIcon,
  EyeSlashIcon,
  TrashIcon,
  XIcon,
} from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { Label } from '@kanbain/ui/components/Label';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@kanbain/ui/components/KeyboardDialog';
import type {
  Project,
  ProjectStatus,
  ProjectStatusKind,
  Repo,
} from 'shared/types';
import { repoApi } from '@/shared/lib/api';
import { defineModal, getErrorMessage } from '@/shared/lib/modals';
import { projectsApi } from '@/shared/lib/ticketsApi';
import { useProjectBoard } from '@/shared/hooks/useTicketStreams';

export interface ProjectSettingsDialogProps {
  /** Omit to create a new project. */
  project?: Project;
}

const STATUS_KINDS: { value: ProjectStatusKind; label: string }[] = [
  { value: 'backlog', label: 'Backlog' },
  { value: 'planning', label: 'Planning' },
  { value: 'awaiting_approval', label: 'Awaiting approval' },
  { value: 'in_progress', label: 'In progress' },
  { value: 'testing', label: 'Testing' },
  { value: 'review', label: 'Review' },
  { value: 'done', label: 'Done' },
  { value: 'blocked', label: 'Blocked' },
  { value: 'custom', label: 'Custom' },
];

const selectClass = 'h-9 rounded-sm border bg-primary px-base text-sm';

function RepoPicker({
  linked,
  onLink,
  onUnlink,
}: {
  linked: Repo[];
  onLink: (repoId: string) => void;
  onUnlink: (repoId: string) => void;
}) {
  const { data: allRepos = [] } = useQuery({
    queryKey: ['repos'],
    queryFn: () => repoApi.list(),
  });
  const available = allRepos.filter((r) => !linked.some((l) => l.id === r.id));

  return (
    <div className="flex flex-col gap-half">
      {linked.length === 0 && (
        <p className="text-sm text-low">
          No repositories linked. Workspaces started from tickets use these.
        </p>
      )}
      {linked.map((repo) => (
        <div
          key={repo.id}
          className="flex items-center justify-between gap-base rounded-sm border px-base py-half text-sm"
        >
          <span className="truncate" title={repo.path}>
            {repo.display_name}
          </span>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label={`Unlink ${repo.display_name}`}
            onClick={() => onUnlink(repo.id)}
          >
            <XIcon className="size-icon-xs" />
          </Button>
        </div>
      ))}
      {available.length > 0 && (
        <select
          aria-label="Link repository"
          className={selectClass}
          value=""
          onChange={(e) => e.target.value && onLink(e.target.value)}
        >
          <option value="">Link a repository…</option>
          {available.map((repo) => (
            <option key={repo.id} value={repo.id}>
              {repo.display_name}
            </option>
          ))}
        </select>
      )}
      {allRepos.length === 0 && (
        <p className="text-sm text-low">
          Add repositories from the workspaces screen first.
        </p>
      )}
    </div>
  );
}

function StatusEditor({
  projectId,
  onError,
}: {
  projectId: string;
  onError: (message: string) => void;
}) {
  const board = useProjectBoard(projectId);
  const [newName, setNewName] = useState('');
  const run = (p: Promise<unknown>) =>
    p.catch((err: unknown) => onError(getErrorMessage(err) || 'Failed'));

  const swap = (a: ProjectStatus, b: ProjectStatus) => {
    void run(
      Promise.all([
        projectsApi.updateStatus(a.id, { sort_order: b.sort_order }),
        projectsApi.updateStatus(b.id, { sort_order: a.sort_order }),
      ])
    );
  };

  return (
    <div className="flex flex-col gap-half">
      {board.statuses.map((status, index) => (
        <div key={status.id} className="flex items-center gap-half">
          <span
            className="h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: `hsl(${status.color})` }}
          />
          <Input
            aria-label="Status name"
            defaultValue={status.name}
            onBlur={(e) => {
              const name = e.target.value.trim();
              if (name && name !== status.name) {
                void run(projectsApi.updateStatus(status.id, { name }));
              }
            }}
          />
          <select
            aria-label="Pipeline role"
            className={selectClass}
            value={status.kind}
            onChange={(e) =>
              void run(
                projectsApi.updateStatus(status.id, {
                  kind: e.target.value as ProjectStatusKind,
                })
              )
            }
          >
            {STATUS_KINDS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label={status.hidden ? 'Show column' : 'Hide column'}
            onClick={() =>
              void run(
                projectsApi.updateStatus(status.id, { hidden: !status.hidden })
              )
            }
          >
            {status.hidden ? (
              <EyeSlashIcon className="size-icon-xs" />
            ) : (
              <EyeIcon className="size-icon-xs" />
            )}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label="Move up"
            disabled={index === 0}
            onClick={() => swap(status, board.statuses[index - 1])}
          >
            <ArrowUpIcon className="size-icon-xs" />
          </Button>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label="Move down"
            disabled={index === board.statuses.length - 1}
            onClick={() => swap(status, board.statuses[index + 1])}
          >
            <ArrowDownIcon className="size-icon-xs" />
          </Button>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            aria-label="Delete status"
            disabled={board.statuses.length <= 1}
            title="Tickets move to the first remaining status"
            onClick={() => void run(projectsApi.deleteStatus(status.id))}
          >
            <TrashIcon className="size-icon-xs" />
          </Button>
        </div>
      ))}
      <form
        className="flex gap-half"
        onSubmit={(e) => {
          e.preventDefault();
          const name = newName.trim();
          if (!name) return;
          void run(
            projectsApi
              .createStatus(projectId, { name, color: '220 9% 46%' })
              .then(() => setNewName(''))
          );
        }}
      >
        <Input
          aria-label="New status"
          placeholder="New status"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
        />
        <Button type="submit" size="sm" variant="outline">
          Add
        </Button>
      </form>

      {board.tags.length > 0 && (
        <>
          <Label className="mt-base">Tags</Label>
          <div className="flex flex-wrap gap-half">
            {board.tags.map((tag) => (
              <span
                key={tag.id}
                className="flex items-center gap-half rounded-sm bg-panel px-base py-half text-sm"
              >
                <span
                  className="h-2 w-2 rounded-full"
                  style={{ backgroundColor: `hsl(${tag.color})` }}
                />
                {tag.name}
                <button
                  type="button"
                  aria-label={`Delete tag ${tag.name}`}
                  className="text-low hover:text-error"
                  onClick={() => void run(projectsApi.deleteTag(tag.id))}
                >
                  <XIcon className="size-icon-xs" />
                </button>
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

const ProjectSettingsDialogImpl = create<ProjectSettingsDialogProps>(
  ({ project }) => {
    const modal = useModal();
    const isEdit = !!project;
    const [name, setName] = useState(project?.name ?? '');
    const [prefix, setPrefix] = useState(project?.id_prefix ?? 'KAN');
    const [externalKey, setExternalKey] = useState(
      project?.external_id_key ?? ''
    );
    const [externalPattern, setExternalPattern] = useState(
      project?.external_id_pattern ?? ''
    );
    const [historyAlwaysAllow, setHistoryAlwaysAllow] = useState(
      project?.settings.history_always_allow ?? false
    );
    const [remoteCode, setRemoteCode] = useState(
      project?.settings.allow_remote_code_embedding ?? false
    );
    const [searchExcludes, setSearchExcludes] = useState(
      project?.settings.search_exclude_globs.join('\n') ?? ''
    );
    const [testPaths, setTestPaths] = useState(
      project?.settings.test_paths.join('\n') ?? ''
    );
    const [sandboxDomains, setSandboxDomains] = useState(
      project?.settings.sandbox_allowed_domains.join('\n') ?? ''
    );
    const [sandboxDockerfile, setSandboxDockerfile] = useState(
      project?.settings.sandbox_dockerfile ?? ''
    );
    const [repos, setRepos] = useState<Repo[]>([]);
    const [saving, setSaving] = useState(false);
    const [confirmDelete, setConfirmDelete] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
      if (project) {
        projectsApi
          .listRepos(project.id)
          .then(setRepos)
          .catch(() => setRepos([]));
      }
    }, [project]);

    const close = (result: Project | null) => {
      modal.resolve(result);
      modal.hide();
    };

    const linkRepo = async (repoId: string) => {
      if (!project) {
        const repo = (await repoApi.list()).find((r) => r.id === repoId);
        if (repo) setRepos((prev) => [...prev, repo]);
        return;
      }
      try {
        setRepos(await projectsApi.linkRepo(project.id, repoId));
      } catch (err) {
        setError(getErrorMessage(err) || 'Failed to link repository');
      }
    };

    const unlinkRepo = async (repoId: string) => {
      if (!project) {
        setRepos((prev) => prev.filter((r) => r.id !== repoId));
        return;
      }
      try {
        setRepos(await projectsApi.unlinkRepo(project.id, repoId));
      } catch (err) {
        setError(getErrorMessage(err) || 'Failed to unlink repository');
      }
    };

    const save = async () => {
      if (!name.trim()) {
        setError('A project name is required.');
        return;
      }
      setSaving(true);
      setError(null);
      try {
        const saved = project
          ? await projectsApi.update(project.id, {
              name,
              id_prefix: prefix,
              external_id_key: externalKey.trim() || null,
              external_id_pattern: externalPattern.trim() || null,
              settings: {
                history_always_allow: historyAlwaysAllow,
                allow_remote_code_embedding: remoteCode,
                search_exclude_globs: searchExcludes
                  .split('\n')
                  .map((g) => g.trim())
                  .filter(Boolean),
                test_paths: testPaths
                  .split('\n')
                  .map((g) => g.trim())
                  .filter(Boolean),
                sandbox_allowed_domains: sandboxDomains
                  .split('\n')
                  .map((d) => d.trim())
                  .filter(Boolean),
                sandbox_dockerfile: sandboxDockerfile.trim() || null,
              },
            })
          : await projectsApi.create({
              name,
              id_prefix: prefix,
              repo_ids: repos.map((r) => r.id),
            });
        close(saved);
      } catch (err) {
        setError(getErrorMessage(err) || 'Failed to save project');
      } finally {
        setSaving(false);
      }
    };

    const deleteProject = async () => {
      if (!project) return;
      try {
        await projectsApi.delete(project.id);
        close(null);
      } catch (err) {
        setError(getErrorMessage(err) || 'Failed to delete project');
      }
    };

    return (
      <Dialog
        open={modal.visible}
        onOpenChange={(open) => {
          if (!open) close(null);
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-[640px]">
          <DialogHeader>
            <DialogTitle>
              {isEdit ? 'Project settings' : 'New project'}
            </DialogTitle>
          </DialogHeader>
          <form
            className="flex flex-col gap-double py-base"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <div className="grid grid-cols-[1fr_140px] gap-base">
              <div className="flex flex-col gap-half">
                <Label htmlFor="project-name">Name</Label>
                <Input
                  id="project-name"
                  autoFocus={!isEdit}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                />
              </div>
              <div className="flex flex-col gap-half">
                <Label htmlFor="project-prefix">Ticket prefix</Label>
                <Input
                  id="project-prefix"
                  value={prefix}
                  maxLength={10}
                  onChange={(e) => setPrefix(e.target.value.toUpperCase())}
                />
              </div>
            </div>

            {isEdit && (
              <div className="grid grid-cols-2 gap-base">
                <div className="flex flex-col gap-half">
                  <Label htmlFor="project-external-key">External ID key</Label>
                  <Input
                    id="project-external-key"
                    placeholder="e.g. PROJEKTNAME-"
                    value={externalKey}
                    onChange={(e) => setExternalKey(e.target.value)}
                  />
                </div>
                <div className="flex flex-col gap-half">
                  <Label htmlFor="project-external-pattern">
                    External ID pattern (regex)
                  </Label>
                  <Input
                    id="project-external-pattern"
                    placeholder="\[?[A-Z][A-Z0-9]*-\d+\]?"
                    value={externalPattern}
                    onChange={(e) => setExternalPattern(e.target.value)}
                  />
                </div>
                <label className="col-span-2 flex items-center gap-base text-sm">
                  <input
                    type="checkbox"
                    checked={historyAlwaysAllow}
                    onChange={(e) => setHistoryAlwaysAllow(e.target.checked)}
                  />
                  Always allow git-history analysis for this project’s tickets
                </label>
                <label className="col-span-2 flex items-center gap-base text-sm">
                  <input
                    type="checkbox"
                    checked={remoteCode}
                    onChange={(e) => setRemoteCode(e.target.checked)}
                  />
                  Allow sending this project’s code to a remote embedder
                </label>
                <div className="col-span-2 flex flex-col gap-half">
                  <Label htmlFor="project-search-excludes">
                    Files the code index skips (globs, e.g. generated code)
                  </Label>
                  <textarea
                    id="project-search-excludes"
                    className="min-h-12 rounded-sm border border-border bg-secondary px-base py-half font-mono text-sm"
                    placeholder={'shared/types.ts\n**/*.generated.ts'}
                    value={searchExcludes}
                    onChange={(e) => setSearchExcludes(e.target.value)}
                  />
                </div>
                <div className="col-span-2 flex flex-col gap-half">
                  <Label htmlFor="project-test-paths">
                    Test paths: the only files the tester may add or change
                    (globs per repo; empty uses the defaults)
                  </Label>
                  <textarea
                    id="project-test-paths"
                    data-testid="project-test-paths"
                    className="min-h-12 rounded-sm border border-border bg-secondary px-base py-half font-mono text-sm"
                    placeholder={
                      '**/tests/**\n**/__tests__/**\n**/*.test.*\n**/*_test.*'
                    }
                    value={testPaths}
                    onChange={(e) => setTestPaths(e.target.value)}
                  />
                </div>
                <div className="col-span-2 flex flex-col gap-half">
                  <Label htmlFor="project-sandbox-domains">
                    Extra hosts the sandbox may reach
                  </Label>
                  <textarea
                    id="project-sandbox-domains"
                    className="min-h-16 rounded-sm border border-border bg-secondary px-base py-half font-mono text-sm"
                    placeholder={'api.example.com\n*.internal.example.org'}
                    value={sandboxDomains}
                    onChange={(e) => setSandboxDomains(e.target.value)}
                  />
                </div>
                <div className="col-span-2 flex flex-col gap-half">
                  <Label htmlFor="project-sandbox-dockerfile">
                    Sandbox Dockerfile (optional)
                  </Label>
                  <Input
                    id="project-sandbox-dockerfile"
                    placeholder="/path/to/Dockerfile (FROM ${KANBAIN_BASE_IMAGE})"
                    value={sandboxDockerfile}
                    onChange={(e) => setSandboxDockerfile(e.target.value)}
                  />
                </div>
              </div>
            )}

            <div className="flex flex-col gap-half">
              <Label>Repositories</Label>
              <RepoPicker
                linked={repos}
                onLink={(id) => void linkRepo(id)}
                onUnlink={(id) => void unlinkRepo(id)}
              />
            </div>

            {project && (
              <div className="flex flex-col gap-half">
                <Label>Statuses</Label>
                <p className="text-xs text-low">
                  The pipeline role tells agents what a column means, so columns
                  can be renamed freely.
                </p>
                <StatusEditor projectId={project.id} onError={setError} />
              </div>
            )}

            {error && <p className="text-sm text-error">{error}</p>}

            <DialogFooter className="sm:justify-between">
              <div>
                {project &&
                  (confirmDelete ? (
                    <Button
                      type="button"
                      variant="destructive"
                      onClick={() => void deleteProject()}
                    >
                      Delete project and all tickets
                    </Button>
                  ) : (
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => setConfirmDelete(true)}
                    >
                      Delete project
                    </Button>
                  ))}
              </div>
              <div className="flex gap-half">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => close(null)}
                >
                  Cancel
                </Button>
                <Button type="submit" disabled={saving}>
                  {isEdit ? 'Save' : 'Create project'}
                </Button>
              </div>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    );
  }
);

export const ProjectSettingsDialog = defineModal<
  ProjectSettingsDialogProps,
  Project | null
>(ProjectSettingsDialogImpl);
