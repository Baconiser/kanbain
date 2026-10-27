import { KanbanIcon } from '@phosphor-icons/react';
import { usePageTitle } from '@/shared/hooks/usePageTitle';

export function TicketsPlaceholderPage() {
  usePageTitle('Tickets');

  return (
    <div className="h-full w-full overflow-auto bg-primary">
      <div className="mx-auto flex min-h-full w-full max-w-xl items-center px-base py-double">
        <div className="w-full rounded-sm border border-border bg-secondary p-double text-center">
          <KanbanIcon className="mx-auto size-10 text-low" weight="bold" />
          <h1 className="mt-base text-xl font-semibold text-high">
            Local tickets are coming
          </h1>
          <p className="mt-half text-sm text-low">
            The ticket board is being rebuilt on local storage. Until then, use
            workspaces to run agents on your repositories.
          </p>
        </div>
      </div>
    </div>
  );
}
