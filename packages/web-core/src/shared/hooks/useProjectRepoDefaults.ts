import type { DraftWorkspaceRepo } from 'shared/types';
import { projectsApi } from '@/shared/lib/ticketsApi';

/**
 * Read a project's repositories as workspace defaults. The target branch is
 * left empty so the create screen picks each repo's default branch.
 * Returns null if the project has no repositories.
 */
export async function getProjectRepoDefaults(
  projectId: string
): Promise<DraftWorkspaceRepo[] | null> {
  try {
    const repos = await projectsApi.listRepos(projectId);
    if (repos.length === 0) return null;
    return repos.map((repo) => ({ repo_id: repo.id, target_branch: '' }));
  } catch (error) {
    console.error('[useProjectRepoDefaults] Failed to read defaults:', error);
    return null;
  }
}

/**
 * Link the repositories chosen for a ticket's workspace to its project, so
 * the next workspace for that project starts with them.
 */
export async function saveProjectRepoDefaults(
  projectId: string,
  repos: DraftWorkspaceRepo[]
): Promise<void> {
  const linked = new Set(
    (await projectsApi.listRepos(projectId)).map((repo) => repo.id)
  );
  for (const repo of repos) {
    if (!linked.has(repo.repo_id)) {
      await projectsApi.linkRepo(projectId, repo.repo_id);
    }
  }
}

/**
 * Read project repo defaults and filter out repos that no longer exist.
 * Returns an empty array if the project has no usable repositories.
 */
export async function getValidProjectRepoDefaults(
  projectId: string,
  availableRepoIds: Set<string>
): Promise<DraftWorkspaceRepo[]> {
  const defaults = await getProjectRepoDefaults(projectId);
  if (!defaults) {
    return [];
  }
  return defaults.filter((repo) => availableRepoIds.has(repo.repo_id));
}
