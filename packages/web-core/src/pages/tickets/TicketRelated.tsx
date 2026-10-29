import { useEffect, useState } from 'react';
import type { RelatedView } from 'shared/types';
import { searchApi } from '@/shared/lib/ticketsApi';
import { CodeResultLine } from './CodeResultLine';
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
  const [related, setRelated] = useState<RelatedView | null>(null);
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
      {related.tickets.length === 0 ? (
        <p className="text-sm text-low">No similar tickets found.</p>
      ) : (
        <div className="flex flex-col">
          {related.tickets.map((r) => (
            <div key={r.ticket.id} className="flex flex-col">
              <TicketResultLine
                ticket={r.ticket}
                kind={r.kinds.includes('run') ? 'run' : r.kinds[0]}
                excerpt={r.excerpt}
                onOpen={() => onSelectIssue(r.ticket.id)}
              />
              {r.changed.length > 0 && (
                <span
                  className="px-base text-xs text-low"
                  data-testid="related-changed"
                >
                  {r.ticket.internal_id} changed {r.changed.join(', ')}
                </span>
              )}
            </div>
          ))}
        </div>
      )}
      {related.code.length > 0 && (
        <div className="flex flex-col" data-testid="related-code">
          <h4 className="text-xs font-medium uppercase text-low">Code</h4>
          {related.code.map((c, i) => (
            <CodeResultLine
              key={i}
              hit={c.hit}
              repoName={c.repo_name}
              changedBy={c.changed_by}
              onOpenTicket={(t) => onSelectIssue(t.id)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
