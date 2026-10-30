import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { ModelProviderInfo, SaveModelProvider } from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { sandboxApi } from '@/shared/lib/api';
import { agentLibraryKeys } from '@/shared/hooks/useAgentLibrary';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
} from './SettingsComponents';

const providersKey = ['sandbox', 'model-providers'] as const;

type Draft = {
  id: string;
  name: string;
  baseUrl: string;
  models: string;
  apiKey: string;
};

const emptyDraft: Draft = {
  id: '',
  name: '',
  baseUrl: '',
  models: '',
  apiKey: '',
};

function draftFrom(provider: ModelProviderInfo): Draft {
  return {
    id: provider.id,
    name: provider.name,
    baseUrl: provider.base_url,
    models: provider.models.join(', '),
    apiKey: '',
  };
}

/** Custom OpenAI-compatible providers OpenCode agents can use. */
export function ModelProvidersCard() {
  const { t } = useTranslation('settings');
  const queryClient = useQueryClient();
  const { data: providers = [] } = useQuery({
    queryKey: providersKey,
    queryFn: () => sandboxApi.listModelProviders(),
  });
  // `null`: no form open; `editing`: the id of the provider being edited.
  const [draft, setDraft] = useState<Draft | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const apply = async (action: () => Promise<ModelProviderInfo[]>) => {
    setBusy(true);
    setError(null);
    try {
      queryClient.setQueryData(providersKey, await action());
      // The agent editor suggests the providers' models.
      queryClient.invalidateQueries({ queryKey: agentLibraryKeys.meta });
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!draft) return;
    const payload: SaveModelProvider = {
      id: draft.id.trim(),
      name: draft.name.trim(),
      base_url: draft.baseUrl.trim(),
      models: draft.models
        .split(/[\s,]+/)
        .map((m) => m.trim())
        .filter(Boolean),
      ...(draft.apiKey.trim() ? { api_key: draft.apiKey.trim() } : {}),
    };
    if (await apply(() => sandboxApi.saveModelProvider(payload))) {
      setDraft(null);
      setEditing(null);
    }
  };

  const set = (patch: Partial<Draft>) =>
    setDraft((d) => (d ? { ...d, ...patch } : d));

  return (
    <SettingsCard
      title={t('settings.sandbox.modelProviders.title')}
      description={t('settings.sandbox.modelProviders.description')}
    >
      {providers.map((provider) =>
        editing === provider.id ? null : (
          <div
            key={provider.id}
            className="flex flex-wrap items-start gap-base text-sm"
          >
            <div className="min-w-0 flex-1 space-y-half">
              <p className="text-normal">
                {provider.name}{' '}
                <span className="text-low">({provider.id})</span>
              </p>
              <p className="break-all text-low">{provider.base_url}</p>
              <p className="break-all text-low">
                {provider.models.length
                  ? provider.models.map((m) => `${provider.id}/${m}`).join(', ')
                  : t('settings.sandbox.modelProviders.noModels', {
                      id: provider.id,
                    })}
              </p>
              <p className="text-low">
                {provider.api_key_configured
                  ? t('settings.sandbox.modelProviders.keyConfigured')
                  : t('settings.sandbox.modelProviders.keyMissing')}
              </p>
            </div>
            <PrimaryButton
              variant="tertiary"
              value={t('settings.sandbox.modelProviders.edit')}
              onClick={() => {
                setEditing(provider.id);
                setDraft(draftFrom(provider));
                setError(null);
              }}
              disabled={busy}
            />
            <PrimaryButton
              variant="tertiary"
              value={t('settings.sandbox.modelProviders.remove')}
              onClick={() =>
                apply(() => sandboxApi.deleteModelProvider(provider.id))
              }
              disabled={busy}
            />
          </div>
        )
      )}

      {draft ? (
        <div className="space-y-base">
          <SettingsField
            label={t('settings.sandbox.modelProviders.id')}
            description={t('settings.sandbox.modelProviders.idHelper')}
          >
            <SettingsInput
              value={draft.id}
              onChange={(id) => set({ id: id.toLowerCase() })}
              placeholder="deepseek"
              disabled={busy || editing !== null}
            />
          </SettingsField>
          <SettingsField label={t('settings.sandbox.modelProviders.name')}>
            <SettingsInput
              value={draft.name}
              onChange={(name) => set({ name })}
              placeholder="DeepSeek"
              disabled={busy}
            />
          </SettingsField>
          <SettingsField
            label={t('settings.sandbox.modelProviders.baseUrl')}
            description={t('settings.sandbox.modelProviders.baseUrlHelper')}
          >
            <SettingsInput
              value={draft.baseUrl}
              onChange={(baseUrl) => set({ baseUrl })}
              placeholder="https://api.deepseek.com/v1"
              disabled={busy}
            />
          </SettingsField>
          <SettingsField
            label={t('settings.sandbox.modelProviders.models')}
            description={t('settings.sandbox.modelProviders.modelsHelper')}
          >
            <SettingsInput
              value={draft.models}
              onChange={(models) => set({ models })}
              placeholder="deepseek-chat, deepseek-reasoner"
              disabled={busy}
            />
          </SettingsField>
          <SettingsField
            label={t('settings.sandbox.modelProviders.apiKey')}
            description={
              editing
                ? t('settings.sandbox.modelProviders.apiKeyKeep')
                : t('settings.sandbox.modelProviders.apiKeyHelper')
            }
          >
            <SettingsInput
              value={draft.apiKey}
              onChange={(apiKey) => set({ apiKey })}
              type="password"
              disabled={busy}
            />
          </SettingsField>
          <div className="flex gap-base">
            <PrimaryButton
              value={t('settings.sandbox.modelProviders.save')}
              onClick={save}
              disabled={busy || !draft.id.trim() || !draft.baseUrl.trim()}
            />
            <PrimaryButton
              variant="tertiary"
              value={t('settings.agentLibrary.cancel')}
              onClick={() => {
                setDraft(null);
                setEditing(null);
                setError(null);
              }}
              disabled={busy}
            />
          </div>
        </div>
      ) : (
        <PrimaryButton
          variant="secondary"
          value={t('settings.sandbox.modelProviders.add')}
          onClick={() => {
            setDraft(emptyDraft);
            setError(null);
          }}
          disabled={busy}
        />
      )}

      {error && <p className="text-sm text-error">{error}</p>}
      <p className="text-sm text-low">
        {t('settings.sandbox.modelProviders.costNote')}
      </p>
    </SettingsCard>
  );
}
