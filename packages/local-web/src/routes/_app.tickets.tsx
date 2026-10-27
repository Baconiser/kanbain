import { createFileRoute } from '@tanstack/react-router';
import { TicketsPage } from '@/pages/tickets/TicketsPage';

export const Route = createFileRoute('/_app/tickets')({
  component: TicketsPage,
});
