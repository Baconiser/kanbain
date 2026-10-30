import { useMemo } from 'react';
import { useJsonPatchWsStream } from '@/shared/hooks/useJsonPatchWsStream';
import type {
  BoardPlanning,
  Issue,
  IssueComment,
  IssueEvent,
  IssueRelationship,
  IssueTag,
  Project,
  ProjectStatus,
  ProjectTag,
} from 'shared/types';

type ProjectsState = { projects: Record<string, Project> };

const initialProjects = (): ProjectsState => ({ projects: {} });

/** All projects, live, ordered by sort order then creation. */
export function useProjects() {
  const { data, isInitialized, error } = useJsonPatchWsStream<ProjectsState>(
    '/api/projects/stream/ws',
    true,
    initialProjects
  );

  const projects = useMemo(
    () =>
      Object.values(data?.projects ?? {}).sort(
        (a, b) =>
          a.sort_order - b.sort_order ||
          a.created_at.localeCompare(b.created_at)
      ),
    [data]
  );

  return { projects, isLoading: !isInitialized, error };
}

type BoardState = {
  statuses: Record<string, ProjectStatus>;
  issues: Record<string, Issue>;
  tags: Record<string, ProjectTag>;
  issue_tags: Record<string, IssueTag>;
  relationships: Record<string, IssueRelationship>;
  /** Planning states by issue id. */
  planning: Record<string, BoardPlanning>;
};

const initialBoard = (): BoardState => ({
  statuses: {},
  issues: {},
  tags: {},
  issue_tags: {},
  relationships: {},
  planning: {},
});

export interface ProjectBoard {
  statuses: ProjectStatus[];
  issues: Issue[];
  issuesById: Record<string, Issue>;
  tags: ProjectTag[];
  tagsById: Record<string, ProjectTag>;
  /** Tag ids per issue id. */
  tagIdsByIssue: Record<string, string[]>;
  relationships: IssueRelationship[];
  /** Planning states by issue id (tickets that were planned). */
  planningByIssue: Record<string, BoardPlanning>;
  isLoading: boolean;
  error: string | null;
}

/** One project's board, live. */
export function useProjectBoard(projectId: string | undefined): ProjectBoard {
  const { data, isInitialized, error } = useJsonPatchWsStream<BoardState>(
    projectId ? `/api/projects/${projectId}/stream/ws` : undefined,
    !!projectId,
    initialBoard
  );

  return useMemo(() => {
    const state = data ?? initialBoard();
    const statuses = Object.values(state.statuses).sort(
      (a, b) =>
        a.sort_order - b.sort_order || a.created_at.localeCompare(b.created_at)
    );
    const issues = Object.values(state.issues).sort(
      (a, b) => a.sort_order - b.sort_order || a.number - b.number
    );
    const tags = Object.values(state.tags).sort((a, b) =>
      a.name.localeCompare(b.name)
    );
    const tagIdsByIssue: Record<string, string[]> = {};
    for (const it of Object.values(state.issue_tags)) {
      (tagIdsByIssue[it.issue_id] ??= []).push(it.tag_id);
    }
    return {
      statuses,
      issues,
      issuesById: state.issues,
      tags,
      tagsById: state.tags,
      tagIdsByIssue,
      relationships: Object.values(state.relationships),
      planningByIssue: state.planning ?? {},
      isLoading: !!projectId && !isInitialized,
      error,
    };
  }, [data, isInitialized, error, projectId]);
}

type CommentsState = { comments: Record<string, IssueComment> };

const initialComments = (): CommentsState => ({ comments: {} });

/** One ticket's comments, live, oldest first. */
export function useIssueComments(issueId: string | undefined) {
  const { data, isInitialized } = useJsonPatchWsStream<CommentsState>(
    issueId ? `/api/issues/${issueId}/comments/stream/ws` : undefined,
    !!issueId,
    initialComments
  );

  const comments = useMemo(
    () =>
      Object.values(data?.comments ?? {}).sort((a, b) =>
        a.created_at.localeCompare(b.created_at)
      ),
    [data]
  );

  return { comments, isLoading: !!issueId && !isInitialized };
}

type TimelineState = { events: Record<string, IssueEvent> };

const initialTimeline = (): TimelineState => ({ events: {} });

/** One ticket's timeline, live, oldest first. */
export function useIssueTimeline(issueId: string | undefined) {
  const { data, isInitialized } = useJsonPatchWsStream<TimelineState>(
    issueId ? `/api/issues/${issueId}/timeline/stream/ws` : undefined,
    !!issueId,
    initialTimeline
  );

  const events = useMemo(
    () =>
      Object.values(data?.events ?? {}).sort(
        (a, b) =>
          Date.parse(a.occurred_at) - Date.parse(b.occurred_at) ||
          a.created_at.localeCompare(b.created_at)
      ),
    [data]
  );

  return { events, isLoading: !!issueId && !isInitialized };
}
