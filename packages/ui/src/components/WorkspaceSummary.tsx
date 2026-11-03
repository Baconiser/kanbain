import {
  PushPinIcon,
  HandIcon,
  TriangleIcon,
  PlayIcon,
  FileIcon,
  CircleIcon,
  GitPullRequestIcon,
  DotsThreeIcon,
  CheckIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { RunningDots } from './RunningDots';

const formatRelativeElapsed = (dateString: string): string => {
  const date = new Date(dateString);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffSecs = Math.floor(diffMs / 1000);
  const diffMins = Math.floor(diffSecs / 60);
  const diffHours = Math.floor(diffMins / 60);
  const diffDays = Math.floor(diffHours / 24);

  if (diffSecs < 60) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  return `${diffDays}d ago`;
};

/** The one thing a workspace waits for; the row shows only this. */
export type WorkspaceNextStep =
  | 'approve'
  | 'working'
  | 'retry'
  | 'review'
  | 'check'
  | 'archive'
  | 'none';

/** Steps with a one-click action on the row. */
export const ACTIONABLE_STEPS: ReadonlySet<WorkspaceNextStep> = new Set([
  'retry',
  'archive',
]);

export interface WorkspaceSummaryProps {
  name: string;
  workspaceId?: string;
  filesChanged?: number;
  linesAdded?: number;
  linesRemoved?: number;
  isActive?: boolean;
  isRunning?: boolean;
  isPinned?: boolean;
  hasPendingApproval?: boolean;
  hasRunningDevServer?: boolean;
  hasUnseenActivity?: boolean;
  latestProcessCompletedAt?: string;
  latestProcessStatus?: 'running' | 'completed' | 'failed' | 'killed';
  prStatus?: 'open' | 'merged' | 'closed' | 'unknown';
  onClick?: () => void;
  className?: string;
  summary?: boolean;
  /** Whether this is a draft workspace (shows "Draft" instead of elapsed time) */
  isDraft?: boolean;
  onOpenWorkspaceActions?: (workspaceId: string) => void;
  /** Next step from the server; derived from the flags above when absent */
  nextStep?: WorkspaceNextStep;
  /** Colour of the workspace's ticket, shown as the left stripe */
  accentColor?: string;
  /** Runs the next step (retry, archive); shown on hover */
  onNextStep?: (workspaceId: string, step: WorkspaceNextStep) => void;
}

function deriveNextStep({
  isRunning,
  hasPendingApproval,
  hasUnseenActivity,
  isFailed,
}: {
  isRunning: boolean;
  hasPendingApproval: boolean;
  hasUnseenActivity: boolean;
  isFailed: boolean;
}): WorkspaceNextStep {
  if (hasPendingApproval) return 'approve';
  if (isRunning) return 'working';
  if (isFailed) return 'retry';
  if (hasUnseenActivity) return 'check';
  return 'none';
}

export function WorkspaceSummary({
  name,
  workspaceId,
  filesChanged,
  linesAdded,
  linesRemoved,
  isActive = false,
  isRunning = false,
  isPinned = false,
  hasPendingApproval = false,
  hasRunningDevServer = false,
  hasUnseenActivity = false,
  latestProcessCompletedAt,
  latestProcessStatus,
  prStatus,
  onClick,
  className,
  summary = false,
  isDraft = false,
  onOpenWorkspaceActions,
  nextStep,
  accentColor,
  onNextStep,
}: WorkspaceSummaryProps) {
  const { t } = useTranslation('common');
  const isFailed =
    latestProcessStatus === 'failed' || latestProcessStatus === 'killed';
  const step =
    nextStep ??
    deriveNextStep({
      isRunning,
      hasPendingApproval,
      hasUnseenActivity,
      isFailed,
    });
  // Merged work diffs against a branch that moved on: noise.
  const hasChanges =
    step !== 'archive' && filesChanged !== undefined && filesChanged > 0;
  const stepAction =
    workspaceId && onNextStep && ACTIONABLE_STEPS.has(step) ? step : null;

  const handleNextStep = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!workspaceId || !stepAction) return;
    onNextStep?.(workspaceId, stepAction);
  };

  const handleOpenCommandBar = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!workspaceId || !onOpenWorkspaceActions) return;
    onOpenWorkspaceActions(workspaceId);
  };

  return (
    <div
      className={cn(
        'group relative rounded-sm transition-all duration-100 overflow-hidden',
        isActive ? 'bg-tertiary' : '',
        className
      )}
    >
      {/* Ticket colour stripe, or the selection tab without a ticket */}
      <div
        className={cn(
          'absolute left-0 top-1 bottom-1 w-0.5 rounded-full transition-colors duration-100',
          !accentColor && (isActive ? 'bg-brand' : 'bg-transparent')
        )}
        style={accentColor ? { backgroundColor: accentColor } : undefined}
      />
      <button
        onClick={onClick}
        className={cn(
          'flex w-full cursor-pointer flex-col text-left px-base py-half transition-all duration-150',
          isActive
            ? 'text-normal'
            : 'text-low sm:opacity-60 sm:hover:opacity-100 sm:hover:text-normal'
        )}
      >
        <div
          className={cn(
            'overflow-hidden whitespace-nowrap pr-double',
            !summary && 'text-normal'
          )}
          style={{
            maskImage:
              'linear-gradient(to right, black calc(100% - 24px), transparent 100%)',
            WebkitMaskImage:
              'linear-gradient(to right, black calc(100% - 24px), transparent 100%)',
          }}
        >
          {name}
        </div>
        {(!summary || isActive) && (
          <div className="flex w-full items-center gap-base text-sm h-5">
            {/* Dev server running - leftmost */}
            {hasRunningDevServer && (
              <PlayIcon
                className="size-icon-xs text-brand shrink-0"
                weight="fill"
              />
            )}

            {/* The next step: one icon + one word */}
            <NextStepBadge step={step} />

            {/* PR status icon */}
            {prStatus === 'open' && (
              <GitPullRequestIcon
                className="size-icon-xs text-success shrink-0"
                weight="fill"
              />
            )}
            {prStatus === 'merged' && (
              <GitPullRequestIcon
                className="size-icon-xs text-merged shrink-0"
                weight="fill"
              />
            )}

            {/* Pin icon */}
            {isPinned && (
              <PushPinIcon
                className="size-icon-xs text-brand shrink-0"
                weight="fill"
              />
            )}

            {/* Time elapsed OR "Draft" label (when not running) */}
            {step !== 'working' &&
              (isDraft ? (
                <span className="min-w-0 flex-1 truncate">
                  {t('workspaces.draft')}
                </span>
              ) : latestProcessCompletedAt ? (
                <span className="min-w-0 flex-1 truncate">
                  {formatRelativeElapsed(latestProcessCompletedAt)}
                </span>
              ) : (
                <span className="flex-1" />
              ))}

            {/* Spacer when running (no elapsed time shown) */}
            {step === 'working' && <span className="flex-1" />}

            {/* File count + lines changed on the right */}
            {hasChanges && (
              <span className="shrink-0 text-right flex items-center gap-half">
                <FileIcon className="size-icon-xs" weight="fill" />
                <span>{filesChanged}</span>
                {linesAdded !== undefined && (
                  <span className="text-success">+{linesAdded}</span>
                )}
                {linesRemoved !== undefined && (
                  <span className="text-error">-{linesRemoved}</span>
                )}
              </span>
            )}
          </div>
        )}
      </button>

      {/* Right-side hover action - more options only */}
      {workspaceId && onOpenWorkspaceActions && (
        <div className="absolute right-0 top-0 bottom-0 flex items-center sm:opacity-0 sm:group-hover:opacity-100">
          {/* Gradient fade from transparent to background */}
          <div className="h-full w-6 pointer-events-none bg-gradient-to-r from-transparent to-secondary" />
          <div className="flex items-center gap-half pr-base h-full bg-secondary">
            {stepAction && (
              <button
                onClick={handleNextStep}
                onPointerDown={(e) => e.stopPropagation()}
                className="rounded-sm px-1.5 py-0.5 text-xs text-normal bg-tertiary hover:bg-brand hover:text-high"
              >
                {t(`workspaces.stepAction.${stepAction}`)}
              </button>
            )}
            <button
              onClick={handleOpenCommandBar}
              onPointerDown={(e) => e.stopPropagation()}
              className="p-1.5 rounded-sm text-low hover:text-normal hover:bg-tertiary"
              title={t('workspaces.more')}
            >
              <DotsThreeIcon className="size-5" weight="bold" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function NextStepBadge({ step }: { step: WorkspaceNextStep }) {
  const { t } = useTranslation('common');
  const label = (key: WorkspaceNextStep) => (
    <span className="shrink-0">{t(`workspaces.step.${key}`)}</span>
  );
  switch (step) {
    case 'approve':
      return (
        <span className="flex shrink-0 items-center gap-half text-brand">
          <HandIcon className="size-icon-xs" weight="fill" />
          {label(step)}
        </span>
      );
    case 'working':
      return <RunningDots />;
    case 'retry':
      return (
        <span className="flex shrink-0 items-center gap-half text-error">
          <TriangleIcon className="size-icon-xs" weight="fill" />
          {label(step)}
        </span>
      );
    case 'review':
      return (
        <span className="flex shrink-0 items-center gap-half text-brand">
          <GitPullRequestIcon className="size-icon-xs" weight="fill" />
          {label(step)}
        </span>
      );
    case 'check':
      return (
        <span className="flex shrink-0 items-center gap-half text-brand">
          <CircleIcon className="size-icon-xs" weight="fill" />
          {label(step)}
        </span>
      );
    case 'archive':
      return (
        <span className="flex shrink-0 items-center gap-half text-low">
          <CheckIcon className="size-icon-xs" weight="bold" />
          {label(step)}
        </span>
      );
    default:
      return null;
  }
}
