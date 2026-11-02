import { useState, useCallback, useMemo, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@kanbain/ui/components/KeyboardDialog';
import { Button } from '@kanbain/ui/components/Button';
import { Label } from '@kanbain/ui/components/Label';
import { Switch } from '@kanbain/ui/components/Switch';
import { useAgentForRole } from '@/shared/hooks/useAgentLibrary';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { sessionsApi } from '@/shared/lib/api';
import { useQueryClient } from '@tanstack/react-query';
import { create, useModal } from '@ebay/nice-modal-react';
import { defineModal } from '@/shared/lib/modals';
import { buildResolveConflictsInstructions } from '@/shared/lib/conflicts';
import { RUNTIME_EXECUTORS } from '@/shared/lib/agents';
import { toPrettyCase } from '@/shared/lib/string';
import type { ConflictOp } from 'shared/types';

export interface ResolveConflictsDialogProps {
  workspaceId: string;
  conflictOp: ConflictOp;
  sourceBranch: string | null;
  targetBranch: string;
  conflictedFiles: string[];
  repoName?: string;
}

export type ResolveConflictsDialogResult =
  { action: 'resolved'; sessionId?: string } | { action: 'cancelled' };

const ResolveConflictsDialogImpl = create<ResolveConflictsDialogProps>(
  ({
    workspaceId,
    conflictOp,
    sourceBranch,
    targetBranch,
    conflictedFiles,
    repoName,
  }) => {
    const modal = useModal();
    const queryClient = useQueryClient();
    const {
      workspaceId: activeWorkspaceId,
      selectedSessionId,
      selectSession,
    } = useWorkspaceContext();
    const { t } = useTranslation(['tasks', 'common']);
    // Conflict resolution runs as the conflict_resolver agent (Settings → Agents).
    const { agent, isLoading: agentsLoading } =
      useAgentForRole('conflict_resolver');
    const agentExecutor = agent?.definition
      ? RUNTIME_EXECUTORS[agent.definition.runtime]
      : null;

    // Auto-dismiss when the user switches to a different workspace
    useEffect(() => {
      if (activeWorkspaceId && activeWorkspaceId !== workspaceId) {
        modal.resolve({
          action: 'cancelled',
        } as ResolveConflictsDialogResult);
        modal.hide();
      }
    }, [activeWorkspaceId, workspaceId, modal]);

    // Default to creating a new session if no existing session
    const [createNewSession, setCreateNewSession] =
      useState(!selectedSessionId);
    const [isSubmitting, setIsSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const canSubmit = Boolean(
      agent?.definition && agentExecutor && !isSubmitting
    );

    // Build the conflict resolution instructions
    const conflictInstructions = useMemo(
      () =>
        buildResolveConflictsInstructions(
          sourceBranch,
          targetBranch,
          conflictedFiles,
          conflictOp,
          repoName
        ),
      [sourceBranch, targetBranch, conflictedFiles, conflictOp, repoName]
    );

    const handleSubmit = useCallback(async () => {
      if (!agent?.definition || !agentExecutor) return;

      setIsSubmitting(true);
      setError(null);

      try {
        let targetSessionId = selectedSessionId;
        const creatingNewSession = createNewSession || !selectedSessionId;

        // Create new session if user selected that option or no existing session
        if (creatingNewSession) {
          const session = await sessionsApi.create({
            workspace_id: workspaceId,
            executor: agentExecutor,
            name: t('resolveConflicts.dialog.sessionName'),
          });
          targetSessionId = session.id;
        }

        if (!targetSessionId) {
          setError('Failed to create session');
          setIsSubmitting(false);
          return;
        }

        // Send follow-up with conflict resolution instructions; the agent
        // definition decides runtime, model and prompts.
        await sessionsApi.followUp(targetSessionId, {
          prompt: conflictInstructions,
          executor_config: {
            executor: agentExecutor,
            agent_definition: agent.name,
          },
          retry_process_id: null,
          force_when_dirty: null,
          perform_git_reset: null,
        });

        // Invalidate queries and wait for them to complete
        await Promise.all([
          queryClient.invalidateQueries({
            queryKey: workspaceSessionKeys.byWorkspace(workspaceId),
          }),
          queryClient.invalidateQueries({
            queryKey: ['processes', workspaceId],
          }),
          queryClient.invalidateQueries({
            queryKey: ['branchStatus', workspaceId],
          }),
        ]);

        // Navigate to the new session if one was created
        // Do this after queries are refreshed so the session exists in the list
        if (creatingNewSession && targetSessionId) {
          selectSession(targetSessionId);
        }

        modal.resolve({
          action: 'resolved',
          sessionId: creatingNewSession ? targetSessionId : undefined,
        } as ResolveConflictsDialogResult);
        modal.hide();
      } catch (err) {
        console.error('Failed to resolve conflicts:', err);
        setError('Failed to start conflict resolution. Please try again.');
      } finally {
        setIsSubmitting(false);
      }
    }, [
      agent,
      agentExecutor,
      selectedSessionId,
      createNewSession,
      workspaceId,
      conflictInstructions,
      queryClient,
      selectSession,
      modal,
      t,
    ]);

    const handleCancel = useCallback(() => {
      modal.resolve({ action: 'cancelled' } as ResolveConflictsDialogResult);
      modal.hide();
    }, [modal]);

    const handleOpenChange = (open: boolean) => {
      if (!open) handleCancel();
    };

    const handleNewSessionChange = (checked: boolean) => {
      setCreateNewSession(checked);
    };

    const hasExistingSession = Boolean(selectedSessionId);

    return (
      <Dialog open={modal.visible} onOpenChange={handleOpenChange}>
        <DialogContent className="sm:max-w-[500px]">
          <DialogHeader>
            <DialogTitle>
              {t('resolveConflicts.dialog.title', 'Resolve Conflicts')}
            </DialogTitle>
            <DialogDescription>
              {t(
                'resolveConflicts.dialog.description',
                'Conflicts were detected. Choose how you want the agent to resolve them.'
              )}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            {/* Conflict summary */}
            <div className="rounded-md border border-warning/40 bg-warning/10 p-3 text-sm">
              <p className="font-medium text-warning-foreground dark:text-warning">
                {t('resolveConflicts.dialog.filesWithConflicts', {
                  count: conflictedFiles.length,
                })}
              </p>
              {conflictedFiles.length > 0 && (
                <ul className="mt-2 space-y-1 text-xs text-warning-foreground/80 dark:text-warning/80">
                  {conflictedFiles.slice(0, 5).map((file) => (
                    <li key={file} className="truncate">
                      {file}
                    </li>
                  ))}
                  {conflictedFiles.length > 5 && (
                    <li className="text-warning-foreground/60 dark:text-warning/60">
                      {t('resolveConflicts.dialog.andMore', {
                        count: conflictedFiles.length - 5,
                      })}
                    </li>
                  )}
                </ul>
              )}
            </div>

            {error && <div className="text-sm text-destructive">{error}</div>}

            {/* The conflict_resolver agent decides runtime, model and prompts;
                edit it in Settings → Agents. */}
            <div className="space-y-1">
              <Label className="text-sm font-medium">
                {t('resolveConflicts.dialog.agentLabel', 'Agent')}
              </Label>
              {agentsLoading ? (
                <p className="text-sm text-muted-foreground">
                  {t(
                    'resolveConflicts.dialog.loadingAgents',
                    'Loading agents…'
                  )}
                </p>
              ) : agent?.definition ? (
                <p className="text-sm text-muted-foreground">
                  {t('resolveConflicts.dialog.agentDetail', {
                    name: agent.name,
                    runtime: toPrettyCase(agentExecutor ?? ''),
                    model: agent.definition.model,
                    defaultValue: `${agent.name} · ${toPrettyCase(
                      agentExecutor ?? ''
                    )} · ${agent.definition.model}`,
                  })}
                </p>
              ) : (
                <p className="text-sm text-destructive">
                  {t(
                    'resolveConflicts.dialog.noAgent',
                    'No agent with the conflict_resolver role. Create one in Settings → Agents.'
                  )}
                </p>
              )}
            </div>
          </div>

          <DialogFooter className="sm:!justify-between">
            <Button
              variant="outline"
              onClick={handleCancel}
              disabled={isSubmitting}
            >
              {t('common:buttons.cancel')}
            </Button>
            <div className="flex items-center gap-3">
              {hasExistingSession && (
                <div className="flex items-center gap-2">
                  <Switch
                    id="new-session-switch"
                    checked={createNewSession}
                    onCheckedChange={handleNewSessionChange}
                    aria-label={t(
                      'resolveConflicts.dialog.newSession',
                      'New Session'
                    )}
                  />
                  <Label
                    htmlFor="new-session-switch"
                    className="text-sm cursor-pointer"
                  >
                    {t('resolveConflicts.dialog.newSession', 'New Session')}
                  </Label>
                </div>
              )}
              <Button onClick={handleSubmit} disabled={!canSubmit}>
                {isSubmitting
                  ? t('resolveConflicts.dialog.resolving', 'Starting...')
                  : t('resolveConflicts.dialog.resolve', 'Resolve Conflicts')}
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }
);

export const ResolveConflictsDialog = defineModal<
  ResolveConflictsDialogProps,
  ResolveConflictsDialogResult
>(ResolveConflictsDialogImpl);
