import { createFileRoute } from '@tanstack/react-router';
import { TicketsPlaceholderPage } from '@/pages/tickets/TicketsPlaceholderPage';

export const Route = createFileRoute('/_app/tickets')({
  component: TicketsPlaceholderPage,
});
