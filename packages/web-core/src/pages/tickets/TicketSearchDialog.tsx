import { useEffect, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { Input } from '@kanbain/ui/components/Input';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@kanbain/ui/components/KeyboardDialog';
import type { SearchResults, TicketRef } from 'shared/types';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useProjects } from '@/shared/hooks/useTicketStreams';
import { defineModal } from '@/shared/lib/modals';
import { searchApi } from '@/shared/lib/ticketsApi';
import { CodeResultLine } from './CodeResultLine';
import { requestOpenTicket } from './openTicket';

export interface TicketSearchDialogProps {
  query?: string;
}

const PROJECT_STORAGE_KEY = 'kanbain.tickets.projectId';
const DEBOUNCE_MS = 300;

export const KIND_LABELS: Record<string, string> = {
  ticket: 'Ticket',
  comment: 'Comment',
  attachment: 'Attachment',
  prior_work: 'Prior work',
  run: 'Run',
  code: 'Code',
};

function storedProjectId(): string | null {
  try {
    return window.localStorage.getItem(PROJECT_STORAGE_KEY);
  } catch {
    return null;
  }
}

/** One ticket as a search result line. */
export function TicketResultLine({
  ticket,
  kind,
  excerpt,
  onOpen,
}: {
  ticket: TicketRef;
  kind?: string;
  excerpt: string;
  onOpen: () => void;
}) {
  const done = ticket.status_kind === 'done';
  return (
    <button
      type="button"
      onClick={onOpen}
      className="flex w-full flex-col gap-half rounded-sm px-base py-half text-left hover:bg-secondary"
      data-testid="search-result"
    >
      <span className="flex flex-wrap items-baseline gap-x-base text-sm">
        <span className="font-mono text-xs text-low">
          {ticket.internal_id}
          {ticket.external_id ? ` · ${ticket.external_id}` : ''}
        </span>
        <span className="font-medium text-normal">{ticket.title}</span>
        {ticket.status && (
          <span
            className={`rounded-sm px-half text-xs ${
              done ? 'bg-success/15 text-success' : 'bg-secondary text-low'
            }`}
          >
            {done ? `Done in ${ticket.internal_id}` : ticket.status}
          </span>
        )}
        {kind && kind !== 'ticket' && (
          <span className="text-xs text-low">{KIND_LABELS[kind] ?? kind}</span>
        )}
      </span>
      <span className="line-clamp-2 text-xs text-low">{excerpt}</span>
    </button>
  );
}

const TicketSearchDialogImpl = create<TicketSearchDialogProps>(({ query }) => {
  const modal = useModal();
  const appNavigation = useAppNavigation();
  const { projects } = useProjects();
  const [text, setText] = useState(query ?? '');
  const [allProjects, setAllProjects] = useState(false);
  const [results, setResults] = useState<SearchResults | null>(null);
  const [error, setError] = useState<string | null>(null);
  const projectId = storedProjectId() ?? projects[0]?.id ?? null;

  useEffect(() => {
    if (!text.trim()) {
      setResults(null);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      searchApi
        .search({
          query: text,
          project_id: allProjects ? undefined : (projectId ?? undefined),
          all_projects: allProjects || !projectId,
          limit: 20,
        })
        .then((r) => {
          if (!cancelled) {
            setResults(r);
            setError(null);
          }
        })
        .catch((err: unknown) => {
          if (!cancelled)
            setError(err instanceof Error ? err.message : 'Search failed');
        });
    }, DEBOUNCE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [text, allProjects, projectId]);

  const close = () => {
    modal.hide();
    modal.remove();
  };

  const open = (ticket: TicketRef) => {
    requestOpenTicket({ projectId: ticket.project_id, issueId: ticket.id });
    close();
    appNavigation.goToTickets();
  };

  const projectName = (id: string) => projects.find((p) => p.id === id)?.name;

  return (
    <Dialog
      open={modal.visible}
      onOpenChange={(isOpen) => {
        if (!isOpen) close();
      }}
    >
      <DialogContent className="sm:max-w-[720px]">
        <DialogHeader>
          <DialogTitle>Search tickets and history</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-base py-base">
          <div className="flex items-center gap-base">
            <Input
              autoFocus
              aria-label="Search query"
              placeholder="Describe what you are looking for, or paste an ID"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
            <label className="flex shrink-0 items-center gap-half text-sm text-low">
              <input
                type="checkbox"
                checked={allProjects}
                onChange={(e) => setAllProjects(e.target.checked)}
              />
              All projects
            </label>
          </div>
          {results?.notice && (
            <p className="text-xs text-low" data-testid="search-notice">
              {results.notice}
            </p>
          )}
          {error && <p className="text-sm text-error">{error}</p>}
          <div
            className="flex max-h-[60vh] flex-col overflow-y-auto"
            data-testid="ticket-search-results"
          >
            {results && results.hits.length === 0 && (
              <p className="text-sm text-low">Nothing found.</p>
            )}
            {results?.hits.map((hit, i) => {
              if (hit.kind === 'code') {
                return (
                  <CodeResultLine
                    key={i}
                    hit={hit}
                    repoName={hit.repo_id ? results.repos[hit.repo_id] : null}
                    onError={setError}
                  />
                );
              }
              const ticket = hit.issue_id
                ? results.tickets[hit.issue_id]
                : undefined;
              if (!ticket) return null;
              return (
                <div key={i} className="flex flex-col">
                  {allProjects && (
                    <span className="px-base text-xs text-low">
                      {projectName(ticket.project_id)}
                    </span>
                  )}
                  <TicketResultLine
                    ticket={ticket}
                    kind={hit.kind}
                    excerpt={hit.excerpt}
                    onOpen={() => open(ticket)}
                  />
                </div>
              );
            })}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
});

export const TicketSearchDialog = defineModal<TicketSearchDialogProps, void>(
  TicketSearchDialogImpl
);
