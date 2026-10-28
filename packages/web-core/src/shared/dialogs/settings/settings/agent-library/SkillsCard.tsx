import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PlusIcon, WarningIcon } from '@phosphor-icons/react';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { agentLibraryApi } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';
import {
  useInvalidateAgentLibrary,
  useSkillFiles,
  useSkills,
} from '@/shared/hooks/useAgentLibrary';
import { ConfirmDialog } from '@/shared/dialogs/shared/ConfirmDialog';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
  SettingsTextarea,
} from '../SettingsComponents';
import { useSettingsDirty } from '../SettingsDirtyContext';

function formatBytes(bytes: number | bigint): string {
  const n = Number(bytes);
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function NewSkillForm({ onCreated }: { onCreated: (name: string) => void }) {
  const { t } = useTranslation('settings');
  const invalidate = useInvalidateAgentLibrary();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await agentLibraryApi.createSkill({
        name: name.trim(),
        description: description.trim(),
      });
      await invalidate();
      onCreated(name.trim());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4 rounded-sm border border-border p-base">
      {error && <p className="text-sm text-error">{error}</p>}
      <SettingsField
        label={t('settings.agentLibrary.skills.name')}
        description={t('settings.agentLibrary.nameHelper')}
      >
        <SettingsInput value={name} onChange={setName} placeholder="pdf" />
      </SettingsField>
      <SettingsField
        label={t('settings.agentLibrary.skills.descriptionLabel')}
        description={t('settings.agentLibrary.skills.descriptionHelper')}
      >
        <SettingsInput value={description} onChange={setDescription} />
      </SettingsField>
      <PrimaryButton
        value={t('settings.agentLibrary.skills.create')}
        onClick={create}
        disabled={busy || !name.trim() || !description.trim()}
        actionIcon={busy ? 'spinner' : undefined}
      />
    </div>
  );
}

function SkillEditor({
  name,
  onDeleted,
}: {
  name: string;
  onDeleted: () => void;
}) {
  const { t } = useTranslation('settings');
  const { setDirty } = useSettingsDirty();
  const invalidate = useInvalidateAgentLibrary();
  const { data: files = [], refetch } = useSkillFiles(name);
  const [path, setPath] = useState('SKILL.md');
  const [loaded, setLoaded] = useState<string | null>(null);
  const [contents, setContents] = useState('');
  const [newPath, setNewPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dirty = loaded !== null && contents !== loaded;
  useEffect(() => setDirty('skills', dirty), [dirty, setDirty]);
  useEffect(() => () => setDirty('skills', false), [setDirty]);

  useEffect(() => {
    let cancelled = false;
    setLoaded(null);
    setError(null);
    agentLibraryApi
      .readSkillFile(name, path)
      .then((text) => {
        if (cancelled) return;
        setLoaded(text);
        setContents(text);
      })
      .catch((err) => {
        if (!cancelled)
          setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [name, path]);

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
      await agentLibraryApi.writeSkillFile(name, path, contents);
      setLoaded(contents);
      await invalidate();
      await refetch();
    });

  const addFile = () =>
    run(async () => {
      const target = newPath.trim();
      await agentLibraryApi.writeSkillFile(name, target, '');
      setNewPath('');
      await refetch();
      setPath(target);
    });

  const removeFile = () =>
    run(async () => {
      await agentLibraryApi.deleteSkillFile(name, path);
      await refetch();
      setPath('SKILL.md');
    });

  const removeSkill = async () => {
    const result = await ConfirmDialog.show({
      title: t('settings.agentLibrary.skills.deleteTitle'),
      message: t('settings.agentLibrary.skills.deleteMessage', { name }),
      confirmText: t('settings.agentLibrary.delete'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    await run(async () => {
      await agentLibraryApi.deleteSkill(name);
      await invalidate();
      onDeleted();
    });
  };

  return (
    <div className="space-y-4 rounded-sm border border-border p-base">
      {error && <p className="text-sm text-error">{error}</p>}
      <div className="flex flex-col gap-base md:flex-row">
        <div className="md:w-56 shrink-0 space-y-half">
          <p className="text-sm font-medium text-low">
            {t('settings.agentLibrary.skills.files')}
          </p>
          <div className="divide-y divide-border rounded-sm border border-border">
            {files.map((file) => (
              <button
                key={file.path}
                type="button"
                onClick={() => setPath(file.path)}
                className={cn(
                  'flex w-full items-center justify-between gap-half px-half py-half text-left hover:bg-secondary',
                  file.path === path && 'bg-brand/10'
                )}
              >
                <span className="truncate font-mono text-xs text-normal">
                  {file.path}
                </span>
                <span className="shrink-0 text-xs text-low">
                  {formatBytes(file.size)}
                </span>
              </button>
            ))}
          </div>
          <div className="flex gap-half">
            <div className="flex-1">
              <SettingsInput
                value={newPath}
                onChange={setNewPath}
                placeholder="reference.md"
              />
            </div>
            <PrimaryButton
              variant="tertiary"
              actionIcon={PlusIcon}
              onClick={addFile}
              disabled={busy || !newPath.trim()}
            />
          </div>
        </div>
        <div className="min-w-0 flex-1 space-y-half">
          <p className="font-mono text-xs text-low">{path}</p>
          <SettingsTextarea
            value={contents}
            onChange={setContents}
            rows={14}
            monospace
            disabled={loaded === null}
          />
        </div>
      </div>
      <div className="flex flex-wrap gap-base">
        <PrimaryButton
          value={t('settings.agentLibrary.save')}
          onClick={save}
          disabled={busy || !dirty}
          actionIcon={busy ? 'spinner' : undefined}
        />
        {path !== 'SKILL.md' && (
          <PrimaryButton
            variant="tertiary"
            value={t('settings.agentLibrary.skills.deleteFile')}
            onClick={removeFile}
            disabled={busy}
          />
        )}
        <PrimaryButton
          variant="tertiary"
          value={t('settings.agentLibrary.skills.deleteSkill')}
          onClick={removeSkill}
          disabled={busy}
        />
      </div>
    </div>
  );
}

/** Skill folders made available to agents in their containers. */
export function SkillsCard() {
  const { t } = useTranslation('settings');
  const { data: skills = [] } = useSkills();
  // `null` = nothing open, '' = a new skill, otherwise the skill's name.
  const [open, setOpen] = useState<string | null>(null);

  return (
    <SettingsCard
      title={t('settings.agentLibrary.skills.title')}
      description={t('settings.agentLibrary.skills.description')}
      headerAction={
        <PrimaryButton
          variant="secondary"
          actionIcon={PlusIcon}
          value={t('settings.agentLibrary.skills.new')}
          onClick={() => setOpen('')}
        />
      }
    >
      <div className="divide-y divide-border rounded-sm border border-border">
        {skills.map((skill) => (
          <button
            key={skill.name}
            type="button"
            onClick={() => setOpen(skill.name === open ? null : skill.name)}
            className={cn(
              'flex w-full flex-wrap items-center gap-x-base gap-y-half px-base py-half text-left hover:bg-secondary',
              skill.name === open && 'bg-brand/10'
            )}
          >
            <span className="font-mono text-sm text-high">{skill.name}</span>
            <span className="text-xs text-low">
              {t('settings.agentLibrary.skills.fileCount', {
                count: Number(skill.file_count),
              })}{' '}
              · {formatBytes(skill.total_bytes)}
            </span>
            <span className="text-xs text-low">
              {skill.used_by.length > 0
                ? t('settings.agentLibrary.usedBy', {
                    names: skill.used_by.join(', '),
                  })
                : t('settings.agentLibrary.unused')}
            </span>
            {skill.error && (
              <span className="flex items-center gap-half text-xs text-error">
                <WarningIcon className="size-icon-xs" />
                {skill.error}
              </span>
            )}
            {skill.description && (
              <span className="basis-full truncate text-sm text-low">
                {skill.description}
              </span>
            )}
          </button>
        ))}
        {skills.length === 0 && (
          <p className="px-base py-half text-sm text-low">
            {t('settings.agentLibrary.skills.empty')}
          </p>
        )}
      </div>
      {open === '' && <NewSkillForm onCreated={setOpen} />}
      {open && (
        <SkillEditor key={open} name={open} onDeleted={() => setOpen(null)} />
      )}
    </SettingsCard>
  );
}
