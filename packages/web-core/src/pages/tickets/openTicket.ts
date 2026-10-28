/**
 * Opening a ticket from outside the tickets page (search results). The
 * request waits here until the page is mounted to pick it up.
 */
export interface OpenTicketRequest {
  projectId: string;
  issueId: string;
}

const EVENT = 'kanbain:open-ticket';
let pending: OpenTicketRequest | null = null;

export function requestOpenTicket(request: OpenTicketRequest) {
  pending = request;
  window.dispatchEvent(new CustomEvent(EVENT));
}

/** The pending request, once. */
export function takeOpenTicketRequest(): OpenTicketRequest | null {
  const request = pending;
  pending = null;
  return request;
}

export function onOpenTicketRequest(handler: () => void): () => void {
  window.addEventListener(EVENT, handler);
  return () => window.removeEventListener(EVENT, handler);
}
