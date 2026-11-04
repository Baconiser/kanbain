import {
  ArrowClockwiseIcon,
  CheckIcon,
  SpinnerIcon,
  XIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import type { ConflictResolution } from 'shared/types';
import { cn } from '@/shared/lib/utils';

// A queued hand-off whose run never showed up within this window is dead:
// the follow-up failed or the dialog got closed mid-flight.
const QUEUED_STALE_MS = 2 * 60 * 1000;

type StepState = 'done' | 'active' | 'failed' | 'todo';

const STEPS = ['found', 'handoff', 'rebasing', 'syncing', 'ready'] as const;
type Step = (typeof STEPS)[number];

/** Which step the attempt is on (or died on). */
export function currentStep(resolution: ConflictResolution): Step {
  switch (resolution.stage) {
    case 'queued':
      return 'handoff';
    case 'rebasing':
      return 'rebasing';
    case 'syncing':
      return 'syncing';
    case 'resolved':
      return 'ready';
    case 'failed':
      if (!resolution.execution_process_id) return 'handoff';
      return resolution.error?.startsWith('Bringing') ? 'syncing' : 'rebasing';
  }
}

export function isQueuedStale(
  resolution: ConflictResolution,
  now = Date.now()
): boolean {
  if (resolution.stage !== 'queued') return false;
  const created = Date.parse(resolution.created_at);
  return Number.isFinite(created) && now - created > QUEUED_STALE_MS;
}

function stepStates(resolution: ConflictResolution): StepState[] {
  const at = STEPS.indexOf(currentStep(resolution));
  const failed = resolution.stage === 'failed' || isQueuedStale(resolution);
  return STEPS.map((_, i) => {
    if (i < at) return 'done';
    if (i > at) return 'todo';
    if (failed) return 'failed';
    return resolution.stage === 'resolved' ? 'done' : 'active';
  });
}

interface ConflictResolutionProgressProps {
  resolution: ConflictResolution;
  branch: string | null;
  onRetry?: () => void;
  onDismiss?: () => void;
}

export function ConflictResolutionProgress({
  resolution,
  branch,
  onRetry,
  onDismiss,
}: ConflictResolutionProgressProps) {
  const { t } = useTranslation('tasks');
  const states = stepStates(resolution);
  const stale = isQueuedStale(resolution);
  const target = resolution.target_branch;
  const files = resolution.predicted_files;
  const finished =
    resolution.stage === 'resolved' || resolution.stage === 'failed' || stale;

  let detail: string;
  if (stale) {
    detail = t('resolveConflicts.progress.detail.stale');
  } else {
    switch (resolution.stage) {
      case 'queued':
        detail = t('resolveConflicts.progress.detail.queued');
        break;
      case 'rebasing':
        detail = t('resolveConflicts.progress.detail.rebasing', {
          branch: branch ?? t('resolveConflicts.progress.yourBranch'),
          target,
          count: files.length,
        });
        break;
      case 'syncing':
        detail = t('resolveConflicts.progress.detail.syncing');
        break;
      case 'resolved':
        detail = t('resolveConflicts.progress.detail.resolved', { target });
        break;
      case 'failed':
        detail = resolution.error ?? t('resolveConflicts.progress.failed');
        break;
    }
  }

  return (
    <div className="bg-secondary border-b px-double py-base flex flex-col gap-half">
      <div className="flex items-center gap-base min-w-0">
        <ol className="flex items-center gap-half min-w-0 flex-1 overflow-hidden">
          {STEPS.map((step, i) => (
            <li
              key={step}
              className="flex items-center gap-half min-w-0 text-xs"
              aria-current={states[i] === 'active' ? 'step' : undefined}
            >
              {i > 0 && (
                <span
                  className={cn(
                    'h-px w-3 flex-shrink-0',
                    states[i] === 'todo' ? 'bg-border' : 'bg-brand'
                  )}
                />
              )}
              <StepDot state={states[i]} />
              <span
                className={cn(
                  'truncate',
                  states[i] === 'active' && 'text-normal font-medium',
                  states[i] === 'done' && 'text-low',
                  states[i] === 'failed' && 'text-error font-medium',
                  states[i] === 'todo' && 'text-low opacity-60'
                )}
              >
                {t(`resolveConflicts.progress.steps.${step}`, { target })}
              </span>
            </li>
          ))}
        </ol>
        {finished && resolution.stage !== 'resolved' && onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="flex items-center gap-half text-xs text-brand hover:underline flex-shrink-0"
          >
            <ArrowClockwiseIcon className="size-icon-xs" />
            {t('resolveConflicts.progress.retry')}
          </button>
        )}
        {finished && onDismiss && (
          <button
            type="button"
            onClick={onDismiss}
            className="text-low hover:text-normal p-1 -m-1 flex-shrink-0"
            title={t('resolveConflicts.progress.dismiss')}
          >
            <XIcon className="size-icon-xs" />
          </button>
        )}
      </div>
      <p
        className={cn(
          'text-xs',
          resolution.stage === 'failed' || stale ? 'text-error' : 'text-low'
        )}
      >
        {detail}
        {resolution.stage === 'rebasing' && files.length > 0 && (
          <span className="text-low"> {files.slice(0, 3).join(', ')}</span>
        )}
        {resolution.stage === 'rebasing' && files.length > 3 && (
          <span className="text-low">
            {' '}
            {t('resolveConflicts.dialog.andMore', { count: files.length - 3 })}
          </span>
        )}
      </p>
    </div>
  );
}

function StepDot({ state }: { state: StepState }) {
  const base =
    'flex items-center justify-center rounded-full size-4 flex-shrink-0';
  switch (state) {
    case 'done':
      return (
        <span className={cn(base, 'bg-brand text-white')}>
          <CheckIcon weight="bold" className="size-2.5" />
        </span>
      );
    case 'active':
      return (
        <span className={cn(base, 'text-brand')}>
          <SpinnerIcon className="size-4 animate-spin" />
        </span>
      );
    case 'failed':
      return (
        <span className={cn(base, 'bg-error text-white')}>
          <XIcon weight="bold" className="size-2.5" />
        </span>
      );
    case 'todo':
      return <span className={cn(base, 'border border-border')} />;
  }
}
