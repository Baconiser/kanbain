import { useState, useCallback } from 'react';
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
import { Textarea } from '@kanbain/ui/components/Textarea';
import { Label } from '@kanbain/ui/components/Label';
import { Switch } from '@kanbain/ui/components/Switch';
import { Checkbox } from '@kanbain/ui/components/Checkbox';
import { useAgentForRole } from '@/shared/hooks/useAgentLibrary';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { sessionsApi } from '@/shared/lib/api';
import { useQueryClient } from '@tanstack/react-query';
import { create, useModal } from '@ebay/nice-modal-react';
import { defineModal } from '@/shared/lib/modals';
import { RUNTIME_EXECUTORS } from '@/shared/lib/agents';
import { toPrettyCase } from '@/shared/lib/string';

export interface StartReviewDialogProps {
  sessionId?: string;
  workspaceId: string;
  reviewMarkdown?: string;
  onSuccess?: (newSessionId?: string) => void;
}

const StartReviewDialogImpl = create<StartReviewDialogProps>(
  ({ sessionId, workspaceId, reviewMarkdown, onSuccess }) => {
    const modal = useModal();
    const queryClient = useQueryClient();
    const { selectedSessionId, selectSession } = useWorkspaceContext();
    const { t } = useTranslation(['tasks', 'common']);
    // Reviews run as the reviewer agent (Settings → Agents).
    const { agent, isLoading: agentsLoading } = useAgentForRole('reviewer');
    const agentExecutor = agent?.definition
      ? RUNTIME_EXECUTORS[agent.definition.runtime]
      : null;

    const resolvedSessionId = sessionId ?? selectedSessionId;

    const [additionalPrompt, setAdditionalPrompt] = useState('');
    const [createNewSession, setCreateNewSession] = useState(
      () => !resolvedSessionId
    );
    const [includeGitContext, setIncludeGitContext] = useState(true);
    const [isSubmitting, setIsSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const canSubmit = Boolean(
      agent?.definition && agentExecutor && !isSubmitting
    );

    const handleSubmit = useCallback(async () => {
      if (!agent?.definition || !agentExecutor) return;

      setIsSubmitting(true);
      setError(null);

      try {
        let targetSessionId = resolvedSessionId;

        if (createNewSession || !resolvedSessionId) {
          const session = await sessionsApi.create({
            workspace_id: workspaceId,
            executor: agentExecutor,
            name: t('startReviewDialog.sessionName'),
          });
          targetSessionId = session.id;

          queryClient.invalidateQueries({
            queryKey: workspaceSessionKeys.byWorkspace(workspaceId),
          });
        }

        if (!targetSessionId) {
          setError('Failed to create session');
          setIsSubmitting(false);
          return;
        }

        const promptParts = [reviewMarkdown, additionalPrompt].filter(Boolean);
        const combinedPrompt = promptParts.join('\n\n');

        await sessionsApi.startReview(targetSessionId, {
          executor_config: {
            executor: agentExecutor,
            agent_definition: agent.name,
          },
          additional_prompt: combinedPrompt || null,
          use_all_workspace_commits: includeGitContext,
        });

        queryClient.invalidateQueries({
          queryKey: ['processes', workspaceId],
        });
        queryClient.invalidateQueries({
          queryKey: ['branchStatus', workspaceId],
        });

        const createdNewSession = targetSessionId !== resolvedSessionId;
        if (createdNewSession && targetSessionId) {
          selectSession(targetSessionId);
        }
        onSuccess?.(createdNewSession ? targetSessionId : undefined);
        modal.hide();
      } catch (err) {
        console.error('Failed to start review:', err);
        setError('Failed to start review. Please try again.');
      } finally {
        setIsSubmitting(false);
      }
    }, [
      agent,
      agentExecutor,
      resolvedSessionId,
      workspaceId,
      createNewSession,
      includeGitContext,
      reviewMarkdown,
      additionalPrompt,
      queryClient,
      selectSession,
      onSuccess,
      modal,
      t,
    ]);

    const handleOpenChange = (open: boolean) => {
      if (!open) modal.hide();
    };

    const handleNewSessionChange = (checked: boolean) => {
      setCreateNewSession(checked);
    };

    const hasReviewComments = Boolean(reviewMarkdown);

    return (
      <Dialog open={modal.visible} onOpenChange={handleOpenChange}>
        <DialogContent className="sm:max-w-[500px]">
          <DialogHeader>
            <DialogTitle>{t('startReviewDialog.title')}</DialogTitle>
            <DialogDescription>
              {t('startReviewDialog.description')}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div className="space-y-2">
              <Label
                htmlFor="additional-prompt"
                className="text-sm font-medium"
              >
                {t('startReviewDialog.additionalInstructions')}
              </Label>
              <Textarea
                id="additional-prompt"
                value={additionalPrompt}
                onChange={(e) => setAdditionalPrompt(e.target.value)}
                placeholder="Add any specific instructions for the review..."
                className="min-h-[80px] resize-none"
              />
            </div>

            {hasReviewComments && (
              <div className="space-y-2">
                <Label className="text-sm font-medium">
                  {t('startReviewDialog.reviewComments', {
                    count:
                      reviewMarkdown
                        ?.split('\n')
                        .filter((l) => l.startsWith('-')).length ?? 0,
                  })}
                </Label>
                <div className="text-sm text-muted-foreground bg-muted/50 rounded-md p-3 max-h-32 overflow-y-auto">
                  <pre className="whitespace-pre-wrap font-sans text-xs">
                    {reviewMarkdown}
                  </pre>
                </div>
              </div>
            )}

            {error && <div className="text-sm text-destructive">{error}</div>}

            <div className="space-y-1">
              <div className="flex items-center space-x-2">
                <Checkbox
                  id="include-git-context"
                  checked={includeGitContext}
                  onCheckedChange={(checked) =>
                    setIncludeGitContext(checked === true)
                  }
                />
                <Label
                  htmlFor="include-git-context"
                  className="cursor-pointer text-sm"
                >
                  {t('startReviewDialog.includeGitContext')}
                </Label>
              </div>
              <p className="text-xs text-muted-foreground ml-6">
                {t('startReviewDialog.includeGitContextDescription')}
              </p>
            </div>

            {/* The reviewer agent decides runtime, model and prompts;
                edit it in Settings → Agents. */}
            <div className="space-y-1">
              <Label className="text-sm font-medium">
                {t('startReviewDialog.agentLabel', 'Agent')}
              </Label>
              {agentsLoading ? (
                <p className="text-sm text-muted-foreground">
                  {t('startReviewDialog.loadingAgents', 'Loading agents…')}
                </p>
              ) : agent?.definition ? (
                <p className="text-sm text-muted-foreground">
                  {t('startReviewDialog.agentDetail', {
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
                    'startReviewDialog.noAgent',
                    'No agent with the reviewer role. Create one in Settings → Agents.'
                  )}
                </p>
              )}
            </div>
          </div>

          <DialogFooter className="sm:!justify-between">
            <Button
              variant="outline"
              onClick={() => modal.hide()}
              disabled={isSubmitting}
            >
              {t('common:buttons.cancel')}
            </Button>
            <div className="flex items-center gap-3">
              <div className="flex items-center gap-2">
                <Switch
                  id="new-session-switch"
                  checked={createNewSession}
                  onCheckedChange={handleNewSessionChange}
                  disabled={!resolvedSessionId}
                  aria-label={t('startReviewDialog.newSession')}
                />
                <Label
                  htmlFor="new-session-switch"
                  className="text-sm cursor-pointer"
                >
                  {t('startReviewDialog.newSession')}
                </Label>
              </div>
              <Button onClick={handleSubmit} disabled={!canSubmit}>
                {isSubmitting
                  ? t('actionsMenu.startingReview')
                  : t('actionsMenu.startReview')}
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }
);

export const StartReviewDialog = defineModal<StartReviewDialogProps, void>(
  StartReviewDialogImpl
);
