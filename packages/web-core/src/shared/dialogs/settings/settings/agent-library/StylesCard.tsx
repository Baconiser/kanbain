import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PlusIcon } from '@phosphor-icons/react';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { agentLibraryApi } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';
import {
  useInvalidateAgentLibrary,
  useStyles,
} from '@/shared/hooks/useAgentLibrary';
import { ConfirmDialog } from '@/shared/dialogs/shared/ConfirmDialog';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
  SettingsTextarea,
} from '../SettingsComponents';
import { useSettingsDirty } from '../SettingsDirtyContext';

/** Reusable blocks appended to agents' system prompts. */
export function StylesCard() {
  const { t } = useTranslation('settings');
  const { setDirty } = useSettingsDirty();
  const invalidate = useInvalidateAgentLibrary();
  const { data: styles = [] } = useStyles();
  // `null` = nothing open, '' = a new style, otherwise the style's name.
  const [open, setOpen] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [body, setBody] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const opened = styles.find((s) => s.name === open);
  const dirty =
    open !== null &&
    (open === '' ? !!(name || body) : name !== open || body !== opened?.body);

  useEffect(() => setDirty('styles', dirty), [dirty, setDirty]);
  useEffect(() => () => setDirty('styles', false), [setDirty]);

  const select = (next: string | null) => {
    const style = styles.find((s) => s.name === next);
    setOpen(next);
    setName(style?.name ?? '');
    setBody(style?.body ?? '');
    setError(null);
  };

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      const style = { name: name.trim(), body };
      if (open) {
        await agentLibraryApi.updateStyle(open, style);
      } else {
        await agentLibraryApi.createStyle(style);
      }
      await invalidate();
      setOpen(style.name);
    });

  const remove = async () => {
    if (!open) return;
    const result = await ConfirmDialog.show({
      title: t('settings.agentLibrary.styles.deleteTitle'),
      message: t('settings.agentLibrary.styles.deleteMessage', { name: open }),
      confirmText: t('settings.agentLibrary.delete'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    await run(async () => {
      await agentLibraryApi.deleteStyle(open);
      await invalidate();
      select(null);
    });
  };

  return (
    <SettingsCard
      title={t('settings.agentLibrary.styles.title')}
      description={t('settings.agentLibrary.styles.description')}
      headerAction={
        <PrimaryButton
          variant="secondary"
          actionIcon={PlusIcon}
          value={t('settings.agentLibrary.styles.new')}
          onClick={() => select('')}
          disabled={busy}
        />
      }
    >
      <div className="divide-y divide-border rounded-sm border border-border">
        {styles.map((style) => (
          <button
            key={style.name}
            type="button"
            onClick={() => select(style.name === open ? null : style.name)}
            className={cn(
              'flex w-full items-center gap-base px-base py-half text-left hover:bg-secondary',
              style.name === open && 'bg-brand/10'
            )}
          >
            <span className="font-mono text-sm text-high">{style.name}</span>
            <span className="flex-1 truncate text-sm text-low">
              {style.body.split('\n')[0]}
            </span>
            <span className="shrink-0 text-xs text-low">
              {style.used_by.length > 0
                ? t('settings.agentLibrary.usedBy', {
                    names: style.used_by.join(', '),
                  })
                : t('settings.agentLibrary.unused')}
            </span>
          </button>
        ))}
        {styles.length === 0 && (
          <p className="px-base py-half text-sm text-low">
            {t('settings.agentLibrary.styles.empty')}
          </p>
        )}
      </div>

      {open !== null && (
        <div className="space-y-4 rounded-sm border border-border p-base">
          {error && <p className="text-sm text-error">{error}</p>}
          <SettingsField
            label={t('settings.agentLibrary.styles.name')}
            description={t('settings.agentLibrary.nameHelper')}
          >
            <SettingsInput
              value={name}
              onChange={setName}
              placeholder="concise-commits"
            />
          </SettingsField>
          <SettingsField
            label={t('settings.agentLibrary.styles.body')}
            description={
              opened && opened.used_by.length > 0
                ? t('settings.agentLibrary.styles.bodyHelperUsed', {
                    names: opened.used_by.join(', '),
                  })
                : undefined
            }
          >
            <SettingsTextarea value={body} onChange={setBody} rows={8} />
          </SettingsField>
          <div className="flex flex-wrap gap-base">
            <PrimaryButton
              value={t('settings.agentLibrary.save')}
              onClick={save}
              disabled={busy || !dirty || !name.trim()}
              actionIcon={busy ? 'spinner' : undefined}
            />
            <PrimaryButton
              variant="tertiary"
              value={t('settings.agentLibrary.cancel')}
              onClick={() => select(null)}
              disabled={busy}
            />
            {open && (
              <PrimaryButton
                variant="tertiary"
                value={t('settings.agentLibrary.delete')}
                onClick={remove}
                disabled={busy || (opened?.used_by.length ?? 0) > 0}
              />
            )}
          </div>
        </div>
      )}
    </SettingsCard>
  );
}
