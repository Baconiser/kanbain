import type { Issue, Project } from 'shared/types';

/** The internal ticket id, e.g. `KAN-42`. */
export function internalTicketId(
  issue: Pick<Issue, 'number'>,
  project: Pick<Project, 'id_prefix'>
): string {
  return `${project.id_prefix}-${issue.number}`;
}

/** Card label: the internal id, followed by the External ID when set. */
export function ticketDisplayId(
  issue: Pick<Issue, 'number' | 'external_id'>,
  project: Pick<Project, 'id_prefix'>
): string {
  const internal = internalTicketId(issue, project);
  return issue.external_id ? `${internal} · ${issue.external_id}` : internal;
}

/** Case-insensitive match on title, internal id and External ID. */
export function ticketMatchesQuery(
  issue: Issue,
  project: Pick<Project, 'id_prefix'>,
  query: string
): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [issue.title, internalTicketId(issue, project), issue.external_id]
    .filter((v): v is string => !!v)
    .some((v) => v.toLowerCase().includes(q));
}
