import { useEffect, useState } from 'react';
import type { RelatedTicket } from 'shared/types';
import { searchApi } from '@/shared/lib/ticketsApi';
import { TicketResultLine } from './TicketSearchDialog';

interface TicketRelatedProps {
  issueId: string;
  /** Changes when the ticket's text changes, to reload. */
  refreshKey: string;
  onSelectIssue: (issueId: string) => void;
}

/** Similar tickets and runs (E23), from the search index. */
export function TicketRelated({
  issueId,
  refreshKey,
  onSelectIssue,
}: TicketRelatedProps) {
  const [related, setRelated] = useState<RelatedTicket[] | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      searchApi
        .related(issueId)
        .then((r) => {
          if (!cancelled) {
            setRelated(r);
            setUnavailable(false);
          }
        })
        .catch(() => {
          if (!cancelled) setUnavailable(true);
        });
    }, 400);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [issueId, refreshKey]);

  if (unavailable || !related) return null;

  return (
    <section className="flex flex-col gap-half" data-testid="ticket-related">
      <h3 className="text-sm font-medium text-normal">Related</h3>
      {related.length === 0 ? (
        <p className="text-sm text-low">No similar tickets found.</p>
      ) : (
        <div className="flex flex-col">
          {related.map((r) => (
            <TicketResultLine
              key={r.ticket.id}
              ticket={r.ticket}
              kind={r.kinds.includes('run') ? 'run' : r.kinds[0]}
              excerpt={r.excerpt}
              onOpen={() => onSelectIssue(r.ticket.id)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
