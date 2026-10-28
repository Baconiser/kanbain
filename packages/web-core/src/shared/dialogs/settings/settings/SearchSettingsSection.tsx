import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { cloneDeep, isEqual } from 'lodash';
import { CheckCircleIcon, WarningIcon } from '@phosphor-icons/react';
import type {
  ConnectionTest,
  EmbedderConfig,
  EmbedderProvider,
  EmbeddingStatus,
  RerankerConfig,
  RerankerProvider,
  SearchConfig,
  SearchKeySlot,
  SearchKeys,
} from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { searchApi } from '@/shared/lib/ticketsApi';
import {
  SettingsCard,
  SettingsCheckbox,
  SettingsField,
  SettingsInput,
  SettingsSaveBar,
  SettingsSelect,
} from './SettingsComponents';
import { useSettingsDirty } from './SettingsDirtyContext';

const STATUS_POLL_MS = 3000;

const DEFAULT_EMBEDDER: EmbedderConfig = {
  provider: 'openai',
  base_url: 'http://localhost:11434/v1',
  model: 'nomic-embed-text',
  dimensions: null,
  query_prefix: 'search_query: ',
  document_prefix: 'search_document: ',
  batch_size: 32,
  max_input_tokens: 2048,
  timeout_secs: 30,
};

const DEFAULT_RERANKER: RerankerConfig = {
  provider: 'tei',
  base_url: 'http://localhost:8081',
  model: 'BAAI/bge-reranker-v2-m3',
  timeout_secs: 5,
};

/** Whether a URL leaves this machine and its private networks (E9). */
export function isRemoteUrl(url: string): boolean {
  let host: string;
  try {
    host = new URL(url.trim()).hostname.toLowerCase().replace(/^\[|\]$/g, '');
  } catch {
    return false;
  }
  if (
    host === 'localhost' ||
    host.endsWith('.localhost') ||
    host.endsWith('.local') ||
    host.endsWith('.internal') ||
    !host.includes('.') ||
    host === '::1'
  ) {
    return false;
  }
  const v4 = host.match(/^(\d+)\.(\d+)\.\d+\.\d+$/);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    const local =
      a === 127 ||
      a === 10 ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 169 && b === 254) ||
      a === 0;
    return !local;
  }
  return !/^f[cd]/.test(host);
}

function parseCount(value: string): number | null {
  const n = Number.parseInt(value, 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function TestResult({ result }: { result: ConnectionTest | null }) {
  const { t } = useTranslation('settings');
  if (!result) return null;
  return (
    <div
      className="flex items-start gap-half text-sm"
      data-testid="search-test"
    >
      {result.ok ? (
        <CheckCircleIcon className="size-icon-base shrink-0 text-success" />
      ) : (
        <WarningIcon className="size-icon-base shrink-0 text-error" />
      )}
      <span className={result.ok ? 'text-normal' : 'text-error'}>
        {result.ok
          ? result.dimension != null
            ? t('settings.search.test.okEmbedder', {
                model: result.model,
                dimension: result.dimension,
                ms: result.latency_ms,
              })
            : t('settings.search.test.okReranker', {
                model: result.model,
                ms: result.latency_ms,
              })
          : result.error}
      </span>
    </div>
  );
}

function ApiKeyField({
  slot,
  configured,
  onSaved,
}: {
  slot: SearchKeySlot;
  configured: boolean;
  onSaved: () => void;
}) {
  const { t } = useTranslation('settings');
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async (value: string | null) => {
    setBusy(true);
    setError(null);
    try {
      await searchApi.setKey({ slot, key: value ?? undefined });
      setKey('');
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsField
      label={t('settings.search.apiKey.label')}
      description={
        configured
          ? t('settings.search.apiKey.configured')
          : t('settings.search.apiKey.missing')
      }
      error={error}
    >
      <div className="flex gap-base">
        <div className="flex-1">
          <SettingsInput
            value={key}
            onChange={setKey}
            placeholder={t('settings.search.apiKey.placeholder')}
            disabled={busy}
            type="password"
          />
        </div>
        <PrimaryButton
          value={t('settings.search.apiKey.save')}
          onClick={() => save(key)}
          disabled={busy || !key.trim()}
        />
        {configured && (
          <PrimaryButton
            variant="tertiary"
            value={t('settings.search.apiKey.remove')}
            onClick={() => save(null)}
            disabled={busy}
          />
        )}
      </div>
    </SettingsField>
  );
}

function RemoteWarning({ url, code }: { url: string; code?: boolean }) {
  const { t } = useTranslation('settings');
  if (!isRemoteUrl(url)) return null;
  let host = url;
  try {
    host = new URL(url).host;
  } catch {
    // keep the raw value
  }
  return (
    <div
      className="flex items-start gap-half rounded-sm border border-warning/50 bg-warning/10 p-base text-sm"
      data-testid="search-remote-warning"
    >
      <WarningIcon className="size-icon-base shrink-0 text-warning" />
      <span>
        {code
          ? t('settings.search.remote.code', { host })
          : t('settings.search.remote.text', { host })}
      </span>
    </div>
  );
}

function EmbedderFields({
  idPrefix,
  value,
  onChange,
  slot,
  keys,
  onKeysChanged,
}: {
  idPrefix: string;
  value: EmbedderConfig;
  onChange: (value: EmbedderConfig) => void;
  slot: SearchKeySlot;
  keys: SearchKeys | null;
  onKeysChanged: () => void;
}) {
  const { t } = useTranslation('settings');
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ConnectionTest | null>(null);
  const set = (patch: Partial<EmbedderConfig>) =>
    onChange({ ...value, ...patch });

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      setResult(await searchApi.testEmbedder({ config: value, slot }));
    } catch (err) {
      setResult({
        ok: false,
        model: value.model,
        dimension: null,
        latency_ms: 0,
        remote: false,
        error: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div className="flex flex-col gap-base" data-testid={`${idPrefix}-fields`}>
      <SettingsField label={t('settings.search.embedder.provider')}>
        <SettingsSelect<EmbedderProvider>
          value={value.provider}
          options={[
            { value: 'openai', label: t('settings.search.embedder.openai') },
            { value: 'tei', label: t('settings.search.embedder.tei') },
          ]}
          onChange={(provider) => set({ provider })}
        />
      </SettingsField>
      <SettingsField
        label={t('settings.search.embedder.baseUrl')}
        description={t('settings.search.embedder.baseUrlHelper')}
      >
        <SettingsInput
          value={value.base_url}
          onChange={(base_url) => set({ base_url })}
          placeholder="http://localhost:11434/v1"
        />
      </SettingsField>
      <RemoteWarning url={value.base_url} code={slot === 'code'} />
      <SettingsField label={t('settings.search.embedder.model')}>
        <SettingsInput
          value={value.model}
          onChange={(model) => set({ model })}
          placeholder="nomic-embed-text"
        />
      </SettingsField>
      <SettingsField
        label={t('settings.search.embedder.prefixes')}
        description={t('settings.search.embedder.prefixesHelper')}
      >
        <div className="grid grid-cols-2 gap-base">
          <SettingsInput
            value={value.query_prefix}
            onChange={(query_prefix) => set({ query_prefix })}
            placeholder="search_query: "
          />
          <SettingsInput
            value={value.document_prefix}
            onChange={(document_prefix) => set({ document_prefix })}
            placeholder="search_document: "
          />
        </div>
      </SettingsField>
      <SettingsField
        label={t('settings.search.embedder.dimensions')}
        description={t('settings.search.embedder.dimensionsHelper')}
      >
        <SettingsInput
          value={value.dimensions == null ? '' : String(value.dimensions)}
          onChange={(v) => set({ dimensions: v.trim() ? parseCount(v) : null })}
        />
      </SettingsField>
      <SettingsField label={t('settings.search.embedder.batchSize')}>
        <SettingsInput
          value={String(value.batch_size)}
          onChange={(v) => {
            const n = parseCount(v);
            if (n !== null) set({ batch_size: n });
          }}
        />
      </SettingsField>
      <ApiKeyField
        slot={slot}
        configured={keys?.[slot] ?? false}
        onSaved={onKeysChanged}
      />
      <div className="flex items-center gap-base">
        <PrimaryButton
          variant="secondary"
          value={t('settings.search.test.button')}
          onClick={() => void test()}
          disabled={testing || !value.base_url.trim()}
        />
        <TestResult result={result} />
      </div>
    </div>
  );
}

function IndexStatus({ status }: { status: EmbeddingStatus | null }) {
  const { t } = useTranslation('settings');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const reindex = async (rebuild: boolean) => {
    setBusy(true);
    setMessage(null);
    try {
      const result = await searchApi.reindex({ rebuild_vectors: rebuild });
      setMessage(t('settings.search.index.queued', { count: result.queued }));
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!status) {
    return (
      <p className="text-sm text-low">
        {t('settings.search.index.unavailable')}
      </p>
    );
  }
  const spaces = status.spaces.filter((s) => s.space.state !== 'retired');
  return (
    <div
      className="flex flex-col gap-base text-sm"
      data-testid="search-index-status"
    >
      <p className="text-low">
        {t('settings.search.index.summary', {
          chunks: status.chunks,
          sources: status.sources,
          queued: status.jobs.queued + status.jobs.running,
          failed: status.jobs.failed,
        })}
      </p>
      {status.embedder_error && (
        <p className="text-error">{status.embedder_error}</p>
      )}
      {spaces.length === 0 ? (
        <p className="text-low">{t('settings.search.index.noSpaces')}</p>
      ) : (
        <ul className="flex flex-col gap-half">
          {spaces.map(({ space, chunks, embedded }) => (
            <li
              key={space.id}
              className="flex flex-wrap items-baseline gap-x-base"
              data-state={space.state}
            >
              <span className="font-medium text-normal">
                {space.purpose} · {space.model}
              </span>
              <span className="text-low">
                {t('settings.search.index.space', {
                  dimension: space.dimension,
                  state: t(`settings.search.index.state.${space.state}`),
                  embedded,
                  chunks,
                })}
              </span>
              {space.last_error && (
                <span className="text-error">{space.last_error}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-wrap items-center gap-base">
        <PrimaryButton
          variant="secondary"
          value={t('settings.search.index.reindex')}
          onClick={() => void reindex(false)}
          disabled={busy}
        />
        <PrimaryButton
          variant="tertiary"
          value={t('settings.search.index.rebuild')}
          onClick={() => void reindex(true)}
          disabled={busy}
        />
        {message && <span className="text-low">{message}</span>}
      </div>
    </div>
  );
}

export function SearchSettingsSection() {
  const { t } = useTranslation(['settings', 'common']);
  const { setDirty: setContextDirty } = useSettingsDirty();
  const { config, updateAndSaveConfig } = useUserSystem();
  const [draft, setDraft] = useState<SearchConfig | null>(() =>
    config ? cloneDeep(config.search) : null
  );
  const [keys, setKeys] = useState<SearchKeys | null>(null);
  const [status, setStatus] = useState<EmbeddingStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rerankResult, setRerankResult] = useState<ConnectionTest | null>(null);

  useEffect(() => {
    if (config && draft === null) setDraft(cloneDeep(config.search));
  }, [config, draft]);

  const loadKeys = useCallback(() => {
    searchApi
      .keys()
      .then(setKeys)
      .catch(() => {});
  }, []);
  useEffect(loadKeys, [loadKeys]);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      searchApi
        .status()
        .then((s) => {
          if (!cancelled) setStatus(s);
        })
        .catch(() => {
          if (!cancelled) setStatus(null);
        });
    void load();
    const timer = window.setInterval(() => void load(), STATUS_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const hasUnsavedChanges = useMemo(
    () => !!config && !!draft && !isEqual(draft, config.search),
    [config, draft]
  );
  useEffect(() => {
    setContextDirty('search', hasUnsavedChanges);
    return () => setContextDirty('search', false);
  }, [hasUnsavedChanges, setContextDirty]);

  const update = useCallback((patch: Partial<SearchConfig>) => {
    setDraft((prev) => (prev ? { ...prev, ...patch } : prev));
  }, []);

  const handleSave = async () => {
    if (!config || !draft) return;
    setSaving(true);
    setError(null);
    try {
      await updateAndSaveConfig({ ...config, search: draft });
    } catch (err) {
      setError(t('settings.general.save.error'));
      console.error('Error saving search settings:', err);
    } finally {
      setSaving(false);
    }
  };

  if (!draft) return null;
  const reranker = draft.reranker;

  return (
    <>
      {error && (
        <div className="bg-error/10 border border-error/50 rounded-sm p-4 text-error">
          {error}
        </div>
      )}

      <SettingsCard
        title={t('settings.search.text.title')}
        description={t('settings.search.text.description')}
      >
        <SettingsCheckbox
          id="search-text-enabled"
          label={t('settings.search.text.enabled')}
          checked={draft.text !== null}
          onChange={(checked) =>
            update({ text: checked ? cloneDeep(DEFAULT_EMBEDDER) : null })
          }
        />
        {draft.text && (
          <EmbedderFields
            idPrefix="search-text"
            value={draft.text}
            onChange={(text) => update({ text })}
            slot="text"
            keys={keys}
            onKeysChanged={loadKeys}
          />
        )}
      </SettingsCard>

      <SettingsCard
        title={t('settings.search.code.title')}
        description={t('settings.search.code.description')}
      >
        <SettingsCheckbox
          id="search-code-same"
          label={t('settings.search.code.sameAsText')}
          checked={draft.code === null}
          onChange={(same) =>
            update({
              code: same ? null : cloneDeep(draft.text ?? DEFAULT_EMBEDDER),
            })
          }
        />
        {draft.code && (
          <EmbedderFields
            idPrefix="search-code"
            value={draft.code}
            onChange={(code) => update({ code })}
            slot="code"
            keys={keys}
            onKeysChanged={loadKeys}
          />
        )}
      </SettingsCard>

      <SettingsCard
        title={t('settings.search.reranker.title')}
        description={t('settings.search.reranker.description')}
      >
        <SettingsCheckbox
          id="search-reranker-enabled"
          label={t('settings.search.reranker.enabled')}
          checked={reranker !== null}
          onChange={(checked) =>
            update({ reranker: checked ? cloneDeep(DEFAULT_RERANKER) : null })
          }
        />
        {reranker && (
          <div className="flex flex-col gap-base">
            <SettingsField label={t('settings.search.embedder.provider')}>
              <SettingsSelect<RerankerProvider>
                value={reranker.provider}
                options={[
                  { value: 'tei', label: t('settings.search.reranker.tei') },
                  { value: 'jina', label: t('settings.search.reranker.jina') },
                ]}
                onChange={(provider) =>
                  update({ reranker: { ...reranker, provider } })
                }
              />
            </SettingsField>
            <SettingsField label={t('settings.search.embedder.baseUrl')}>
              <SettingsInput
                value={reranker.base_url}
                onChange={(base_url) =>
                  update({ reranker: { ...reranker, base_url } })
                }
              />
            </SettingsField>
            <RemoteWarning url={reranker.base_url} />
            <SettingsField label={t('settings.search.embedder.model')}>
              <SettingsInput
                value={reranker.model}
                onChange={(model) =>
                  update({ reranker: { ...reranker, model } })
                }
              />
            </SettingsField>
            <ApiKeyField
              slot="reranker"
              configured={keys?.reranker ?? false}
              onSaved={loadKeys}
            />
            <div className="flex items-center gap-base">
              <PrimaryButton
                variant="secondary"
                value={t('settings.search.test.button')}
                onClick={() =>
                  void searchApi
                    .testReranker({ config: reranker })
                    .then(setRerankResult)
                    .catch(() => {})
                }
                disabled={!reranker.base_url.trim()}
              />
              <TestResult result={rerankResult} />
            </div>
          </div>
        )}
      </SettingsCard>

      <SettingsCard
        title={t('settings.search.retrieval.title')}
        description={t('settings.search.retrieval.description')}
      >
        <div className="grid grid-cols-2 gap-base">
          <SettingsField label={t('settings.search.retrieval.candidates')}>
            <SettingsInput
              value={String(draft.candidates)}
              onChange={(v) => {
                const n = parseCount(v);
                if (n !== null) update({ candidates: n });
              }}
            />
          </SettingsField>
          <SettingsField label={t('settings.search.retrieval.rrfK')}>
            <SettingsInput
              value={String(draft.rrf_k)}
              onChange={(v) => {
                const n = parseCount(v);
                if (n !== null) update({ rrf_k: n });
              }}
            />
          </SettingsField>
          <SettingsField label={t('settings.search.retrieval.limit')}>
            <SettingsInput
              value={String(draft.limit)}
              onChange={(v) => {
                const n = parseCount(v);
                if (n !== null) update({ limit: n });
              }}
            />
          </SettingsField>
          <SettingsField label={t('settings.search.retrieval.excerpt')}>
            <SettingsInput
              value={String(draft.excerpt_chars)}
              onChange={(v) => {
                const n = parseCount(v);
                if (n !== null) update({ excerpt_chars: n });
              }}
            />
          </SettingsField>
        </div>
      </SettingsCard>

      <SettingsCard
        title={t('settings.search.index.title')}
        description={t('settings.search.index.description')}
      >
        <IndexStatus status={status} />
      </SettingsCard>

      <SettingsSaveBar
        show={hasUnsavedChanges}
        saving={saving}
        onSave={handleSave}
        onDiscard={() => config && setDraft(cloneDeep(config.search))}
      />
    </>
  );
}
