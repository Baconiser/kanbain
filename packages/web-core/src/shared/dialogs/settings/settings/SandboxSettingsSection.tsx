import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cloneDeep, isEqual } from 'lodash';
import { useQueryClient } from '@tanstack/react-query';
import { CheckCircleIcon, WarningIcon } from '@phosphor-icons/react';
import type { CredentialProvider, SandboxConfig } from 'shared/types';
import { sandboxApi } from '@/shared/lib/api';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import {
  sandboxStatusKey,
  useSandboxStatus,
} from '@/shared/hooks/useSandboxStatus';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import {
  SettingsCard,
  SettingsCheckbox,
  SettingsField,
  SettingsInput,
  SettingsSaveBar,
  SettingsTextarea,
} from './SettingsComponents';
import { useSettingsDirty } from './SettingsDirtyContext';
import { ClaudeAccountCard } from './ClaudeAccountCard';
import { ModelProvidersCard } from './ModelProvidersCard';

function parseNumber(value: string): number | null {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) && n >= 0 ? n : null;
}

function CredentialField({
  provider,
  configured,
}: {
  provider: CredentialProvider;
  configured: boolean;
}) {
  const { t } = useTranslation('settings');
  const queryClient = useQueryClient();
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async (apiKey: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const status = await sandboxApi.setCredential({
        provider,
        api_key: apiKey,
      });
      queryClient.setQueryData(sandboxStatusKey, status);
      setKey('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsField
      label={t(`settings.sandbox.credentials.${provider}.label`)}
      description={
        configured
          ? t('settings.sandbox.credentials.configured')
          : t('settings.sandbox.credentials.missing')
      }
      error={error}
    >
      <div className="flex gap-base">
        <div className="flex-1">
          <SettingsInput
            value={key}
            onChange={setKey}
            placeholder={t(
              `settings.sandbox.credentials.${provider}.placeholder`
            )}
            disabled={busy}
            type="password"
          />
        </div>
        <PrimaryButton
          value={t('settings.sandbox.credentials.save')}
          onClick={() => save(key)}
          disabled={busy || !key.trim()}
        />
        {configured && (
          <PrimaryButton
            variant="tertiary"
            value={t('settings.sandbox.credentials.remove')}
            onClick={() => save(null)}
            disabled={busy}
          />
        )}
      </div>
    </SettingsField>
  );
}

export function SandboxSettingsSection() {
  const { t } = useTranslation(['settings', 'common']);
  const { setDirty: setContextDirty } = useSettingsDirty();
  const { config, updateAndSaveConfig } = useUserSystem();
  const { data: status } = useSandboxStatus();

  const [draft, setDraft] = useState<SandboxConfig | null>(() =>
    config ? cloneDeep(config.sandbox) : null
  );
  const [domains, setDomains] = useState(
    () => config?.sandbox.allowed_domains.join('\n') ?? ''
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (config && draft === null) {
      setDraft(cloneDeep(config.sandbox));
      setDomains(config.sandbox.allowed_domains.join('\n'));
    }
  }, [config, draft]);

  const edited = useMemo(() => {
    if (!draft) return null;
    return {
      ...draft,
      allowed_domains: domains
        .split('\n')
        .map((d) => d.trim())
        .filter(Boolean),
    };
  }, [draft, domains]);

  const hasUnsavedChanges = useMemo(
    () => !!config && !!edited && !isEqual(edited, config.sandbox),
    [config, edited]
  );

  useEffect(() => {
    setContextDirty('sandbox', hasUnsavedChanges);
    return () => setContextDirty('sandbox', false);
  }, [hasUnsavedChanges, setContextDirty]);

  const update = useCallback((patch: Partial<SandboxConfig>) => {
    setDraft((prev) => (prev ? { ...prev, ...patch } : prev));
  }, []);

  const handleSave = async () => {
    if (!config || !edited) return;
    setSaving(true);
    setError(null);
    try {
      await updateAndSaveConfig({ ...config, sandbox: edited });
    } catch (err) {
      setError(t('settings.general.save.error'));
      console.error('Error saving sandbox settings:', err);
    } finally {
      setSaving(false);
    }
  };

  const handleDiscard = () => {
    if (!config) return;
    setDraft(cloneDeep(config.sandbox));
    setDomains(config.sandbox.allowed_domains.join('\n'));
  };

  if (!draft) return null;

  const dockerOk = status?.docker_available ?? false;
  const imageState = status?.image_building
    ? t('settings.sandbox.docker.imageBuilding')
    : status?.image_error
      ? t('settings.sandbox.docker.imageError', { error: status.image_error })
      : status?.image_ready
        ? t('settings.sandbox.docker.imageReady')
        : t('settings.sandbox.docker.imageMissing');

  return (
    <>
      {error && (
        <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error">
          {error}
        </div>
      )}

      <SettingsCard
        title={t('settings.sandbox.docker.title')}
        description={t('settings.sandbox.docker.description')}
      >
        <div className="flex items-start gap-base text-sm">
          {dockerOk ? (
            <CheckCircleIcon className="size-icon-base text-success shrink-0" />
          ) : (
            <WarningIcon className="size-icon-base text-error shrink-0" />
          )}
          <div className="space-y-half">
            <p className="text-normal">
              {dockerOk
                ? t('settings.sandbox.docker.available', {
                    version: status?.docker_version ?? '',
                  })
                : t('settings.sandbox.docker.unavailable')}
            </p>
            {!dockerOk && status?.error && (
              <p className="text-low font-mono text-xs break-all">
                {status.error}
              </p>
            )}
            {!dockerOk && (
              <p className="text-low">
                {t('settings.sandbox.docker.installHint')}
              </p>
            )}
            {dockerOk && (
              <p className="text-low">
                {imageState} · {status?.image}
              </p>
            )}
            {dockerOk && (
              <p className="text-low">
                {t('settings.sandbox.docker.containers', {
                  count: status?.running_containers ?? 0,
                })}
              </p>
            )}
          </div>
        </div>
        <SettingsCheckbox
          id="sandbox-enabled"
          label={t('settings.sandbox.enabled.label')}
          description={t('settings.sandbox.enabled.helper')}
          checked={draft.enabled}
          onChange={(checked) => update({ enabled: checked })}
        />
      </SettingsCard>

      <SettingsCard
        title={t('settings.sandbox.credentials.title')}
        description={t('settings.sandbox.credentials.description')}
      >
        <CredentialField
          provider="anthropic"
          configured={status?.anthropic_key_configured ?? false}
        />
        <CredentialField
          provider="openai"
          configured={status?.openai_key_configured ?? false}
        />
      </SettingsCard>

      <ClaudeAccountCard />

      <ModelProvidersCard />

      <SettingsCard
        title={t('settings.sandbox.limits.title')}
        description={t('settings.sandbox.limits.description')}
      >
        <SettingsField
          label={t('settings.sandbox.limits.runBudget')}
          description={t('settings.sandbox.limits.budgetHelper')}
        >
          <SettingsInput
            value={
              draft.run_budget_usd == null ? '' : String(draft.run_budget_usd)
            }
            onChange={(value) =>
              update({
                run_budget_usd: value.trim() ? parseNumber(value) : null,
              })
            }
          />
        </SettingsField>
        <SettingsField
          label={t('settings.sandbox.limits.ticketBudget')}
          description={t('settings.sandbox.limits.budgetHelper')}
        >
          <SettingsInput
            value={
              draft.ticket_budget_usd == null
                ? ''
                : String(draft.ticket_budget_usd)
            }
            onChange={(value) =>
              update({
                ticket_budget_usd: value.trim() ? parseNumber(value) : null,
              })
            }
          />
        </SettingsField>
        <SettingsField
          label={t('settings.sandbox.limits.maxRunMinutes')}
          description={t('settings.sandbox.limits.maxRunMinutesHelper')}
        >
          <SettingsInput
            value={String(draft.max_run_minutes)}
            onChange={(value) => {
              const n = parseNumber(value);
              if (n !== null) update({ max_run_minutes: Math.round(n) });
            }}
          />
        </SettingsField>
        <SettingsField
          label={t('settings.sandbox.limits.maxConcurrent')}
          description={t('settings.sandbox.limits.restartHelper')}
        >
          <SettingsInput
            value={String(draft.max_concurrent_runs)}
            onChange={(value) => {
              const n = parseNumber(value);
              if (n !== null && n >= 1) {
                update({ max_concurrent_runs: Math.round(n) });
              }
            }}
          />
        </SettingsField>
        <SettingsField label={t('settings.sandbox.limits.cpus')}>
          <SettingsInput
            value={String(draft.cpus)}
            onChange={(value) => {
              const n = parseNumber(value);
              if (n !== null && n > 0) update({ cpus: n });
            }}
          />
        </SettingsField>
        <SettingsField label={t('settings.sandbox.limits.memory')}>
          <SettingsInput
            value={String(draft.memory_mb)}
            onChange={(value) => {
              const n = parseNumber(value);
              if (n !== null && n >= 256) update({ memory_mb: Math.round(n) });
            }}
          />
        </SettingsField>
        <SettingsField
          label={t('settings.sandbox.limits.allowedDomains')}
          description={t('settings.sandbox.limits.allowedDomainsHelper')}
        >
          <SettingsTextarea
            value={domains}
            onChange={setDomains}
            placeholder={'api.example.com\n*.example.org'}
            rows={4}
            monospace
          />
        </SettingsField>
      </SettingsCard>

      <SettingsSaveBar
        show={hasUnsavedChanges}
        saving={saving}
        onSave={handleSave}
        onDiscard={handleDiscard}
      />
    </>
  );
}
