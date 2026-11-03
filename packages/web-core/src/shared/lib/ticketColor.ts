import type { Issue } from 'shared/types';

/** Accent colour of a main ticket, shared by its sub-tickets and their
 * workspaces. Hues step by the golden angle per ticket number: looks random,
 * stays stable across reloads, and tickets created back to back never get
 * lookalike colours. */
export function ticketAccentColor(issue: Pick<Issue, 'number'>): string {
  const hue = Math.round((issue.number * 137.508) % 360);
  return `hsl(${hue} 70% 55%)`;
}
