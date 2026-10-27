import type {
  Config,
  GetMcpServerResponse,
  GitBranch,
  McpServerQuery,
  Repo,
  UpdateMcpServersBody,
  UpdateRepo,
  UserSystemInfo,
} from 'shared/types';
import { handleApiResponse } from './api';
import { makeLocalApiRequest } from './localApiTransport';

export interface MachineClient {
  queryScopeKey: readonly ['machine', string];
  getConfig: () => Promise<UserSystemInfo>;
  saveConfig: (config: Config) => Promise<Config>;
  listRepos: () => Promise<Repo[]>;
  updateRepo: (repoId: string, data: UpdateRepo) => Promise<Repo>;
  deleteRepo: (repoId: string) => Promise<void>;
  registerRepo: (data: {
    path: string;
    display_name?: string;
  }) => Promise<Repo>;
  getRepoBranches: (repoId: string) => Promise<GitBranch[]>;
  loadProfiles: () => Promise<{ content: string; path: string }>;
  saveProfiles: (content: string) => Promise<string>;
  loadMcpServers: (query: McpServerQuery) => Promise<GetMcpServerResponse>;
  saveMcpServers: (
    query: McpServerQuery,
    data: UpdateMcpServersBody
  ) => Promise<void>;
}

async function makeMachineRequest(
  path: string,
  options: RequestInit = {}
): Promise<Response> {
  const headers = new Headers(options.headers ?? {});
  if (!headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  return makeLocalApiRequest(path, { ...options, headers });
}

export const localMachineClient: MachineClient = {
  queryScopeKey: ['machine', 'local'] as const,
  getConfig: async () =>
    handleApiResponse<UserSystemInfo>(
      await makeMachineRequest('/api/info', { cache: 'no-store' })
    ),
  saveConfig: async (config) =>
    handleApiResponse<Config>(
      await makeMachineRequest('/api/config', {
        method: 'PUT',
        body: JSON.stringify(config),
      })
    ),
  listRepos: async () =>
    handleApiResponse<Repo[]>(await makeMachineRequest('/api/repos')),
  updateRepo: async (repoId, data) =>
    handleApiResponse<Repo>(
      await makeMachineRequest(`/api/repos/${repoId}`, {
        method: 'PUT',
        body: JSON.stringify(data),
      })
    ),
  deleteRepo: async (repoId) =>
    handleApiResponse<void>(
      await makeMachineRequest(`/api/repos/${repoId}`, { method: 'DELETE' })
    ),
  registerRepo: async (data) =>
    handleApiResponse<Repo>(
      await makeMachineRequest('/api/repos', {
        method: 'POST',
        body: JSON.stringify(data),
      })
    ),
  getRepoBranches: async (repoId) =>
    handleApiResponse<GitBranch[]>(
      await makeMachineRequest(`/api/repos/${repoId}/branches`)
    ),
  loadProfiles: async () =>
    handleApiResponse<{ content: string; path: string }>(
      await makeMachineRequest('/api/profiles')
    ),
  saveProfiles: async (content) =>
    handleApiResponse<string>(
      await makeMachineRequest('/api/profiles', {
        method: 'PUT',
        body: content,
      })
    ),
  loadMcpServers: async (query) => {
    const params = new URLSearchParams(query);
    return handleApiResponse<GetMcpServerResponse>(
      await makeMachineRequest(`/api/mcp-config?${params.toString()}`)
    );
  },
  saveMcpServers: async (query, data) => {
    const params = new URLSearchParams(query);
    await handleApiResponse<void>(
      await makeMachineRequest(`/api/mcp-config?${params.toString()}`, {
        method: 'POST',
        body: JSON.stringify(data),
      })
    );
  },
};
