// Import all necessary types from shared types

import {
  ApprovalStatus,
  ApiResponse,
  Config,
  CreateFollowUpAttempt,
  ResetProcessRequest,
  EditorType,
  CreatePrApiRequest,
  CreateTag,
  DirectoryListResponse,
  DirectoryEntry,
  ExecutionProcess,
  ExecutionProcessEvent,
  ExecutionProcessRepoState,
  SandboxStatus,
  SetModelCredential,
  AgentDefinition,
  AgentLibraryMeta,
  AgentSummary,
  AgentTestRun,
  ClaudeLoginState,
  CreateSkill,
  ImportSummary,
  SkillFileInfo,
  SkillSummary,
  StyleBlock,
  StyleSummary,
  GitBranch,
  Repo,
  RepoWithTargetBranch,
  UpdateRepo,
  SearchMode,
  SearchResult,
  Tag,
  TagSearchParams,
  UpdateTag,
  UserSystemInfo,
  McpServerQuery,
  UpdateMcpServersBody,
  GetMcpServerResponse,
  AttachmentResponse,
  GitOperationError,
  ApprovalResponse,
  RebaseWorkspaceRequest,
  ChangeTargetBranchRequest,
  ChangeTargetBranchResponse,
  RenameBranchRequest,
  RenameBranchResponse,
  CheckEditorAvailabilityResponse,
  AvailabilityInfo,
  BaseCodingAgent,
  ExecutorConfig,
  DraftFollowUpData,
  AgentPresetOptionsQuery,
  GhCliSetupError,
  RunScriptError,
  OpenEditorResponse,
  OpenEditorRequest,
  PrError,
  Scratch,
  ScratchType,
  CreateScratch,
  UpdateScratch,
  PushError,
  QueueStatus,
  PrCommentsResponse,
  MergeWorkspaceRequest,
  PushWorkspaceRequest,
  RepoBranchStatus,
  AbortConflictsRequest,
  ContinueRebaseRequest,
  Session,
  Workspace,
  StartReviewRequest,
  ReviewError,
  GitRemote,
  ListPrsError,
  PullRequestDetail,
  AttachExistingPrRequest,
  AttachPrResponse,
  CreateWorkspaceFromPrBody,
  CreateWorkspaceFromPrResponse,
  CreateFromPrError,
  CreateAndStartWorkspaceRequest,
  CreateAndStartWorkspaceResponse,
} from 'shared/types';
import type { WorkspaceWithSession } from '@/shared/types/attempt';
import { createWorkspaceWithSession } from '@/shared/types/attempt';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';

export class ApiError<E = unknown> extends Error {
  public status?: number;
  public error_data?: E;

  constructor(
    message: string,
    public statusCode?: number,
    public response?: Response,
    error_data?: E
  ) {
    super(message);
    this.name = 'ApiError';
    this.status = statusCode;
    this.error_data = error_data;
  }
}

const makeRequest = async (url: string, options: RequestInit = {}) => {
  const headers = new Headers(options.headers ?? {});
  if (!headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  return makeLocalApiRequest(url, {
    ...options,
    headers,
  });
};

export type Ok<T> = { success: true; data: T };
export type Err<E> = { success: false; error: E | undefined; message?: string };

// Result type for endpoints that need typed errors
export type Result<T, E> = Ok<T> | Err<E>;

// Special handler for Result-returning endpoints
const handleApiResponseAsResult = async <T, E>(
  response: Response
): Promise<Result<T, E>> => {
  if (!response.ok) {
    // HTTP error - no structured error data
    let errorMessage = `Request failed with status ${response.status}`;

    try {
      const errorData = await response.json();
      if (errorData.message) {
        errorMessage = errorData.message;
      }
    } catch {
      errorMessage = response.statusText || errorMessage;
    }

    return {
      success: false,
      error: undefined,
      message: errorMessage,
    };
  }

  const result: ApiResponse<T, E> = await response.json();

  if (!result.success) {
    return {
      success: false,
      error: result.error_data || undefined,
      message: result.message || undefined,
    };
  }

  return { success: true, data: result.data as T };
};

export const handleApiResponse = async <T, E = T>(
  response: Response
): Promise<T> => {
  if (!response.ok) {
    let errorMessage = `Request failed with status ${response.status}`;

    try {
      const errorData = await response.json();
      if (errorData.message) {
        errorMessage = errorData.message;
      }
    } catch {
      // Fallback to status text if JSON parsing fails
      errorMessage = response.statusText || errorMessage;
    }

    console.error('[API Error]', {
      message: errorMessage,
      status: response.status,
      response,
      endpoint: response.url,
      timestamp: new Date().toISOString(),
    });
    throw new ApiError<E>(errorMessage, response.status, response);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const result: ApiResponse<T, E> = await response.json();

  if (!result.success) {
    // Check for error_data first (structured errors), then fall back to message
    if (result.error_data) {
      console.error('[API Error with data]', {
        error_data: result.error_data,
        message: result.message,
        status: response.status,
        response,
        endpoint: response.url,
        timestamp: new Date().toISOString(),
      });
      // Throw a properly typed error with the error data
      throw new ApiError<E>(
        result.message || 'API request failed',
        response.status,
        response,
        result.error_data
      );
    }

    console.error('[API Error]', {
      message: result.message || 'API request failed',
      status: response.status,
      response,
      endpoint: response.url,
      timestamp: new Date().toISOString(),
    });
    throw new ApiError<E>(
      result.message || 'API request failed',
      response.status,
      response
    );
  }

  return result.data as T;
};

// Sessions API
export const sessionsApi = {
  getByWorkspace: async (workspaceId: string): Promise<Session[]> => {
    const response = await makeRequest(
      `/api/sessions?workspace_id=${workspaceId}`
    );
    return handleApiResponse<Session[]>(response);
  },

  getById: async (sessionId: string): Promise<Session> => {
    const response = await makeRequest(`/api/sessions/${sessionId}`);
    return handleApiResponse<Session>(response);
  },

  create: async (data: {
    workspace_id: string;
    executor?: string;
    name?: string;
  }): Promise<Session> => {
    const response = await makeRequest('/api/sessions', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Session>(response);
  },

  followUp: async (
    sessionId: string,
    data: CreateFollowUpAttempt
  ): Promise<ExecutionProcess> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/follow-up`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<ExecutionProcess>(response);
  },

  startReview: async (
    sessionId: string,
    data: StartReviewRequest
  ): Promise<ExecutionProcess> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/review`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<ExecutionProcess, ReviewError>(response);
  },

  reset: async (
    sessionId: string,
    data: ResetProcessRequest
  ): Promise<void> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/reset`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<void>(response);
  },

  runSetupScript: async (
    sessionId: string
  ): Promise<Result<ExecutionProcess, RunScriptError>> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/setup`, {
      method: 'POST',
    });
    return handleApiResponseAsResult<ExecutionProcess, RunScriptError>(
      response
    );
  },

  update: async (
    sessionId: string,
    data: { name?: string }
  ): Promise<Session> => {
    const response = await makeRequest(`/api/sessions/${sessionId}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Session>(response);
  },
};

// Workspace APIs
export const workspacesApi = {
  createAndStart: async (
    data: CreateAndStartWorkspaceRequest
  ): Promise<CreateAndStartWorkspaceResponse> => {
    const response = await makeRequest(`/api/workspaces/start`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<CreateAndStartWorkspaceResponse>(response);
  },

  /** Get all workspaces across all tasks (newest first) */
  getAllWorkspaces: async (): Promise<Workspace[]> => {
    const response = await makeRequest('/api/workspaces');
    return handleApiResponse<Workspace[]>(response);
  },

  get: async (workspaceId: string): Promise<Workspace> => {
    const response = await makeRequest(`/api/workspaces/${workspaceId}`);
    return handleApiResponse<Workspace>(response);
  },

  update: async (
    workspaceId: string,
    data: { archived?: boolean; pinned?: boolean; name?: string }
  ): Promise<Workspace> => {
    const response = await makeRequest(`/api/workspaces/${workspaceId}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Workspace>(response);
  },

  /** Get workspace with latest session */
  getWithSession: async (
    workspaceId: string
  ): Promise<WorkspaceWithSession> => {
    const [workspace, sessions] = await Promise.all([
      workspacesApi.get(workspaceId),
      sessionsApi.getByWorkspace(workspaceId),
    ]);
    return createWorkspaceWithSession(workspace, sessions[0]);
  },

  stop: async (workspaceId: string): Promise<void> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/execution/stop`,
      {
        method: 'POST',
      }
    );
    return handleApiResponse<void>(response);
  },

  delete: async (
    workspaceId: string,
    deleteBranches?: boolean
  ): Promise<void> => {
    const params = new URLSearchParams();
    if (deleteBranches) {
      params.set('delete_branches', 'true');
    }
    const queryString = params.toString();
    const url = `/api/workspaces/${workspaceId}${queryString ? `?${queryString}` : ''}`;
    const response = await makeRequest(url, {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },

  openEditor: async (
    workspaceId: string,
    data: OpenEditorRequest
  ): Promise<OpenEditorResponse> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/integration/editor/open`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponse<OpenEditorResponse>(response);
  },

  getEditorPath: async (
    workspaceId: string
  ): Promise<{ workspace_path: string }> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/integration/editor/path`
    );
    return handleApiResponse<{ workspace_path: string }>(response);
  },

  getBranchStatus: async (workspaceId: string): Promise<RepoBranchStatus[]> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/status`
    );
    return handleApiResponse<RepoBranchStatus[]>(response);
  },

  getRepos: async (workspaceId: string): Promise<RepoWithTargetBranch[]> => {
    const response = await makeRequest(`/api/workspaces/${workspaceId}/repos`);
    return handleApiResponse<RepoWithTargetBranch[]>(response);
  },

  getFirstUserMessage: async (workspaceId: string): Promise<string | null> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/messages/first`
    );
    return handleApiResponse<string | null>(response);
  },

  merge: async (
    workspaceId: string,
    data: MergeWorkspaceRequest
  ): Promise<void> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/merge`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponse<void>(response);
  },

  push: async (
    workspaceId: string,
    data: PushWorkspaceRequest
  ): Promise<Result<void, PushError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/push`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponseAsResult<void, PushError>(response);
  },

  forcePush: async (
    workspaceId: string,
    data: PushWorkspaceRequest
  ): Promise<Result<void, PushError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/push/force`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponseAsResult<void, PushError>(response);
  },

  rebase: async (
    workspaceId: string,
    data: RebaseWorkspaceRequest
  ): Promise<Result<void, GitOperationError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/rebase`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponseAsResult<void, GitOperationError>(response);
  },

  change_target_branch: async (
    workspaceId: string,
    data: ChangeTargetBranchRequest
  ): Promise<ChangeTargetBranchResponse> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/target-branch`,
      {
        method: 'PUT',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponse<ChangeTargetBranchResponse>(response);
  },

  renameBranch: async (
    workspaceId: string,
    newBranchName: string
  ): Promise<RenameBranchResponse> => {
    const payload: RenameBranchRequest = {
      new_branch_name: newBranchName,
    };
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/branch`,
      {
        method: 'PUT',
        body: JSON.stringify(payload),
      }
    );
    return handleApiResponse<RenameBranchResponse>(response);
  },

  abortConflicts: async (
    workspaceId: string,
    data: AbortConflictsRequest
  ): Promise<void> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/conflicts/abort`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponse<void>(response);
  },

  continueRebase: async (
    workspaceId: string,
    data: ContinueRebaseRequest
  ): Promise<void> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/git/rebase/continue`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponse<void>(response);
  },

  createPR: async (
    workspaceId: string,
    data: CreatePrApiRequest
  ): Promise<Result<string, PrError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/pull-requests`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponseAsResult<string, PrError>(response);
  },

  /** Try to auto-attach a PR by matching the workspace branch */
  attachPr: async (
    workspaceId: string,
    data: AttachExistingPrRequest
  ): Promise<Result<AttachPrResponse, PrError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/pull-requests/attach`,
      {
        method: 'POST',
        body: JSON.stringify(data),
      }
    );
    return handleApiResponseAsResult<AttachPrResponse, PrError>(response);
  },

  startDevServer: async (workspaceId: string): Promise<ExecutionProcess[]> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/execution/dev-server/start`,
      {
        method: 'POST',
      }
    );
    return handleApiResponse<ExecutionProcess[]>(response);
  },

  setupGhCli: async (workspaceId: string): Promise<ExecutionProcess> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/integration/github/cli/setup`,
      {
        method: 'POST',
      }
    );
    return handleApiResponse<ExecutionProcess, GhCliSetupError>(response);
  },

  runSetupScript: async (
    workspaceId: string
  ): Promise<Result<ExecutionProcess, RunScriptError>> => {
    const sessions = await sessionsApi.getByWorkspace(workspaceId);
    const session =
      sessions[0] ??
      (await sessionsApi.create({
        workspace_id: workspaceId,
      }));

    return sessionsApi.runSetupScript(session.id);
  },

  runCleanupScript: async (
    workspaceId: string
  ): Promise<Result<ExecutionProcess, RunScriptError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/execution/cleanup`,
      {
        method: 'POST',
      }
    );
    return handleApiResponseAsResult<ExecutionProcess, RunScriptError>(
      response
    );
  },

  runArchiveScript: async (
    workspaceId: string
  ): Promise<Result<ExecutionProcess, RunScriptError>> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/execution/archive`,
      {
        method: 'POST',
      }
    );
    return handleApiResponseAsResult<ExecutionProcess, RunScriptError>(
      response
    );
  },

  getPrComments: async (
    workspaceId: string,
    repoId: string
  ): Promise<PrCommentsResponse> => {
    const response = await makeRequest(
      `/api/workspaces/${workspaceId}/pull-requests/comments?repo_id=${encodeURIComponent(repoId)}`
    );
    return handleApiResponse<PrCommentsResponse>(response);
  },

  /** Mark all coding agent turns for a workspace as seen */
  markSeen: async (workspaceId: string): Promise<void> => {
    const response = await makeRequest(`/api/workspaces/${workspaceId}/seen`, {
      method: 'PUT',
    });
    return handleApiResponse<void>(response);
  },

  /** Create a workspace directly from a pull request */
  createFromPr: async (
    data: CreateWorkspaceFromPrBody
  ): Promise<Result<CreateWorkspaceFromPrResponse, CreateFromPrError>> => {
    const response = await makeRequest('/api/workspaces/from-pr', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponseAsResult<
      CreateWorkspaceFromPrResponse,
      CreateFromPrError
    >(response);
  },
};

// Execution Process APIs
export const executionProcessesApi = {
  getDetails: async (processId: string): Promise<ExecutionProcess> => {
    const response = await makeRequest(`/api/execution-processes/${processId}`);
    return handleApiResponse<ExecutionProcess>(response);
  },

  getRepoStates: async (
    processId: string
  ): Promise<ExecutionProcessRepoState[]> => {
    const response = await makeRequest(
      `/api/execution-processes/${processId}/repo-states`
    );
    return handleApiResponse<ExecutionProcessRepoState[]>(response);
  },

  getEvents: async (processId: string): Promise<ExecutionProcessEvent[]> => {
    const response = await makeRequest(
      `/api/execution-processes/${processId}/events`
    );
    return handleApiResponse<ExecutionProcessEvent[]>(response);
  },

  stopExecutionProcess: async (processId: string): Promise<void> => {
    const response = await makeRequest(
      `/api/execution-processes/${processId}/stop`,
      {
        method: 'POST',
      }
    );
    return handleApiResponse<void>(response);
  },
};

// Docker sandbox and model credentials
export const sandboxApi = {
  getStatus: async (): Promise<SandboxStatus> => {
    const response = await makeRequest('/api/sandbox/status', {
      cache: 'no-store',
    });
    return handleApiResponse<SandboxStatus>(response);
  },

  setCredential: async (data: SetModelCredential): Promise<SandboxStatus> => {
    const response = await makeRequest('/api/sandbox/credentials', {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<SandboxStatus>(response);
  },

  getClaudeLogin: async (): Promise<ClaudeLoginState> => {
    const response = await makeRequest('/api/sandbox/claude-login', {
      cache: 'no-store',
    });
    return handleApiResponse<ClaudeLoginState>(response);
  },

  startClaudeLogin: async (): Promise<ClaudeLoginState> => {
    const response = await makeRequest('/api/sandbox/claude-login', {
      method: 'POST',
    });
    return handleApiResponse<ClaudeLoginState>(response);
  },

  submitClaudeLoginCode: async (code: string): Promise<ClaudeLoginState> => {
    const response = await makeRequest('/api/sandbox/claude-login/code', {
      method: 'POST',
      body: JSON.stringify({ code }),
    });
    return handleApiResponse<ClaudeLoginState>(response);
  },

  cancelClaudeLogin: async (): Promise<ClaudeLoginState> => {
    const response = await makeRequest('/api/sandbox/claude-login', {
      method: 'DELETE',
    });
    return handleApiResponse<ClaudeLoginState>(response);
  },

  disconnectClaudeAccount: async (): Promise<SandboxStatus> => {
    const response = await makeRequest('/api/sandbox/claude-account', {
      method: 'DELETE',
    });
    return handleApiResponse<SandboxStatus>(response);
  },
};

const libraryPath = (path: string) => `/api/agent-library${path}`;
const skillFilePath = (skill: string, path: string) =>
  libraryPath(
    `/skills/${encodeURIComponent(skill)}/file?path=${encodeURIComponent(path)}`
  );

// Agent library APIs (agents, style blocks, skills)
export const agentLibraryApi = {
  meta: async (): Promise<AgentLibraryMeta> => {
    const response = await makeRequest(libraryPath('/meta'));
    return handleApiResponse<AgentLibraryMeta>(response);
  },

  listAgents: async (): Promise<AgentSummary[]> => {
    const response = await makeRequest(libraryPath('/agents'), {
      cache: 'no-store',
    });
    return handleApiResponse<AgentSummary[]>(response);
  },

  createAgent: async (agent: AgentDefinition): Promise<AgentDefinition> => {
    const response = await makeRequest(libraryPath('/agents'), {
      method: 'POST',
      body: JSON.stringify(agent),
    });
    return handleApiResponse<AgentDefinition>(response);
  },

  updateAgent: async (
    name: string,
    agent: AgentDefinition
  ): Promise<AgentDefinition> => {
    const response = await makeRequest(
      libraryPath(`/agents/${encodeURIComponent(name)}`),
      { method: 'PUT', body: JSON.stringify(agent) }
    );
    return handleApiResponse<AgentDefinition>(response);
  },

  deleteAgent: async (name: string): Promise<void> => {
    const response = await makeRequest(
      libraryPath(`/agents/${encodeURIComponent(name)}`),
      { method: 'DELETE' }
    );
    return handleApiResponse<void>(response);
  },

  testRun: async (
    name: string,
    data: AgentTestRun
  ): Promise<CreateAndStartWorkspaceResponse> => {
    const response = await makeRequest(
      libraryPath(`/agents/${encodeURIComponent(name)}/test-run`),
      { method: 'POST', body: JSON.stringify(data) }
    );
    return handleApiResponse<CreateAndStartWorkspaceResponse>(response);
  },

  listStyles: async (): Promise<StyleSummary[]> => {
    const response = await makeRequest(libraryPath('/styles'), {
      cache: 'no-store',
    });
    return handleApiResponse<StyleSummary[]>(response);
  },

  createStyle: async (style: StyleBlock): Promise<StyleBlock> => {
    const response = await makeRequest(libraryPath('/styles'), {
      method: 'POST',
      body: JSON.stringify(style),
    });
    return handleApiResponse<StyleBlock>(response);
  },

  updateStyle: async (name: string, style: StyleBlock): Promise<StyleBlock> => {
    const response = await makeRequest(
      libraryPath(`/styles/${encodeURIComponent(name)}`),
      { method: 'PUT', body: JSON.stringify(style) }
    );
    return handleApiResponse<StyleBlock>(response);
  },

  deleteStyle: async (name: string): Promise<void> => {
    const response = await makeRequest(
      libraryPath(`/styles/${encodeURIComponent(name)}`),
      { method: 'DELETE' }
    );
    return handleApiResponse<void>(response);
  },

  listSkills: async (): Promise<SkillSummary[]> => {
    const response = await makeRequest(libraryPath('/skills'), {
      cache: 'no-store',
    });
    return handleApiResponse<SkillSummary[]>(response);
  },

  createSkill: async (data: CreateSkill): Promise<void> => {
    const response = await makeRequest(libraryPath('/skills'), {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<void>(response);
  },

  deleteSkill: async (name: string): Promise<void> => {
    const response = await makeRequest(
      libraryPath(`/skills/${encodeURIComponent(name)}`),
      { method: 'DELETE' }
    );
    return handleApiResponse<void>(response);
  },

  listSkillFiles: async (name: string): Promise<SkillFileInfo[]> => {
    const response = await makeRequest(
      libraryPath(`/skills/${encodeURIComponent(name)}/files`),
      { cache: 'no-store' }
    );
    return handleApiResponse<SkillFileInfo[]>(response);
  },

  readSkillFile: async (name: string, path: string): Promise<string> => {
    const response = await makeRequest(skillFilePath(name, path), {
      cache: 'no-store',
    });
    if (!response.ok) {
      throw new ApiError(`Failed to read ${path}`, response.status, response);
    }
    return response.text();
  },

  writeSkillFile: async (
    name: string,
    path: string,
    contents: string
  ): Promise<void> => {
    const response = await makeRequest(skillFilePath(name, path), {
      method: 'PUT',
      body: contents,
      headers: { 'Content-Type': 'text/plain; charset=utf-8' },
    });
    return handleApiResponse<void>(response);
  },

  deleteSkillFile: async (name: string, path: string): Promise<void> => {
    const response = await makeRequest(skillFilePath(name, path), {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },

  /** Import zips, or files named by their path in a chosen folder. */
  importFiles: async (
    files: { file: File; path: string }[]
  ): Promise<ImportSummary> => {
    const formData = new FormData();
    for (const { file, path } of files) {
      formData.append('file', file, path);
    }
    const response = await makeLocalApiRequest(libraryPath('/import'), {
      method: 'POST',
      body: formData,
      credentials: 'include',
    });
    return handleApiResponse<ImportSummary>(response);
  },

  exportUrl: (agents?: string[]): string =>
    agents?.length
      ? libraryPath(`/export?agents=${encodeURIComponent(agents.join(','))}`)
      : libraryPath('/export'),
};

// File System APIs
export const fileSystemApi = {
  list: async (path?: string): Promise<DirectoryListResponse> => {
    const queryParam = path ? `?path=${encodeURIComponent(path)}` : '';
    const response = await makeRequest(
      `/api/filesystem/directory${queryParam}`
    );
    return handleApiResponse<DirectoryListResponse>(response);
  },

  listGitRepos: async (path?: string): Promise<DirectoryEntry[]> => {
    const queryParam = path ? `?path=${encodeURIComponent(path)}` : '';
    const response = await makeRequest(
      `/api/filesystem/git-repos${queryParam}`
    );
    return handleApiResponse<DirectoryEntry[]>(response);
  },
};

// Repo APIs
export const repoApi = {
  list: async (): Promise<Repo[]> => {
    const response = await makeRequest('/api/repos');
    return handleApiResponse<Repo[]>(response);
  },

  listRecent: async (): Promise<Repo[]> => {
    const response = await makeRequest('/api/repos/recent');
    return handleApiResponse<Repo[]>(response);
  },

  getById: async (repoId: string): Promise<Repo> => {
    const response = await makeRequest(`/api/repos/${repoId}`);
    return handleApiResponse<Repo>(response);
  },

  update: async (repoId: string, data: UpdateRepo): Promise<Repo> => {
    const response = await makeRequest(`/api/repos/${repoId}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Repo>(response);
  },

  delete: async (repoId: string): Promise<void> => {
    const response = await makeRequest(`/api/repos/${repoId}`, {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },

  register: async (data: {
    path: string;
    display_name?: string;
  }): Promise<Repo> => {
    const response = await makeRequest('/api/repos', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Repo>(response);
  },

  getBranches: async (repoId: string): Promise<GitBranch[]> => {
    const response = await makeRequest(`/api/repos/${repoId}/branches`);
    return handleApiResponse<GitBranch[]>(response);
  },

  init: async (data: {
    parent_path: string;
    folder_name: string;
  }): Promise<Repo> => {
    const response = await makeRequest('/api/repos/init', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Repo>(response);
  },

  getBatch: async (ids: string[]): Promise<Repo[]> => {
    const response = await makeRequest('/api/repos/batch', {
      method: 'POST',
      body: JSON.stringify({ ids }),
    });
    return handleApiResponse<Repo[]>(response);
  },

  openEditor: async (
    repoId: string,
    data: OpenEditorRequest
  ): Promise<OpenEditorResponse> => {
    const response = await makeRequest(`/api/repos/${repoId}/open-editor`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<OpenEditorResponse>(response);
  },

  searchFiles: async (
    repoId: string,
    query: string,
    mode?: SearchMode,
    options?: RequestInit
  ): Promise<SearchResult[]> => {
    const modeParam = mode ? `&mode=${encodeURIComponent(mode)}` : '';
    const response = await makeRequest(
      `/api/repos/${repoId}/search?q=${encodeURIComponent(query)}${modeParam}`,
      options
    );
    return handleApiResponse<SearchResult[]>(response);
  },

  listOpenPrs: async (
    repoId: string,
    remoteName?: string
  ): Promise<Result<PullRequestDetail[], ListPrsError>> => {
    const params = remoteName
      ? `?remote=${encodeURIComponent(remoteName)}`
      : '';
    const response = await makeRequest(`/api/repos/${repoId}/prs${params}`);
    return handleApiResponseAsResult<PullRequestDetail[], ListPrsError>(
      response
    );
  },

  listRemotes: async (repoId: string): Promise<GitRemote[]> => {
    const response = await makeRequest(`/api/repos/${repoId}/remotes`);
    return handleApiResponse<GitRemote[]>(response);
  },
};

// Config APIs (backwards compatible)
export const configApi = {
  getConfig: async (): Promise<UserSystemInfo> => {
    const response = await makeRequest('/api/info', {
      cache: 'no-store',
    });
    return handleApiResponse<UserSystemInfo>(response);
  },
  saveConfig: async (config: Config): Promise<Config> => {
    const response = await makeRequest('/api/config', {
      method: 'PUT',
      body: JSON.stringify(config),
    });
    return handleApiResponse<Config>(response);
  },
  checkEditorAvailability: async (
    editorType: EditorType
  ): Promise<CheckEditorAvailabilityResponse> => {
    const response = await makeRequest(
      `/api/editors/check-availability?editor_type=${encodeURIComponent(editorType)}`
    );
    return handleApiResponse<CheckEditorAvailabilityResponse>(response);
  },
  checkAgentAvailability: async (
    agent: BaseCodingAgent
  ): Promise<AvailabilityInfo> => {
    const response = await makeRequest(
      `/api/agents/check-availability?executor=${encodeURIComponent(agent)}`
    );
    return handleApiResponse<AvailabilityInfo>(response);
  },
};

// Task Tags APIs (all tags are global)
export const tagsApi = {
  list: async (params?: TagSearchParams): Promise<Tag[]> => {
    const queryParam = params?.search
      ? `?search=${encodeURIComponent(params.search)}`
      : '';
    const response = await makeRequest(`/api/tags${queryParam}`);
    return handleApiResponse<Tag[]>(response);
  },

  create: async (data: CreateTag): Promise<Tag> => {
    const response = await makeRequest('/api/tags', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Tag>(response);
  },

  update: async (tagId: string, data: UpdateTag): Promise<Tag> => {
    const response = await makeRequest(`/api/tags/${tagId}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Tag>(response);
  },

  delete: async (tagId: string): Promise<void> => {
    const response = await makeRequest(`/api/tags/${tagId}`, {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },
};

// MCP Servers APIs
export const mcpServersApi = {
  load: async (query: McpServerQuery): Promise<GetMcpServerResponse> => {
    const params = new URLSearchParams(query);
    const response = await makeRequest(`/api/mcp-config?${params.toString()}`);
    return handleApiResponse<GetMcpServerResponse>(response);
  },
  save: async (
    query: McpServerQuery,
    data: UpdateMcpServersBody
  ): Promise<void> => {
    const params = new URLSearchParams(query);
    // params.set('profile', profile);
    const response = await makeRequest(`/api/mcp-config?${params.toString()}`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    if (!response.ok) {
      const errorData = await response.json();
      console.error('[API Error] Failed to save MCP servers', {
        message: errorData.message,
        status: response.status,
        response,
        timestamp: new Date().toISOString(),
      });
      throw new ApiError(
        errorData.message || 'Failed to save MCP servers',
        response.status,
        response
      );
    }
  },
};

// Profiles API
export const profilesApi = {
  load: async (): Promise<{ content: string; path: string }> => {
    const response = await makeRequest('/api/profiles');
    return handleApiResponse<{ content: string; path: string }>(response);
  },
  save: async (content: string): Promise<string> => {
    const response = await makeRequest('/api/profiles', {
      method: 'PUT',
      body: content,
      headers: {
        'Content-Type': 'application/json',
      },
    });
    return handleApiResponse<string>(response);
  },
};

// Workspace attachments API
export const attachmentsApi = {
  upload: async (attachment: File): Promise<AttachmentResponse> => {
    const formData = new FormData();
    formData.append('image', attachment);

    const response = await makeLocalApiRequest('/api/attachments/upload', {
      method: 'POST',
      body: formData,
      credentials: 'include',
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new ApiError(
        `Failed to upload attachment: ${errorText}`,
        response.status,
        response
      );
    }

    return handleApiResponse<AttachmentResponse>(response);
  },

  uploadForAttempt: async (
    workspaceId: string,
    sessionId: string,
    attachment: File
  ): Promise<AttachmentResponse> => {
    const formData = new FormData();
    formData.append('image', attachment);

    const response = await makeLocalApiRequest(
      `/api/workspaces/${workspaceId}/attachments/upload?session_id=${sessionId}`,
      {
        method: 'POST',
        body: formData,
        credentials: 'include',
      }
    );

    if (!response.ok) {
      const errorText = await response.text();
      throw new ApiError(
        `Failed to upload attachment: ${errorText}`,
        response.status,
        response
      );
    }

    return handleApiResponse<AttachmentResponse>(response);
  },

  delete: async (attachmentId: string): Promise<void> => {
    const response = await makeRequest(`/api/attachments/${attachmentId}`, {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },

  getAttachmentUrl: (attachmentId: string): string => {
    return `/api/attachments/${attachmentId}/file`;
  },
};

// Approval API
export const approvalsApi = {
  respond: async (
    approvalId: string,
    payload: ApprovalResponse,
    signal?: AbortSignal
  ): Promise<ApprovalStatus> => {
    const res = await makeRequest(`/api/approvals/${approvalId}/respond`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
      signal,
    });

    return handleApiResponse<ApprovalStatus>(res);
  },
};

// Scratch API
export const scratchApi = {
  create: async (
    scratchType: ScratchType,
    id: string,
    data: CreateScratch
  ): Promise<Scratch> => {
    const response = await makeRequest(`/api/scratch/${scratchType}/${id}`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<Scratch>(response);
  },

  get: async (scratchType: ScratchType, id: string): Promise<Scratch> => {
    const response = await makeRequest(`/api/scratch/${scratchType}/${id}`);
    return handleApiResponse<Scratch>(response);
  },

  update: async (
    scratchType: ScratchType,
    id: string,
    data: UpdateScratch
  ): Promise<void> => {
    const response = await makeRequest(`/api/scratch/${scratchType}/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return handleApiResponse<void>(response);
  },

  delete: async (scratchType: ScratchType, id: string): Promise<void> => {
    const response = await makeRequest(`/api/scratch/${scratchType}/${id}`, {
      method: 'DELETE',
    });
    return handleApiResponse<void>(response);
  },

  getStreamUrl: (scratchType: ScratchType, id: string): string =>
    `/api/scratch/${scratchType}/${id}/stream/ws`,
};

// Agents API
export const agentsApi = {
  getDiscoveredOptionsStreamUrl: (
    agent: BaseCodingAgent,
    opts?: { workspaceId?: string; sessionId?: string; repoId?: string }
  ): string => {
    const params = new URLSearchParams();
    params.set('executor', agent);
    if (opts?.workspaceId) params.set('workspace_id', opts.workspaceId);
    if (opts?.sessionId) params.set('session_id', opts.sessionId);
    if (opts?.repoId) params.set('repo_id', opts.repoId);

    return `/api/agents/discovered-options/ws?${params.toString()}`;
  },

  getPresetOptions: async (
    query: AgentPresetOptionsQuery
  ): Promise<ExecutorConfig> => {
    const params = new URLSearchParams();
    params.set('executor', query.executor);
    if (query.variant) params.set('variant', query.variant);
    const response = await makeRequest(
      `/api/agents/preset-options?${params.toString()}`
    );
    return handleApiResponse<ExecutorConfig>(response);
  },
};

// Queue API for session follow-up messages
export const queueApi = {
  /**
   * Queue a follow-up message to be executed when current execution finishes
   */
  queue: async (
    sessionId: string,
    data: DraftFollowUpData
  ): Promise<QueueStatus> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/queue`, {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return handleApiResponse<QueueStatus>(response);
  },

  /**
   * Cancel a queued follow-up message
   */
  cancel: async (sessionId: string): Promise<QueueStatus> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/queue`, {
      method: 'DELETE',
    });
    return handleApiResponse<QueueStatus>(response);
  },

  /**
   * Get the current queue status for a session
   */
  getStatus: async (sessionId: string): Promise<QueueStatus> => {
    const response = await makeRequest(`/api/sessions/${sessionId}/queue`);
    return handleApiResponse<QueueStatus>(response);
  },
};

// Search API (multi-repo file search)
export const searchApi = {
  searchFiles: async (
    repoIds: string[],
    query: string,
    mode?: SearchMode,
    options?: RequestInit
  ): Promise<SearchResult[]> => {
    const repoIdsParam = repoIds.join(',');
    const modeParam = mode ? `&mode=${encodeURIComponent(mode)}` : '';
    const response = await makeRequest(
      `/api/search?q=${encodeURIComponent(query)}&repo_ids=${encodeURIComponent(repoIdsParam)}${modeParam}`,
      options
    );
    return handleApiResponse<SearchResult[]>(response);
  },
};
