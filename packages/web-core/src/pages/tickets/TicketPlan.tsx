import { useCallback, useEffect, useMemo, useState } from 'react';
import { CaretRightIcon, PlusIcon, TrashIcon } from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { Input } from '@kanbain/ui/components/Input';
import { Textarea } from '@kanbain/ui/components/Textarea';
import type {
  Plan,
  PlanCheckpoint,
  PlanDocument,
  PlanSubtask,
  PlanningView,
} from 'shared/types';
import { planningApi } from '@/shared/lib/ticketsApi';

interface TicketPlanProps {
  issueId: string;
  /** Changes when planning events arrive, to reload. */
  refreshKey: number;
  onOpenWorkspace: (workspaceId: string) => void;
  /** Brings the comments, where the planner's questions are, into view. */
  onShowComments: () => void;
  onError: (message: string) => void;
}

const POLL_MS = 3000;

function errorMessage(err: unknown, fallback: string): string {
  return err instanceof Error ? err.message : fallback;
}

// Lists are edited as one item per line, id lists as comma-separated text.
const toLines = (items: string[]) => items.join('\n');
const fromLines = (text: string) =>
  text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
const toIds = (items: string[]) => items.join(', ');
const fromIds = (text: string) =>
  text
    .split(',')
    .map((l) => l.trim())
    .filter(Boolean);

function nextId(subtasks: PlanSubtask[]): string {
  const numbers = subtasks.map((s) => Number(s.id)).filter(Number.isFinite);
  return String(
    numbers.length ? Math.max(...numbers) + 1 : subtasks.length + 1
  );
}

function FieldLabel({ children }: { children: React.ReactNode }) {
  return (
    <span className="text-xs font-medium uppercase text-low">{children}</span>
  );
}

function ListField({
  label,
  value,
  readOnly,
  onChange,
  testId,
}: {
  label: string;
  value: string[];
  readOnly: boolean;
  onChange: (value: string[]) => void;
  testId: string;
}) {
  // Keep the raw text while typing so empty lines don't vanish mid-edit.
  const [text, setText] = useState(toLines(value));
  useEffect(() => {
    setText((current) =>
      toLines(fromLines(current)) === toLines(value) ? current : toLines(value)
    );
  }, [value]);
  return (
    <label className="flex flex-col gap-half">
      <FieldLabel>{label} (one per line)</FieldLabel>
      <Textarea
        data-testid={testId}
        value={text}
        readOnly={readOnly}
        rows={Math.max(2, value.length + 1)}
        onChange={(e) => {
          setText(e.target.value);
          onChange(fromLines(e.target.value));
        }}
      />
    </label>
  );
}

function SubtaskEditor({
  subtask,
  index,
  readOnly,
  open,
  onToggle,
  onChange,
  onRemove,
}: {
  subtask: PlanSubtask;
  index: number;
  readOnly: boolean;
  open: boolean;
  onToggle: () => void;
  onChange: (subtask: PlanSubtask) => void;
  onRemove: () => void;
}) {
  const [files, setFiles] = useState(toLines(subtask.files));
  const [deps, setDeps] = useState(toIds(subtask.depends_on));
  // Editors are keyed by position; follow the subtask when one above is
  // removed, without fighting the raw text being typed.
  useEffect(() => {
    setFiles((current) =>
      toLines(fromLines(current)) === toLines(subtask.files)
        ? current
        : toLines(subtask.files)
    );
  }, [subtask.files]);
  useEffect(() => {
    setDeps((current) =>
      toIds(fromIds(current)) === toIds(subtask.depends_on)
        ? current
        : toIds(subtask.depends_on)
    );
  }, [subtask.depends_on]);
  return (
    <li
      className="flex flex-col gap-half rounded-sm border p-base"
      data-testid={`plan-subtask-${index}`}
    >
      <div className="flex items-center gap-half">
        <button
          type="button"
          aria-label={open ? 'Fold subtask' : 'Unfold subtask'}
          aria-expanded={open}
          className="shrink-0 text-low hover:text-normal"
          onClick={onToggle}
        >
          <CaretRightIcon
            weight="bold"
            className={`size-icon-xs transition-transform ${open ? 'rotate-90' : ''}`}
          />
        </button>
        <Input
          aria-label="Subtask id"
          className="h-8 w-12 shrink-0 px-half font-mono text-xs"
          value={subtask.id}
          readOnly={readOnly}
          onChange={(e) => onChange({ ...subtask, id: e.target.value })}
        />
        <Input
          aria-label="Subtask title"
          data-testid="plan-subtask-title"
          className="h-8"
          title={subtask.title}
          value={subtask.title}
          readOnly={readOnly}
          onChange={(e) => onChange({ ...subtask, title: e.target.value })}
        />
        {!readOnly && (
          <button
            type="button"
            aria-label="Remove subtask"
            className="shrink-0 text-low hover:text-error"
            onClick={onRemove}
          >
            <TrashIcon className="size-icon-sm" />
          </button>
        )}
      </div>
      {!open && (
        <button
          type="button"
          className="flex flex-wrap gap-x-base pl-[1.25rem] text-left text-xs text-low hover:text-normal"
          onClick={onToggle}
        >
          {subtask.description && (
            <span className="w-full truncate">{subtask.description}</span>
          )}
          {subtask.depends_on.length > 0 && (
            <span>after {subtask.depends_on.join(', ')}</span>
          )}
          <span>
            {subtask.files.length} file{subtask.files.length === 1 ? '' : 's'}
          </span>
        </button>
      )}
      {open && (
        <Textarea
          aria-label="Subtask description"
          placeholder="What to do and why"
          value={subtask.description}
          readOnly={readOnly}
          rows={4}
          onChange={(e) =>
            onChange({ ...subtask, description: e.target.value })
          }
        />
      )}
      {open && (
        <div className="grid grid-cols-[2fr_1fr] gap-half">
          <label className="flex flex-col gap-half">
            <FieldLabel>Likely files (one per line)</FieldLabel>
            <Textarea
              className="font-mono text-xs"
              value={files}
              readOnly={readOnly}
              rows={Math.max(1, subtask.files.length)}
              onChange={(e) => {
                setFiles(e.target.value);
                onChange({ ...subtask, files: fromLines(e.target.value) });
              }}
            />
          </label>
          <label className="flex flex-col gap-half">
            <FieldLabel>Depends on</FieldLabel>
            <Input
              placeholder="e.g. 1, 2"
              value={deps}
              readOnly={readOnly}
              onChange={(e) => {
                setDeps(e.target.value);
                onChange({ ...subtask, depends_on: fromIds(e.target.value) });
              }}
            />
          </label>
        </div>
      )}
    </li>
  );
}

function CheckpointEditor({
  checkpoint,
  readOnly,
  onChange,
  onRemove,
}: {
  checkpoint: PlanCheckpoint;
  readOnly: boolean;
  onChange: (checkpoint: PlanCheckpoint) => void;
  onRemove: () => void;
}) {
  const [after, setAfter] = useState(toIds(checkpoint.after_subtasks));
  useEffect(() => {
    setAfter((current) =>
      toIds(fromIds(current)) === toIds(checkpoint.after_subtasks)
        ? current
        : toIds(checkpoint.after_subtasks)
    );
  }, [checkpoint.after_subtasks]);
  return (
    <li className="flex items-start gap-half">
      <Input
        aria-label="After subtasks"
        className="w-24 shrink-0"
        placeholder="after 1, 2"
        value={after}
        readOnly={readOnly}
        onChange={(e) => {
          setAfter(e.target.value);
          onChange({ ...checkpoint, after_subtasks: fromIds(e.target.value) });
        }}
      />
      <Input
        aria-label="What the tester checks"
        value={checkpoint.what}
        readOnly={readOnly}
        onChange={(e) => onChange({ ...checkpoint, what: e.target.value })}
      />
      {!readOnly && (
        <button
          type="button"
          aria-label="Remove checkpoint"
          className="mt-half shrink-0 text-low hover:text-error"
          onClick={onRemove}
        >
          <TrashIcon className="size-icon-sm" />
        </button>
      )}
    </li>
  );
}

/** The plan as an editable form (D21). */
function PlanForm({
  plan,
  readOnly,
  onChange,
}: {
  plan: PlanDocument;
  readOnly: boolean;
  onChange: (plan: PlanDocument) => void;
}) {
  // Subtasks start folded to one line each; positions, like the editors.
  const [openSubtasks, setOpenSubtasks] = useState<Set<number>>(new Set());
  const allOpen =
    plan.subtasks.length > 0 && openSubtasks.size >= plan.subtasks.length;
  const toggleSubtask = (i: number) =>
    setOpenSubtasks((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  const setSubtask = (i: number, subtask: PlanSubtask) =>
    onChange({
      ...plan,
      subtasks: plan.subtasks.map((s, j) => (j === i ? subtask : s)),
    });
  const setCheckpoint = (i: number, checkpoint: PlanCheckpoint) =>
    onChange({
      ...plan,
      checkpoints: plan.checkpoints.map((c, j) => (j === i ? checkpoint : c)),
    });
  return (
    <div className="flex flex-col gap-base" data-testid="plan-form">
      <div className="flex flex-col gap-half">
        <div className="flex items-center justify-between">
          <FieldLabel>Subtasks ({plan.subtasks.length})</FieldLabel>
          <div className="flex items-center gap-base">
            {plan.subtasks.length > 0 && (
              <button
                type="button"
                className="text-xs text-low hover:text-normal"
                onClick={() =>
                  setOpenSubtasks(
                    allOpen
                      ? new Set()
                      : new Set(plan.subtasks.map((_, i) => i))
                  )
                }
              >
                {allOpen ? 'Fold all' : 'Unfold all'}
              </button>
            )}
            {!readOnly && (
              <button
                type="button"
                className="flex items-center gap-half text-xs text-brand hover:underline"
                onClick={() => {
                  setOpenSubtasks((prev) =>
                    new Set(prev).add(plan.subtasks.length)
                  );
                  onChange({
                    ...plan,
                    subtasks: [
                      ...plan.subtasks,
                      {
                        id: nextId(plan.subtasks),
                        title: '',
                        description: '',
                        files: [],
                        depends_on: [],
                      },
                    ],
                  });
                }}
              >
                <PlusIcon className="size-icon-xs" /> Add subtask
              </button>
            )}
          </div>
        </div>
        <ol className="flex flex-col gap-half">
          {plan.subtasks.map((s, i) => (
            <SubtaskEditor
              // Index keys: ids are editable.
              key={i}
              index={i}
              subtask={s}
              readOnly={readOnly}
              open={openSubtasks.has(i)}
              onToggle={() => toggleSubtask(i)}
              onChange={(next) => setSubtask(i, next)}
              onRemove={() => {
                // Folds shift up with the subtasks below the removed one.
                setOpenSubtasks(
                  (prev) =>
                    new Set(
                      [...prev]
                        .filter((j) => j !== i)
                        .map((j) => (j > i ? j - 1 : j))
                    )
                );
                onChange({
                  ...plan,
                  subtasks: plan.subtasks.filter((_, j) => j !== i),
                });
              }}
            />
          ))}
        </ol>
      </div>
      <ListField
        label="Acceptance criteria"
        testId="plan-acceptance"
        value={plan.acceptance_criteria}
        readOnly={readOnly}
        onChange={(acceptance_criteria) =>
          onChange({ ...plan, acceptance_criteria })
        }
      />
      <ListField
        label="Assumptions"
        testId="plan-assumptions"
        value={plan.assumptions}
        readOnly={readOnly}
        onChange={(assumptions) => onChange({ ...plan, assumptions })}
      />
      <label className="flex flex-col gap-half">
        <FieldLabel>Test strategy</FieldLabel>
        <Textarea
          value={plan.test_strategy}
          readOnly={readOnly}
          rows={2}
          onChange={(e) => onChange({ ...plan, test_strategy: e.target.value })}
        />
      </label>
      <div className="flex flex-col gap-half">
        <div className="flex items-center justify-between">
          <FieldLabel>Checkpoints</FieldLabel>
          {!readOnly && (
            <button
              type="button"
              className="flex items-center gap-half text-xs text-brand hover:underline"
              onClick={() =>
                onChange({
                  ...plan,
                  checkpoints: [
                    ...plan.checkpoints,
                    { after_subtasks: [], what: '' },
                  ],
                })
              }
            >
              <PlusIcon className="size-icon-xs" /> Add checkpoint
            </button>
          )}
        </div>
        <ul className="flex flex-col gap-half">
          {plan.checkpoints.map((c, i) => (
            <CheckpointEditor
              key={i}
              checkpoint={c}
              readOnly={readOnly}
              onChange={(next) => setCheckpoint(i, next)}
              onRemove={() =>
                onChange({
                  ...plan,
                  checkpoints: plan.checkpoints.filter((_, j) => j !== i),
                })
              }
            />
          ))}
        </ul>
      </div>
    </div>
  );
}

function versionLabel(plan: Plan): string {
  const who = plan.source === 'edited' ? 'edited' : 'planner';
  return `v${plan.version} · ${who} · ${plan.status}`;
}

/**
 * The planning flow on a ticket (Phase 4): assign it to the planner, see
 * whether the planner works or waits for answers, and approve, edit or
 * send back its plan (D20, D21).
 */
export function TicketPlan({
  issueId,
  refreshKey,
  onOpenWorkspace,
  onShowComments,
  onError,
}: TicketPlanProps) {
  const [view, setView] = useState<PlanningView | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<PlanDocument | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    () =>
      planningApi
        .get(issueId)
        .then(setView)
        .catch((err: unknown) =>
          onError(errorMessage(err, 'Failed to load the plan'))
        ),
    [issueId, onError]
  );

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const status = view?.planning?.status ?? 'idle';
  useEffect(() => {
    if (status !== 'planning') return;
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [status, load]);

  const plans = useMemo(() => view?.plans ?? [], [view]);
  const latest = plans.length ? plans[plans.length - 1] : null;
  const shown = plans.find((p) => p.id === selected) ?? latest;

  // A new version replaces what the form shows (and any unsaved edit).
  useEffect(() => {
    setSelected(null);
    setDraft(null);
    setFeedback(null);
  }, [latest?.id]);

  const editable =
    !!shown &&
    shown.id === latest?.id &&
    shown.status === 'draft' &&
    status !== 'planning';
  const dirty =
    !!draft && !!shown && JSON.stringify(draft) !== JSON.stringify(shown.plan);

  const act = async (action: () => Promise<unknown>, fallback: string) => {
    setBusy(true);
    try {
      await action();
      await load();
    } catch (err) {
      onError(errorMessage(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  if (!view) return null;
  const planning = view.planning;
  const canStart = status !== 'planning' && status !== 'waiting_for_answer';

  return (
    <section className="flex flex-col gap-half" data-testid="ticket-plan">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-normal">Plan</h3>
        <div className="flex items-center gap-base">
          {planning?.workspace_id && (
            <button
              type="button"
              className="text-xs text-low hover:text-normal"
              onClick={() => onOpenWorkspace(planning.workspace_id!)}
            >
              Planner workspace
            </button>
          )}
          {canStart && (
            <Button
              size="xs"
              variant={latest ? 'outline' : 'default'}
              disabled={busy}
              data-testid="plan-start"
              onClick={() =>
                void act(
                  () => planningApi.start(issueId),
                  'Failed to start the planner'
                )
              }
            >
              {latest ? 'Plan again' : 'Plan with AI'}
            </Button>
          )}
        </div>
      </div>

      {status === 'planning' && (
        <p className="text-sm text-low" data-testid="planning-running">
          The planner is working on this ticket…
        </p>
      )}
      {status === 'waiting_for_answer' && (
        <div
          className="rounded-sm border border-brand/40 bg-brand/10 p-base text-sm"
          data-testid="planning-waiting"
        >
          <strong>Waiting for your answer.</strong> The planner asked questions
          in the{' '}
          <button
            type="button"
            className="text-brand hover:underline"
            onClick={onShowComments}
          >
            comments
          </button>
          ; reply there and it continues in the same session.
        </div>
      )}
      {status === 'failed' && (
        <p className="text-sm text-error" data-testid="planning-failed">
          Planning failed{planning?.error ? `: ${planning.error}` : ''}.
        </p>
      )}
      {!latest && status === 'idle' && (
        <p className="text-sm text-low">
          No plan yet. Assign the ticket to the planner, or drag it to Planning.
        </p>
      )}

      {shown && (
        <div className="flex flex-col gap-base rounded-sm border bg-secondary p-base">
          <div className="flex flex-wrap items-center gap-half">
            {plans.map((p) => (
              <button
                key={p.id}
                type="button"
                data-testid={`plan-version-${p.version}`}
                className={`rounded-sm border px-half text-xs ${
                  p.id === shown.id
                    ? 'border-brand text-normal'
                    : 'text-low hover:text-normal'
                }`}
                onClick={() => {
                  setSelected(p.id);
                  setDraft(null);
                  setFeedback(null);
                }}
              >
                {versionLabel(p)}
              </button>
            ))}
          </div>
          {shown.feedback && (
            <p className="text-sm text-low">
              Sent back: <span className="text-normal">{shown.feedback}</span>
            </p>
          )}

          <PlanForm
            // Remount per version so the form's raw text follows it.
            key={shown.id}
            plan={draft ?? shown.plan}
            readOnly={!editable}
            onChange={setDraft}
          />

          {editable && feedback === null && (
            <div className="flex flex-wrap gap-half">
              <Button
                size="sm"
                disabled={busy || dirty}
                title={dirty ? 'Save your edits as a new version first' : ''}
                data-testid="plan-approve"
                onClick={() =>
                  void act(
                    () => planningApi.approve(shown.id),
                    'Failed to approve the plan'
                  )
                }
              >
                Approve
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy || !dirty}
                data-testid="plan-save"
                onClick={() =>
                  draft &&
                  void act(
                    () => planningApi.edit(shown.id, draft),
                    'Failed to save the plan'
                  )
                }
              >
                Save as new version
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy || dirty}
                data-testid="plan-send-back"
                onClick={() => setFeedback('')}
              >
                Send back
              </Button>
            </div>
          )}
          {editable && feedback !== null && (
            <div className="flex flex-col gap-half">
              <Textarea
                aria-label="Feedback for the planner"
                data-testid="plan-feedback"
                placeholder="What should the planner change?"
                value={feedback}
                onChange={(e) => setFeedback(e.target.value)}
              />
              <div className="flex gap-half">
                <Button
                  size="sm"
                  disabled={busy || !feedback.trim()}
                  data-testid="plan-send-back-submit"
                  onClick={() =>
                    void act(
                      () => planningApi.sendBack(shown.id, feedback),
                      'Failed to send the plan back'
                    )
                  }
                >
                  Send to planner
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setFeedback(null)}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}
          {shown.status === 'approved' && (
            <p className="text-sm text-success" data-testid="plan-approved">
              Approved. Coding can start on this plan.
            </p>
          )}
        </div>
      )}
    </section>
  );
}
