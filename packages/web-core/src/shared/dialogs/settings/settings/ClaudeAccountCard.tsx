import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { CheckCircleIcon } from '@phosphor-icons/react';
import type { ClaudeLoginState } from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { sandboxApi } from '@/shared/lib/api';
import {
  sandboxStatusKey,
  useSandboxStatus,
} from '@/shared/hooks/useSandboxStatus';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
} from './SettingsComponents';

const loginKey = ['sandbox', 'claude-login'] as const;

function inProgress(state: ClaudeLoginState | undefined) {
  return (
    state?.state === 'starting' ||
    state?.state === 'waiting_for_code' ||
    state?.state === 'exchanging'
  );
}

/** "Connect Claude account": `claude setup-token` in a throwaway container. */
export function ClaudeAccountCard() {
  const { t } = useTranslation('settings');
  const queryClient = useQueryClient();
  const { data: status } = useSandboxStatus();
  const { data: login } = useQuery({
    queryKey: loginKey,
    queryFn: () => sandboxApi.getClaudeLogin(),
    refetchInterval: (query) => (inProgress(query.state.data) ? 1_500 : false),
  });
  const [code, setCode] = useState('');
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const connected = status?.claude_account_connected ?? false;

  useEffect(() => {
    if (login?.state === 'connected') {
      queryClient.invalidateQueries({ queryKey: sandboxStatusKey });
    }
  }, [login?.state, queryClient]);

  const act = async (action: () => Promise<ClaudeLoginState | void>) => {
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      if (next) queryClient.setQueryData(loginKey, next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = () =>
    act(async () => {
      const next = await sandboxApi.disconnectClaudeAccount();
      queryClient.setQueryData(sandboxStatusKey, next);
      return sandboxApi.getClaudeLogin();
    });

  return (
    <SettingsCard
      title={t('settings.sandbox.claudeAccount.title')}
      description={t('settings.sandbox.claudeAccount.description')}
    >
      {connected && !inProgress(login) && (
        <div className="flex flex-wrap items-center gap-base text-sm">
          <CheckCircleIcon className="size-icon-base text-success" />
          <span className="flex-1 text-normal">
            {t('settings.sandbox.claudeAccount.connected')}
          </span>
          <PrimaryButton
            variant="tertiary"
            value={t('settings.sandbox.claudeAccount.disconnect')}
            onClick={disconnect}
            disabled={busy}
          />
        </div>
      )}

      {!inProgress(login) && (
        <div className="space-y-half">
          <PrimaryButton
            variant={connected ? 'tertiary' : 'secondary'}
            value={
              connected
                ? t('settings.sandbox.claudeAccount.reconnect')
                : t('settings.sandbox.claudeAccount.connect')
            }
            onClick={() => act(() => sandboxApi.startClaudeLogin())}
            disabled={busy || !status?.docker_available}
          />
          {login?.state === 'failed' && (
            <p className="whitespace-pre-wrap text-sm text-error">
              {login.message}
            </p>
          )}
        </div>
      )}

      {login?.state === 'starting' && (
        <p className="text-sm text-low">
          {t('settings.sandbox.claudeAccount.starting')}
        </p>
      )}

      {login?.state === 'waiting_for_code' && (
        <div className="space-y-base">
          <p className="text-sm text-normal">
            {t('settings.sandbox.claudeAccount.openLink')}
          </p>
          <div className="flex flex-wrap items-center gap-base">
            <a
              href={login.url}
              target="_blank"
              rel="noopener noreferrer"
              className="text-sm text-brand underline"
            >
              {t('settings.sandbox.claudeAccount.signIn')}
            </a>
            <button
              type="button"
              className="text-sm text-low hover:text-normal"
              onClick={() => {
                navigator.clipboard
                  .writeText(login.url)
                  .then(() => setCopied(true))
                  .catch(() => setCopied(false));
              }}
            >
              {copied
                ? t('settings.sandbox.claudeAccount.copied')
                : t('settings.sandbox.claudeAccount.copyLink')}
            </button>
          </div>
          <SettingsField label={t('settings.sandbox.claudeAccount.code')}>
            <div className="flex gap-base">
              <div className="flex-1">
                <SettingsInput
                  value={code}
                  onChange={setCode}
                  type="password"
                  disabled={busy}
                />
              </div>
              <PrimaryButton
                value={t('settings.sandbox.claudeAccount.submit')}
                onClick={() =>
                  act(async () => {
                    const next = await sandboxApi.submitClaudeLoginCode(code);
                    setCode('');
                    return next;
                  })
                }
                disabled={busy || !code.trim()}
              />
            </div>
          </SettingsField>
        </div>
      )}

      {login?.state === 'exchanging' && (
        <p className="text-sm text-low">
          {t('settings.sandbox.claudeAccount.exchanging')}
        </p>
      )}

      {inProgress(login) && (
        <PrimaryButton
          variant="tertiary"
          value={t('settings.agentLibrary.cancel')}
          onClick={() => act(() => sandboxApi.cancelClaudeLogin())}
          disabled={busy}
        />
      )}

      {error && <p className="text-sm text-error">{error}</p>}
      <p className="text-sm text-low">
        {t('settings.sandbox.claudeAccount.costNote')}
      </p>
    </SettingsCard>
  );
}
