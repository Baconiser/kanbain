import { useEffect, useState } from 'react';
import { CaretDownIcon } from '@phosphor-icons/react';

const STORAGE_PREFIX = 'kanbain.ticketPanel.section.';

function storedOpen(id: string, fallback: boolean): boolean {
  try {
    const stored = window.localStorage.getItem(STORAGE_PREFIX + id);
    if (stored === 'true') return true;
    if (stored === 'false') return false;
  } catch {
    // Storage can be unavailable (private mode).
  }
  return fallback;
}

interface CollapsibleSectionProps {
  /** Remembers the fold across tickets, per section kind. */
  id: string;
  title: string;
  /** Short summary beside the title, e.g. a count; shown folded too. */
  meta?: React.ReactNode;
  /** Right-aligned controls, shown while open. */
  action?: React.ReactNode;
  defaultOpen?: boolean;
  /** Opens the section regardless of the remembered fold. */
  forceOpen?: boolean;
  testId?: string;
  children: React.ReactNode;
}

/** A titled ticket-panel block that folds away from its header. */
export function CollapsibleSection({
  id,
  title,
  meta,
  action,
  defaultOpen = true,
  forceOpen = false,
  testId,
  children,
}: CollapsibleSectionProps) {
  const [open, setOpenState] = useState(() => storedOpen(id, defaultOpen));

  useEffect(() => {
    if (forceOpen) setOpenState(true);
  }, [forceOpen]);

  const toggle = () =>
    setOpenState((prev) => {
      try {
        window.localStorage.setItem(STORAGE_PREFIX + id, String(!prev));
      } catch {
        // Not remembering the fold is fine.
      }
      return !prev;
    });

  return (
    <section className="flex flex-col gap-half" data-testid={testId}>
      <div className="flex min-h-6 items-center justify-between gap-base">
        <button
          type="button"
          aria-expanded={open}
          data-testid={testId ? `${testId}-toggle` : undefined}
          className="group flex min-w-0 items-center gap-half text-left text-sm"
          onClick={toggle}
        >
          <CaretDownIcon
            weight="fill"
            className={`size-icon-xs shrink-0 text-low transition-transform ${
              open ? '' : '-rotate-90'
            }`}
          />
          <span className="font-medium text-normal group-hover:text-high">
            {title}
          </span>
          {meta != null && meta !== '' && (
            <span className="truncate text-xs text-low">{meta}</span>
          )}
        </button>
        {open && action}
      </div>
      {open && children}
    </section>
  );
}
