import { useEffect, useState } from 'react';
import { FileIcon } from '@phosphor-icons/react';
import type { AttachmentResponse, AttachmentText } from 'shared/types';
import { issuesApi } from '@/shared/lib/ticketsApi';

interface TicketAttachmentsProps {
  issueId: string;
  /** Changes when attachments are added, to reload the list. */
  refreshKey: number;
  onError: (message: string) => void;
}

/** A ticket's attachments with the text extracted from them. */
export function TicketAttachments({
  issueId,
  refreshKey,
  onError,
}: TicketAttachmentsProps) {
  const [files, setFiles] = useState<AttachmentResponse[]>([]);
  const [texts, setTexts] = useState<Record<string, AttachmentText>>({});
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      issuesApi.listAttachments(issueId),
      issuesApi.listAttachmentTexts(issueId),
    ])
      .then(([list, extracted]) => {
        if (cancelled) return;
        setFiles(list);
        setTexts(
          Object.fromEntries(extracted.map((t) => [t.attachment_id, t]))
        );
      })
      .catch((err: unknown) =>
        onError(err instanceof Error ? err.message : 'Failed to load files')
      );
    return () => {
      cancelled = true;
    };
  }, [issueId, refreshKey, onError]);

  if (files.length === 0) return null;

  return (
    <section
      className="flex flex-col gap-half"
      data-testid="ticket-attachments"
    >
      <h3 className="text-sm font-medium text-normal">
        Attachments ({files.length})
      </h3>
      <ul className="flex flex-col gap-half text-sm">
        {files.map((file) => {
          const text = texts[file.id];
          return (
            <li key={file.id} className="flex flex-col gap-half">
              <div className="flex items-center gap-half">
                <FileIcon className="size-icon-xs shrink-0 text-low" />
                <span className="truncate">{file.original_name}</span>
                {text?.error ? (
                  <span className="text-xs text-error">{text.error}</span>
                ) : text ? (
                  <button
                    type="button"
                    className="text-xs text-brand hover:underline"
                    onClick={() => setOpen(open === file.id ? null : file.id)}
                  >
                    {open === file.id
                      ? 'Hide text'
                      : `Show text (${text.format})`}
                  </button>
                ) : null}
              </div>
              {open === file.id && text && (
                <pre
                  className="max-h-72 overflow-auto whitespace-pre-wrap rounded-sm border bg-secondary p-base text-xs"
                  data-testid="attachment-text"
                >
                  {text.text}
                </pre>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
