import { useMemo, useState } from 'react';
import {
  CheckCircleIcon,
  FileIcon,
  FlagIcon,
  ArrowBendDownRightIcon,
} from '@phosphor-icons/react';
import type { PlanCheckpoint, PlanDocument, PlanSubtask } from 'shared/types';
import { CollapsibleSection } from './CollapsibleSection';

interface Stage {
  subtasks: PlanSubtask[];
  checkpoints: PlanCheckpoint[];
}

/**
 * Groups subtasks into stages: a subtask runs one stage after the last of
 * its dependencies, so each stage's subtasks can run side by side.
 */
function toStages(plan: PlanDocument): Stage[] {
  const byId = new Map(plan.subtasks.map((s) => [s.id, s]));
  const levels = new Map<string, number>();
  const level = (s: PlanSubtask, visiting: Set<string>): number => {
    const known = levels.get(s.id);
    if (known !== undefined) return known;
    // A dependency cycle or unknown id doesn't push a subtask further down.
    visiting.add(s.id);
    let result = 0;
    for (const dep of s.depends_on) {
      const parent = byId.get(dep);
      if (!parent || visiting.has(dep)) continue;
      result = Math.max(result, level(parent, visiting) + 1);
    }
    visiting.delete(s.id);
    levels.set(s.id, result);
    return result;
  };

  const stages: Stage[] = [];
  for (const s of plan.subtasks) {
    const l = level(s, new Set());
    while (stages.length <= l) stages.push({ subtasks: [], checkpoints: [] });
    stages[l].subtasks.push(s);
  }
  // A checkpoint sits after the stage that finishes the last subtask it
  // waits for; one naming no known subtask goes at the end.
  for (const c of plan.checkpoints) {
    const after = c.after_subtasks
      .map((id) => levels.get(id))
      .filter((l): l is number => l !== undefined);
    const at = after.length ? Math.max(...after) : stages.length - 1;
    if (at >= 0) stages[at].checkpoints.push(c);
  }
  return stages;
}

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

function Stat({ value, label }: { value: number; label: string }) {
  return (
    <div className="flex flex-col items-center rounded-sm bg-secondary px-base py-half">
      <span className="text-lg font-semibold leading-tight text-high">
        {value}
      </span>
      <span className="text-xs text-low">{label}</span>
    </div>
  );
}

function StepCard({ subtask }: { subtask: PlanSubtask }) {
  const [open, setOpen] = useState(false);
  return (
    <li className="relative">
      <span className="absolute -left-6 top-half flex size-5 items-center justify-center rounded-full border border-brand/60 bg-primary font-mono text-[10px] text-brand">
        {subtask.id}
      </span>
      <button
        type="button"
        aria-expanded={open}
        className="flex w-full flex-col gap-half rounded-sm border bg-secondary/40 px-base py-half text-left hover:border-brand/40 hover:bg-secondary"
        onClick={() => setOpen((o) => !o)}
      >
        <span
          className="text-sm font-medium text-normal"
          data-testid="plan-subtask-title"
        >
          {subtask.title || 'Untitled step'}
        </span>
        {subtask.description && (
          <span
            className={`whitespace-pre-wrap text-xs text-low ${
              open ? '' : 'line-clamp-2'
            }`}
          >
            {subtask.description}
          </span>
        )}
        <span className="flex flex-wrap items-center gap-x-base gap-y-half text-xs text-low">
          {subtask.depends_on.length > 0 && (
            <span className="flex items-center gap-half">
              <ArrowBendDownRightIcon className="size-icon-xs" />
              after {subtask.depends_on.join(', ')}
            </span>
          )}
          {subtask.files.length > 0 && (
            <span className="flex items-center gap-half">
              <FileIcon className="size-icon-xs" />
              {plural(subtask.files.length, 'file')}
            </span>
          )}
        </span>
        {open && subtask.files.length > 0 && (
          <ul className="flex flex-col border-t pt-half font-mono text-xs text-low">
            {subtask.files.map((f) => (
              <li key={f} className="truncate" title={f}>
                {f}
              </li>
            ))}
          </ul>
        )}
      </button>
    </li>
  );
}

/** The plan drawn for reading: stages of steps, checkpoints, and goals. */
export function PlanOverview({ plan }: { plan: PlanDocument }) {
  const stages = useMemo(() => toStages(plan), [plan]);
  const files = useMemo(
    () => new Set(plan.subtasks.flatMap((s) => s.files)).size,
    [plan.subtasks]
  );

  return (
    <div className="flex flex-col gap-double" data-testid="plan-overview">
      <div className="grid grid-cols-4 gap-half">
        <Stat value={plan.subtasks.length} label="steps" />
        <Stat value={stages.length} label="stages" />
        <Stat value={files} label="files" />
        <Stat value={plan.checkpoints.length} label="checks" />
      </div>

      {plan.acceptance_criteria.length > 0 && (
        <CollapsibleSection
          id="plan-acceptance"
          title="Done when"
          meta={plan.acceptance_criteria.length}
        >
          <ul className="flex flex-col gap-half">
            {plan.acceptance_criteria.map((c, i) => (
              <li key={i} className="flex items-start gap-half text-sm">
                <CheckCircleIcon
                  weight="fill"
                  className="mt-[3px] size-icon-xs shrink-0 text-success"
                />
                <span>{c}</span>
              </li>
            ))}
          </ul>
        </CollapsibleSection>
      )}

      <CollapsibleSection
        id="plan-steps"
        title="Steps"
        meta={`${plural(plan.subtasks.length, 'step')} in ${plural(
          stages.length,
          'stage'
        )}`}
      >
        <ol className="flex flex-col gap-base">
          {stages.map((stage, i) => (
            <li key={i} className="flex flex-col gap-half">
              <div className="flex items-center gap-base text-xs text-low">
                <span className="font-medium uppercase tracking-wide">
                  Stage {i + 1}
                </span>
                {stage.subtasks.length > 1 && (
                  <span>{stage.subtasks.length} side by side</span>
                )}
                <span className="h-px flex-1 bg-border" />
              </div>
              {/* The rail: steps hang off a line, numbered on it. */}
              <ul className="relative ml-[10px] flex flex-col gap-half border-l-2 border-dotted border-brand/50 pl-[14px]">
                {stage.subtasks.map((s) => (
                  <StepCard key={s.id} subtask={s} />
                ))}
                {stage.checkpoints.map((c, j) => (
                  <li
                    key={`c${j}`}
                    className="relative flex items-start gap-half rounded-sm border border-dashed border-success/50 bg-success/5 px-base py-half text-xs"
                  >
                    <span className="absolute -left-6 top-half flex size-5 items-center justify-center rounded-full border border-success/60 bg-primary">
                      <FlagIcon weight="fill" className="size-3 text-success" />
                    </span>
                    <span>
                      <span className="font-medium text-success">
                        Checkpoint
                      </span>{' '}
                      <span className="text-normal">{c.what}</span>
                    </span>
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ol>
      </CollapsibleSection>

      {plan.assumptions.length > 0 && (
        <CollapsibleSection
          id="plan-assumptions"
          title="Assumptions"
          meta={plan.assumptions.length}
          defaultOpen={false}
        >
          <ul className="flex list-disc flex-col gap-half pl-double text-sm text-normal">
            {plan.assumptions.map((a, i) => (
              <li key={i}>{a}</li>
            ))}
          </ul>
        </CollapsibleSection>
      )}

      {plan.test_strategy.trim() && (
        <CollapsibleSection
          id="plan-test-strategy"
          title="Test strategy"
          defaultOpen={false}
        >
          <p className="whitespace-pre-wrap text-sm text-normal">
            {plan.test_strategy}
          </p>
        </CollapsibleSection>
      )}
    </div>
  );
}
