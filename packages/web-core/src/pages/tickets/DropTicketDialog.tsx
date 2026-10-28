import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { create, useModal } from '@ebay/nice-modal-react';
import { FileIcon, UploadSimpleIcon, XIcon } from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { Label } from '@kanbain/ui/components/Label';
import { Textarea } from '@kanbain/ui/components/Textarea';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@kanbain/ui/components/KeyboardDialog';
import type {
  EnhancedTicket,
  IntakeView,
  RelatedTicket,
  Issue,
  IssueMode,
  IssuePriority,
} from 'shared/types';
import { attachmentsApi } from '@/shared/lib/api';
import { defineModal, getErrorMessage } from '@/shared/lib/modals';
import { intakeApi, issuesApi, searchApi } from '@/shared/lib/ticketsApi';
import { TextDiffView } from './TextDiffView';
import { TicketResultLine } from './TicketSearchDialog';

export interface DropTicketDialogProps {
  projectId: string;
}

const PRIORITIES: IssuePriority[] = ['urgent', 'high', 'medium', 'low'];
const POLL_MS = 1500;

interface TicketForm {
  title: string;
  description: string;
  criteria: string;
  questions: string;
  externalId: string;
  priority: IssuePriority | '';
  mode: IssueMode;
}

function lines(text: string): string[] {
  return text
    .split('\n')
    .map((l) => l.replace(/^\s*[-*]\s+/, '').trim())
    .filter(Boolean);
}

/** Same Markdown as the backend's `EnhancedTicket::to_markdown`. */
export function composeDescription(
  description: string,
  criteria: string[],
  questions: string[]
): string {
  let out = description.trim();
  const section = (heading: string, items: string[]) => {
    const kept = items.map((i) => i.trim()).filter(Boolean);
    if (kept.length === 0) return;
    if (out) out += '\n\n';
    out += `## ${heading}\n`;
    for (const item of kept) out += `\n- ${item}`;
  };
  section('Acceptance criteria', criteria);
  section('Open questions', questions);
  return out;
}

function formFrom(view: IntakeView): TicketForm {
  const enhanced: EnhancedTicket | null = view.intake.enhanced;
  if (enhanced) {
    return {
      title: enhanced.title,
      description: enhanced.description,
      criteria: enhanced.acceptance_criteria.join('\n'),
      questions: enhanced.open_questions.join('\n'),
      externalId: enhanced.external_id ?? view.external_id ?? '',
      priority: enhanced.priority ?? '',
      mode: 'ask',
    };
  }
  // No agent result: start from the dropped text as is.
  const raw = view.intake.raw_text.trim();
  const firstLine = raw.split('\n').find((l) => l.trim()) ?? '';
  return {
    title: firstLine.replace(/^#+\s*/, '').slice(0, 200),
    description: raw,
    criteria: '',
    questions: '',
    externalId: view.external_id ?? '',
    priority: '',
    mode: 'ask',
  };
}

function formatUsd(value: number): string {
  return `$${value.toFixed(value < 0.01 ? 4 : 2)}`;
}

const DropTicketDialogImpl = create<DropTicketDialogProps>(({ projectId }) => {
  const modal = useModal();
  const [text, setText] = useState('');
  const [files, setFiles] = useState<globalThis.File[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<IntakeView | null>(null);
  const [form, setForm] = useState<TicketForm | null>(null);
  const [target, setTarget] = useState<'update' | 'create'>('update');
  // Similar tickets from the search index (E23), and one picked to update.
  const [duplicates, setDuplicates] = useState<RelatedTicket[]>([]);
  const [duplicateTarget, setDuplicateTarget] = useState<{
    issue: Issue;
    internalId: string;
  } | null>(null);
  const [openText, setOpenText] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  const close = (result: Issue | null) => {
    modal.resolve(result);
    modal.hide();
    // Unmount, so the next drop starts empty.
    modal.remove();
  };

  const addFiles = (list: FileList | null) => {
    if (!list) return;
    // Copy now: the input's live list is cleared right after.
    const added = Array.from(list);
    setFiles((prev) => [...prev, ...added]);
  };

  const start = async () => {
    if (!text.trim() && files.length === 0) {
      setError('Paste the ticket text or drop a file.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const uploaded = [];
      for (const file of files)
        uploaded.push(await attachmentsApi.upload(file));
      const result = await intakeApi.start(projectId, {
        text,
        attachment_ids: uploaded.map((u) => u.id),
      });
      setView(result);
    } catch (err) {
      setError(getErrorMessage(err) || 'Failed to read the ticket');
    } finally {
      setBusy(false);
    }
  };

  const status = view?.intake.status;
  const intakeId = view?.intake.id;

  // Poll while the intake agent works.
  useEffect(() => {
    if (!intakeId || status !== 'enhancing') return;
    const timer = window.setInterval(() => {
      intakeApi
        .get(intakeId)
        .then(setView)
        .catch(() => {});
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [intakeId, status]);

  // Fill the form once the agent is done (or failed).
  useEffect(() => {
    if (view && !form && (status === 'ready' || status === 'failed')) {
      setForm(formFrom(view));
    }
  }, [view, form, status]);

  const composed = useMemo(
    () =>
      form
        ? composeDescription(
            form.description,
            lines(form.criteria),
            lines(form.questions)
          )
        : '',
    [form]
  );

  const existing = view?.existing_issue ?? null;
  const updateTarget =
    duplicateTarget ??
    (existing && target === 'update'
      ? { issue: existing, internalId: view?.existing_internal_id ?? '' }
      : null);
  const updating = !!updateTarget;

  // Possible duplicates once the agent is done.
  useEffect(() => {
    if (!intakeId || (status !== 'ready' && status !== 'failed')) return;
    let cancelled = false;
    searchApi
      .intakeDuplicates(intakeId)
      .then((found) => {
        if (!cancelled) setDuplicates(found);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [intakeId, status]);

  const pickDuplicate = async (related: RelatedTicket) => {
    if (duplicateTarget?.issue.id === related.ticket.id) {
      setDuplicateTarget(null);
      return;
    }
    try {
      const issue = await issuesApi.get(related.ticket.id);
      setDuplicateTarget({ issue, internalId: related.ticket.internal_id });
    } catch (err) {
      setError(getErrorMessage(err) || 'Failed to load the ticket');
    }
  };

  const retry = async () => {
    if (!intakeId) return;
    setForm(null);
    try {
      setView(await intakeApi.retry(intakeId));
    } catch (err) {
      setError(getErrorMessage(err) || 'Retry failed');
    }
  };

  const accept = async () => {
    if (!view || !form) return;
    if (!form.title.trim()) {
      setError('A title is required.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const issue = await intakeApi.accept(view.intake.id, {
        title: form.title,
        description: composed,
        external_id: form.externalId.trim() || undefined,
        priority: form.priority || undefined,
        mode: form.mode,
        update_issue_id: updateTarget?.issue.id,
      });
      close(issue);
    } catch (err) {
      setError(getErrorMessage(err) || 'Failed to save the ticket');
    } finally {
      setBusy(false);
    }
  };

  const cancel = useCallback(() => {
    if (
      view &&
      (status === 'ready' || status === 'failed' || status === 'enhancing')
    ) {
      void intakeApi.discard(view.intake.id).catch(() => {});
    }
    close(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, status]);

  const set = (patch: Partial<TicketForm>) =>
    setForm((prev) => (prev ? { ...prev, ...patch } : prev));

  return (
    <Dialog
      open={modal.visible}
      onOpenChange={(open) => {
        if (!open) cancel();
      }}
    >
      <DialogContent className="sm:max-w-[1040px]">
        <DialogHeader>
          <DialogTitle>{view ? 'Review ticket' : 'Drop ticket'}</DialogTitle>
        </DialogHeader>

        {!view && (
          <div className="flex flex-col gap-base py-base">
            <p className="text-sm text-low">
              Paste a ticket (Jira wiki markup is fine) and drop its files: PDF,
              HTML, Word or XML exports, text and screenshots. The intake agent
              structures it; you review it before it becomes a ticket.
            </p>
            <Textarea
              aria-label="Ticket text"
              rows={10}
              placeholder="Paste the ticket here…"
              value={text}
              onChange={(e) => setText(e.target.value)}
              onPaste={(e) => {
                if (e.clipboardData.files.length > 0) {
                  e.preventDefault();
                  addFiles(e.clipboardData.files);
                }
              }}
            />
            <div
              className={`flex cursor-pointer flex-col items-center gap-half rounded-sm border border-dashed p-base text-sm ${
                dragOver ? 'border-brand bg-brand/10' : 'text-low'
              }`}
              data-testid="drop-zone"
              onClick={() => fileInput.current?.click()}
              onDragOver={(e) => {
                e.preventDefault();
                setDragOver(true);
              }}
              onDragLeave={() => setDragOver(false)}
              onDrop={(e) => {
                e.preventDefault();
                setDragOver(false);
                addFiles(e.dataTransfer.files);
              }}
            >
              <UploadSimpleIcon className="size-icon-base" />
              Drop files here or click to choose
              <input
                ref={fileInput}
                type="file"
                multiple
                hidden
                aria-label="Ticket files"
                onChange={(e) => {
                  addFiles(e.target.files);
                  e.target.value = '';
                }}
              />
            </div>
            {files.length > 0 && (
              <ul className="flex flex-col gap-half text-sm">
                {files.map((f, i) => (
                  <li key={i} className="flex items-center gap-half">
                    <FileIcon className="size-icon-xs text-low" />
                    <span className="truncate">{f.name}</span>
                    <button
                      type="button"
                      aria-label={`Remove ${f.name}`}
                      className="text-low hover:text-error"
                      onClick={() =>
                        setFiles((prev) => prev.filter((_, j) => j !== i))
                      }
                    >
                      <XIcon className="size-icon-xs" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        {view && (
          <div className="grid max-h-[70vh] grid-cols-1 gap-double overflow-y-auto py-base md:grid-cols-2">
            <section className="flex min-w-0 flex-col gap-base">
              <h3 className="text-sm font-medium">Original</h3>
              <pre
                className="max-h-80 overflow-auto whitespace-pre-wrap rounded-sm border bg-secondary p-base text-xs"
                data-testid="intake-original"
              >
                {view.intake.raw_text || '(no pasted text)'}
              </pre>
              {view.attachments.length > 0 && (
                <ul className="flex flex-col gap-half text-sm">
                  {view.attachments.map(({ attachment, text: extracted }) => (
                    <li key={attachment.id} className="flex flex-col gap-half">
                      <div className="flex items-center gap-half">
                        <FileIcon className="size-icon-xs text-low" />
                        <span className="truncate">
                          {attachment.original_name}
                        </span>
                        {extracted?.error ? (
                          <span className="text-xs text-error">
                            {extracted.error}
                          </span>
                        ) : extracted ? (
                          <button
                            type="button"
                            className="text-xs text-brand hover:underline"
                            onClick={() =>
                              setOpenText(
                                openText === attachment.id
                                  ? null
                                  : attachment.id
                              )
                            }
                          >
                            {openText === attachment.id
                              ? 'Hide text'
                              : `Show text (${extracted.format})`}
                          </button>
                        ) : (
                          <span className="text-xs text-low">no text</span>
                        )}
                      </div>
                      {openText === attachment.id && extracted && (
                        <pre
                          className="max-h-60 overflow-auto whitespace-pre-wrap rounded-sm border bg-secondary p-base text-xs"
                          data-testid="attachment-text"
                        >
                          {extracted.text}
                        </pre>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section className="flex min-w-0 flex-col gap-base">
              <div className="flex items-center justify-between gap-base">
                <h3 className="text-sm font-medium">Ticket</h3>
                {view.job && (
                  <span className="text-xs text-low" data-testid="intake-job">
                    {view.job.agent_def_id} · {view.job.model} ·{' '}
                    {view.job.tokens_in + view.job.tokens_out} tokens ·{' '}
                    {formatUsd(view.job.cost_usd)}
                  </span>
                )}
              </div>
              {status === 'enhancing' && (
                <p className="text-sm text-low" data-testid="intake-enhancing">
                  The intake agent is structuring the ticket…
                </p>
              )}
              {status === 'failed' && (
                <div className="flex items-center justify-between gap-base rounded-sm border border-error/40 bg-error/10 p-base text-sm">
                  <span>
                    The intake agent failed
                    {view.intake.error ? `: ${view.intake.error}` : ''}. You can
                    still create the ticket from the original.
                  </span>
                  <Button size="sm" variant="outline" onClick={retry}>
                    Retry
                  </Button>
                </div>
              )}
              {form && (
                <>
                  <div className="flex flex-col gap-half">
                    <Label htmlFor="intake-title">Title</Label>
                    <Input
                      id="intake-title"
                      value={form.title}
                      onChange={(e) => set({ title: e.target.value })}
                    />
                  </div>
                  <div className="flex flex-col gap-half">
                    <Label htmlFor="intake-description">Description</Label>
                    <Textarea
                      id="intake-description"
                      rows={6}
                      value={form.description}
                      onChange={(e) => set({ description: e.target.value })}
                    />
                  </div>
                  <div className="flex flex-col gap-half">
                    <Label htmlFor="intake-criteria">
                      Acceptance criteria (one per line)
                    </Label>
                    <Textarea
                      id="intake-criteria"
                      rows={3}
                      value={form.criteria}
                      onChange={(e) => set({ criteria: e.target.value })}
                    />
                  </div>
                  <div className="flex flex-col gap-half">
                    <Label htmlFor="intake-questions">
                      Open questions (one per line)
                    </Label>
                    <Textarea
                      id="intake-questions"
                      rows={2}
                      value={form.questions}
                      onChange={(e) => set({ questions: e.target.value })}
                    />
                  </div>
                  <div className="grid grid-cols-[2fr_1fr_1fr] gap-base">
                    <div className="flex flex-col gap-half">
                      <Label htmlFor="intake-external-id">External ID</Label>
                      <Input
                        id="intake-external-id"
                        value={form.externalId}
                        onChange={(e) => set({ externalId: e.target.value })}
                      />
                    </div>
                    <div className="flex flex-col gap-half">
                      <Label htmlFor="intake-priority">Priority</Label>
                      <select
                        id="intake-priority"
                        className="h-9 rounded-sm border bg-primary px-base text-sm"
                        value={form.priority}
                        onChange={(e) =>
                          set({
                            priority: e.target.value as IssuePriority | '',
                          })
                        }
                      >
                        <option value="">None</option>
                        {PRIORITIES.map((p) => (
                          <option key={p} value={p}>
                            {p}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="flex flex-col gap-half">
                      <Label htmlFor="intake-mode">Agent mode</Label>
                      <select
                        id="intake-mode"
                        className="h-9 rounded-sm border bg-primary px-base text-sm"
                        value={form.mode}
                        onChange={(e) =>
                          set({ mode: e.target.value as IssueMode })
                        }
                      >
                        <option value="ask">Ask</option>
                        <option value="yolo">Yolo</option>
                      </select>
                    </div>
                  </div>

                  {existing && (
                    <div
                      className="flex flex-col gap-half rounded-sm border border-brand/40 bg-brand/10 p-base text-sm"
                      data-testid="existing-ticket"
                    >
                      <p>
                        <strong>{view.existing_internal_id}</strong> already has
                        the External ID <strong>{existing.external_id}</strong>:
                        “{existing.title}”.
                      </p>
                      <label className="flex items-center gap-half">
                        <input
                          type="radio"
                          name="intake-target"
                          checked={target === 'update'}
                          onChange={() => setTarget('update')}
                        />
                        Update {view.existing_internal_id} (new description
                        version)
                      </label>
                      <label className="flex items-center gap-half">
                        <input
                          type="radio"
                          name="intake-target"
                          checked={target === 'create'}
                          onChange={() => setTarget('create')}
                        />
                        Create a new ticket
                      </label>
                      {updating && !duplicateTarget && (
                        <TextDiffView
                          oldText={existing.description ?? ''}
                          newText={composed}
                        />
                      )}
                    </div>
                  )}

                  {duplicates.filter((d) => d.ticket.id !== existing?.id)
                    .length > 0 && (
                    <div
                      className="flex flex-col gap-half rounded-sm border p-base text-sm"
                      data-testid="possible-duplicates"
                    >
                      <p className="font-medium">Possible duplicates</p>
                      {duplicates
                        .filter((d) => d.ticket.id !== existing?.id)
                        .map((d) => (
                          <div
                            key={d.ticket.id}
                            className={
                              duplicateTarget?.issue.id === d.ticket.id
                                ? 'rounded-sm ring-1 ring-brand'
                                : ''
                            }
                          >
                            <TicketResultLine
                              ticket={d.ticket}
                              excerpt={d.excerpt}
                              onOpen={() => void pickDuplicate(d)}
                            />
                          </div>
                        ))}
                      <p className="text-xs text-low">
                        {duplicateTarget
                          ? `This will add a new description version to ${duplicateTarget.internalId}. Click it again to create a new ticket instead.`
                          : 'Click one to add this as a new version of it.'}
                      </p>
                      {duplicateTarget && (
                        <TextDiffView
                          oldText={duplicateTarget.issue.description ?? ''}
                          newText={composed}
                        />
                      )}
                    </div>
                  )}
                </>
              )}
            </section>
          </div>
        )}

        {error && <p className="text-sm text-error">{error}</p>}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={cancel}>
            {view ? 'Discard' : 'Cancel'}
          </Button>
          {!view ? (
            <Button onClick={() => void start()} disabled={busy}>
              {busy ? 'Reading…' : 'Enhance'}
            </Button>
          ) : (
            <Button
              onClick={() => void accept()}
              disabled={busy || !form}
              data-testid="intake-accept"
            >
              {updateTarget
                ? `Update ${updateTarget.internalId}`
                : 'Create ticket'}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
});

export const DropTicketDialog = defineModal<
  DropTicketDialogProps,
  Issue | null
>(DropTicketDialogImpl);
