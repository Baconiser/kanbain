import type {
  AcceptIntake,
  AttachmentResponse,
  AttachmentText,
  BulkUpdateIssueItem,
  CreateIssue,
  CreateIssueComment,
  CreateIssueRelationship,
  CreateProject,
  CreateProjectStatus,
  CreateProjectTag,
  Issue,
  IssueComment,
  IssueDescription,
  IssueEvent,
  IssueRelationship,
  IntakeView,
  IssueTag,
  Project,
  ProjectStatus,
  ProjectTag,
  Repo,
  StartIntake,
  TextDiffLine,
  UpdateIssue,
  UpdateProject,
  UpdateProjectStatus,
  UpdateProjectTag,
} from 'shared/types';
import { handleApiResponse } from '@/shared/lib/api';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';

async function request<T>(
  url: string,
  method: 'GET' | 'POST' | 'PUT' | 'DELETE' = 'GET',
  body?: unknown
): Promise<T> {
  const response = await makeLocalApiRequest(url, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return handleApiResponse<T>(response);
}

export const projectsApi = {
  list: () => request<Project[]>('/api/projects'),
  create: (data: CreateProject) =>
    request<Project>('/api/projects', 'POST', data),
  update: (id: string, data: UpdateProject) =>
    request<Project>(`/api/projects/${id}`, 'PUT', data),
  delete: (id: string) => request<void>(`/api/projects/${id}`, 'DELETE'),

  listRepos: (id: string) => request<Repo[]>(`/api/projects/${id}/repos`),
  linkRepo: (id: string, repoId: string) =>
    request<Repo[]>(`/api/projects/${id}/repos`, 'POST', { repo_id: repoId }),
  unlinkRepo: (id: string, repoId: string) =>
    request<Repo[]>(`/api/projects/${id}/repos/${repoId}`, 'DELETE'),

  createStatus: (id: string, data: CreateProjectStatus) =>
    request<ProjectStatus>(`/api/projects/${id}/statuses`, 'POST', data),
  updateStatus: (statusId: string, data: UpdateProjectStatus) =>
    request<ProjectStatus>(`/api/project-statuses/${statusId}`, 'PUT', data),
  deleteStatus: (statusId: string, moveTo?: string) =>
    request<void>(
      `/api/project-statuses/${statusId}${moveTo ? `?move_to=${moveTo}` : ''}`,
      'DELETE'
    ),

  createTag: (id: string, data: CreateProjectTag) =>
    request<ProjectTag>(`/api/projects/${id}/tags`, 'POST', data),
  updateTag: (tagId: string, data: UpdateProjectTag) =>
    request<ProjectTag>(`/api/project-tags/${tagId}`, 'PUT', data),
  deleteTag: (tagId: string) =>
    request<void>(`/api/project-tags/${tagId}`, 'DELETE'),

  createIssue: (id: string, data: CreateIssue) =>
    request<Issue>(`/api/projects/${id}/issues`, 'POST', data),
  bulkUpdateIssues: (id: string, items: BulkUpdateIssueItem[]) =>
    request<Issue[]>(`/api/projects/${id}/issues/bulk`, 'POST', items),
  findIssueByExternalId: (id: string, externalId: string) =>
    request<Issue | null>(
      `/api/projects/${id}/issues/by-external-id?external_id=${encodeURIComponent(externalId)}`
    ),
  detectExternalId: (id: string, text: string) =>
    request<string | null>(`/api/projects/${id}/detect-external-id`, 'POST', {
      text,
    }),
};

export const intakeApi = {
  start: (projectId: string, data: StartIntake) =>
    request<IntakeView>(`/api/projects/${projectId}/intakes`, 'POST', data),
  get: (id: string) => request<IntakeView>(`/api/intakes/${id}`),
  retry: (id: string) =>
    request<IntakeView>(`/api/intakes/${id}/retry`, 'POST'),
  accept: (id: string, data: AcceptIntake) =>
    request<Issue>(`/api/intakes/${id}/accept`, 'POST', data),
  discard: (id: string) => request<void>(`/api/intakes/${id}/discard`, 'POST'),
};

export const textDiff = (oldText: string, newText: string) =>
  request<TextDiffLine[]>('/api/text-diff', 'POST', {
    old: oldText,
    new: newText,
  });

export const issuesApi = {
  update: (id: string, data: UpdateIssue) =>
    request<Issue>(`/api/issues/${id}`, 'PUT', data),
  delete: (id: string) => request<void>(`/api/issues/${id}`, 'DELETE'),

  addTag: (id: string, tagId: string) =>
    request<IssueTag>(`/api/issues/${id}/tags/${tagId}`, 'POST'),
  removeTag: (id: string, tagId: string) =>
    request<void>(`/api/issues/${id}/tags/${tagId}`, 'DELETE'),

  createRelationship: (id: string, data: CreateIssueRelationship) =>
    request<IssueRelationship>(`/api/issues/${id}/relationships`, 'POST', data),
  deleteRelationship: (relationshipId: string) =>
    request<void>(`/api/issue-relationships/${relationshipId}`, 'DELETE'),

  createComment: (id: string, data: CreateIssueComment) =>
    request<IssueComment>(`/api/issues/${id}/comments`, 'POST', data),
  updateComment: (commentId: string, message: string) =>
    request<IssueComment>(`/api/issue-comments/${commentId}`, 'PUT', {
      message,
    }),
  deleteComment: (commentId: string) =>
    request<void>(`/api/issue-comments/${commentId}`, 'DELETE'),

  listAttachments: (id: string) =>
    request<AttachmentResponse[]>(`/api/issues/${id}/attachments`),
  listAttachmentTexts: (id: string) =>
    request<AttachmentText[]>(`/api/issues/${id}/attachment-texts`),
  listDescriptions: (id: string) =>
    request<IssueDescription[]>(`/api/issues/${id}/descriptions`),
  listTimeline: (id: string) =>
    request<IssueEvent[]>(`/api/issues/${id}/timeline`),
  linkAttachments: (id: string, attachmentIds: string[]) =>
    request<AttachmentResponse[]>(`/api/issues/${id}/attachments`, 'POST', {
      attachment_ids: attachmentIds,
    }),
};
