import { useEffect, useRef, useState } from 'react';
import {
  ArrowsOutSimpleIcon,
  CaretDownIcon,
  CaretUpIcon,
} from '@phosphor-icons/react';
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from '@kanbain/ui/components/Dialog';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';

// About five lines of body text.
const COLLAPSED_PX = 160;

interface TicketDescriptionProps {
  title: string;
  value: string;
  onChange: (value: string) => void;
  onPasteFiles: (files: File[]) => void;
}

/**
 * The description, cut to its first few lines until clicked. Long specs can
 * also be opened in a big popup editor.
 */
export function TicketDescription({
  title,
  value,
  onChange,
  onPasteFiles,
}: TicketDescriptionProps) {
  const [expanded, setExpanded] = useState(false);
  const [popout, setPopout] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);

  // Only offer "show more" when there is more to show.
  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    const measure = () => setOverflows(el.scrollHeight > COLLAPSED_PX + 8);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [popout]);

  const collapsed = !expanded && overflows;

  const editor = (className: string) => (
    <WYSIWYGEditor
      placeholder="Describe the work. Paste images or files to attach them."
      value={value}
      onChange={onChange}
      onPasteFiles={onPasteFiles}
      className={className}
    />
  );

  return (
    <section
      className="flex flex-col gap-half"
      data-testid="ticket-description"
    >
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-normal">Description</h3>
        <div className="flex items-center gap-base text-xs text-low">
          {overflows && !popout && (
            <button
              type="button"
              className="flex items-center gap-half hover:text-normal"
              onClick={() => setExpanded((e) => !e)}
            >
              {expanded ? (
                <>
                  <CaretUpIcon className="size-icon-xs" /> Collapse
                </>
              ) : (
                <>
                  <CaretDownIcon className="size-icon-xs" /> Show all
                </>
              )}
            </button>
          )}
          <button
            type="button"
            aria-label="Open description in a larger editor"
            className="flex items-center gap-half hover:text-normal"
            onClick={() => setPopout(true)}
          >
            <ArrowsOutSimpleIcon className="size-icon-xs" />
          </button>
        </div>
      </div>

      {popout ? (
        <button
          type="button"
          className="rounded-sm border border-dashed p-base text-left text-sm text-low hover:text-normal"
          onClick={() => setPopout(false)}
        >
          Editing in the larger editor…
        </button>
      ) : (
        <div
          className="relative rounded-sm border bg-secondary p-base"
          // Clicking the cut-off text opens it fully, then edits as usual.
          onClickCapture={() => collapsed && setExpanded(true)}
        >
          <div
            ref={contentRef}
            className={collapsed ? 'overflow-hidden' : undefined}
            style={collapsed ? { maxHeight: COLLAPSED_PX } : undefined}
          >
            {editor('min-h-[60px]')}
          </div>
          {collapsed && (
            <div className="pointer-events-none absolute inset-x-0 bottom-0 h-10 rounded-b-sm bg-gradient-to-t from-secondary to-transparent" />
          )}
        </div>
      )}

      <Dialog open={popout} onOpenChange={setPopout}>
        <DialogContent className="flex h-[85vh] !max-w-4xl flex-col gap-base p-double">
          <DialogTitle className="truncate pr-double text-base">
            {title}
          </DialogTitle>
          <div className="min-h-0 flex-1 overflow-y-auto rounded-sm border bg-secondary p-base">
            {editor('min-h-full')}
          </div>
        </DialogContent>
      </Dialog>
    </section>
  );
}
