import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  DownloadSimpleIcon,
  FolderOpenIcon,
  PlusIcon,
  UploadSimpleIcon,
  WarningIcon,
} from '@phosphor-icons/react';
import type { AgentDefinition, ImportSummary } from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { agentLibraryApi } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import {
  useAgentLibraryMeta,
  useAgents,
  useInvalidateAgentLibrary,
  useSkills,
  useStyles,
} from '@/shared/hooks/useAgentLibrary';
import { ConfirmDialog } from '@/shared/dialogs/shared/ConfirmDialog';
import { SettingsCard, TwoColumnPickerBadge } from './SettingsComponents';
import { useSettingsDirty } from './SettingsDirtyContext';
import { AgentEditor, newAgentDraft } from './agent-library/AgentEditor';
import { StylesCard } from './agent-library/StylesCard';
import { SkillsCard } from './agent-library/SkillsCard';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Import zips or files, or a whole folder, and export the library. */
function LibraryCard() {
  const { t } = useTranslation('settings');
  const invalidate = useInvalidateAgentLibrary();
  const filesRef = useRef<HTMLInputElement>(null);
  const folderRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ImportSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // `webkitdirectory` isn't in React's input props.
    folderRef.current?.setAttribute('webkitdirectory', '');
  }, []);

  const upload = async (list: FileList | null) => {
    if (!list || list.length === 0) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const files = Array.from(list).map((file) => ({
        file,
        path: file.webkitRelativePath || file.name,
      }));
      setResult(await agentLibraryApi.importFiles(files));
      await invalidate();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
      if (filesRef.current) filesRef.current.value = '';
      if (folderRef.current) folderRef.current.value = '';
    }
  };

  const imported = result
    ? [...result.agents, ...result.styles, ...result.skills].length
    : 0;

  return (
    <SettingsCard
      title={t('settings.agentLibrary.library.title')}
      description={t('settings.agentLibrary.library.description')}
    >
      <div className="flex flex-wrap gap-base">
        <PrimaryButton
          variant="secondary"
          actionIcon={busy ? 'spinner' : UploadSimpleIcon}
          value={t('settings.agentLibrary.library.importFiles')}
          onClick={() => filesRef.current?.click()}
          disabled={busy}
        />
        <PrimaryButton
          variant="secondary"
          actionIcon={FolderOpenIcon}
          value={t('settings.agentLibrary.library.importFolder')}
          onClick={() => folderRef.current?.click()}
          disabled={busy}
        />
        <a
          href={agentLibraryApi.exportUrl()}
          download
          className="inline-flex items-center gap-half rounded-sm px-base py-half text-sm text-normal hover:bg-secondary"
        >
          <DownloadSimpleIcon className="size-icon-sm" />
          {t('settings.agentLibrary.library.exportAll')}
        </a>
        <input
          ref={filesRef}
          type="file"
          multiple
          accept=".zip,.yaml,.yml,.md"
          className="hidden"
          onChange={(e) => upload(e.target.files)}
        />
        <input
          ref={folderRef}
          type="file"
          multiple
          className="hidden"
          onChange={(e) => upload(e.target.files)}
        />
      </div>
      {error && <p className="text-sm text-error">{error}</p>}
      {result && (
        <div className="space-y-half text-sm">
          <p className="text-normal">
            {t('settings.agentLibrary.library.imported', { count: imported })}
          </p>
          {result.replaced.length > 0 && (
            <p className="text-low">
              {t('settings.agentLibrary.library.replaced', {
                items: result.replaced.join(', '),
              })}
            </p>
          )}
          {result.skipped.map((line) => (
            <p key={line} className="font-mono text-xs text-low">
              {line}
            </p>
          ))}
        </div>
      )}
    </SettingsCard>
  );
}

function AgentsCard({ onClose }: { onClose?: () => void }) {
  const { t } = useTranslation('settings');
  const appNavigation = useAppNavigation();
  const { setDirty } = useSettingsDirty();
  const invalidate = useInvalidateAgentLibrary();
  const { data: meta } = useAgentLibraryMeta();
  const { data: agents = [] } = useAgents();
  const { data: styles = [] } = useStyles();
  const { data: skills = [] } = useSkills();
  // `null` = nothing open, '' = a new agent, otherwise the agent's name.
  const [open, setOpen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const draftForNew = useMemo(newAgentDraft, []);
  const opened = agents.find((a) => a.name === open);
  const initial: AgentDefinition | null =
    open === '' ? draftForNew : (opened?.definition ?? null);

  const onDirtyChange = useCallback(
    (dirty: boolean) => setDirty('agents', dirty),
    [setDirty]
  );
  useEffect(() => () => setDirty('agents', false), [setDirty]);

  const close = () => {
    setOpen(null);
    setError(null);
    setDirty('agents', false);
  };

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const save = (agent: AgentDefinition) =>
    run(async () => {
      if (open) {
        await agentLibraryApi.updateAgent(open, agent);
      } else {
        await agentLibraryApi.createAgent(agent);
      }
      await invalidate();
      setDirty('agents', false);
      setOpen(agent.name);
    });

  const remove = async () => {
    if (!open) return;
    const result = await ConfirmDialog.show({
      title: t('settings.agentLibrary.agents.deleteTitle'),
      message: t('settings.agentLibrary.agents.deleteMessage', { name: open }),
      confirmText: t('settings.agentLibrary.delete'),
      variant: 'destructive',
    });
    if (result !== 'confirmed') return;
    await run(async () => {
      await agentLibraryApi.deleteAgent(open);
      await invalidate();
      close();
    });
  };

  const testRun = () =>
    run(async () => {
      if (!open) return;
      const { workspace } = await agentLibraryApi.testRun(open, {});
      setDirty('agents', false);
      onClose?.();
      appNavigation.goToWorkspace(workspace.id);
    });

  return (
    <SettingsCard
      title={t('settings.agentLibrary.agents.title')}
      description={t('settings.agentLibrary.agents.description')}
      headerAction={
        <PrimaryButton
          variant="secondary"
          actionIcon={PlusIcon}
          value={t('settings.agentLibrary.agents.new')}
          onClick={() => {
            setError(null);
            setOpen('');
          }}
          disabled={busy}
        />
      }
    >
      <div className="divide-y divide-border rounded-sm border border-border">
        {agents.map((agent) => {
          const def = agent.definition;
          const problem = agent.error ?? agent.missing.join(', ');
          return (
            <button
              key={agent.name}
              type="button"
              onClick={() => {
                setError(null);
                setOpen(agent.name === open ? null : agent.name);
              }}
              className={cn(
                'flex w-full flex-wrap items-center gap-half px-base py-half text-left hover:bg-secondary',
                agent.name === open && 'bg-brand/10'
              )}
            >
              <span className="font-mono text-sm text-high">{agent.name}</span>
              {def && (
                <>
                  <TwoColumnPickerBadge>
                    {t(`settings.agentLibrary.roles.${def.role}`)}
                  </TwoColumnPickerBadge>
                  <TwoColumnPickerBadge variant="brand">
                    {t(`settings.agentLibrary.runtimes.${def.runtime}`)}
                  </TwoColumnPickerBadge>
                  <span className="text-sm text-low">{def.model}</span>
                </>
              )}
              {problem && (
                <span
                  className="flex items-center gap-half text-xs text-error"
                  title={problem}
                >
                  <WarningIcon className="size-icon-xs" />
                  <span className="max-w-[24rem] truncate">{problem}</span>
                </span>
              )}
              {def?.description && (
                <span className="basis-full truncate text-sm text-low">
                  {def.description}
                </span>
              )}
            </button>
          );
        })}
        {agents.length === 0 && (
          <p className="px-base py-half text-sm text-low">
            {t('settings.agentLibrary.agents.empty')}
          </p>
        )}
      </div>

      {open !== null && opened && !opened.definition && (
        <p className="text-sm text-error">
          {t('settings.agentLibrary.agents.brokenFile', {
            error: opened.error ?? '',
          })}
        </p>
      )}
      {open !== null && initial && meta && (
        <>
          <AgentEditor
            key={open || 'new'}
            originalName={open || null}
            initial={initial}
            meta={meta}
            styles={styles}
            skills={skills}
            busy={busy}
            error={error}
            onDirtyChange={onDirtyChange}
            onSave={save}
            onDelete={remove}
            onTestRun={testRun}
            onCancel={close}
          />
          {open && (
            <a
              href={agentLibraryApi.exportUrl([open])}
              download
              className="inline-flex items-center gap-half text-sm text-low hover:text-normal"
            >
              <DownloadSimpleIcon className="size-icon-sm" />
              {t('settings.agentLibrary.agents.export')}
            </a>
          )}
        </>
      )}
    </SettingsCard>
  );
}

export function AgentsSettingsSection({ onClose }: { onClose?: () => void }) {
  return (
    <>
      <AgentsCard onClose={onClose} />
      <StylesCard />
      <SkillsCard />
      <LibraryCard />
    </>
  );
}
