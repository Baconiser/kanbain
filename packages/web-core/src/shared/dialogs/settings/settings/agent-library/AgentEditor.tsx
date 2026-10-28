import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isEqual } from 'lodash';
import type {
  AgentDefinition,
  AgentLibraryMeta,
  AgentRole,
  AgentRuntime,
  Capability,
  SkillSummary,
  StyleSummary,
} from 'shared/types';
import { PrimaryButton } from '@kanbain/ui/components/PrimaryButton';
import { cn } from '@/shared/lib/utils';
import {
  SettingsField,
  SettingsInput,
  SettingsSelect,
  SettingsTextarea,
} from '../SettingsComponents';

export function newAgentDraft(): AgentDefinition {
  return {
    name: '',
    description: null,
    role: 'coder',
    runtime: 'claude-code',
    model: 'sonnet',
    role_prompt: '',
    styles: [],
    skills: [],
    capabilities: ['read', 'edit', 'bash'],
    limits: {},
  };
}

function numberOrNull(value: string): number | null {
  if (!value.trim()) return null;
  const n = Number.parseFloat(value);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function toggle<T>(list: T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

/** A row of toggleable chips, numbered in selection order when `ordered`. */
function ChipPicker({
  options,
  selected,
  onToggle,
  ordered,
  empty,
  label,
}: {
  options: string[];
  selected: string[];
  onToggle: (value: string) => void;
  ordered?: boolean;
  empty: string;
  label?: (value: string) => string;
}) {
  if (options.length === 0) {
    return <p className="text-sm text-low">{empty}</p>;
  }
  return (
    <div className="flex flex-wrap gap-half">
      {options.map((option) => {
        const index = selected.indexOf(option);
        const active = index >= 0;
        return (
          <button
            key={option}
            type="button"
            onClick={() => onToggle(option)}
            className={cn(
              'rounded-sm border px-base py-half text-sm transition-colors',
              active
                ? 'border-brand bg-brand/10 text-brand'
                : 'border-border text-normal hover:bg-secondary'
            )}
          >
            {ordered && active ? `${index + 1}. ` : ''}
            {label ? label(option) : option}
          </button>
        );
      })}
    </div>
  );
}

interface AgentEditorProps {
  /** `null` when creating an agent. */
  originalName: string | null;
  initial: AgentDefinition;
  meta: AgentLibraryMeta;
  styles: StyleSummary[];
  skills: SkillSummary[];
  busy: boolean;
  error: string | null;
  onDirtyChange: (dirty: boolean) => void;
  onSave: (agent: AgentDefinition) => void;
  onDelete: () => void;
  onTestRun: () => void;
  onCancel: () => void;
}

export function AgentEditor({
  originalName,
  initial,
  meta,
  styles,
  skills,
  busy,
  error,
  onDirtyChange,
  onSave,
  onDelete,
  onTestRun,
  onCancel,
}: AgentEditorProps) {
  const { t } = useTranslation('settings');
  const [draft, setDraft] = useState<AgentDefinition>(initial);
  const [mcp, setMcp] = useState(() =>
    initial.mcp_servers ? JSON.stringify(initial.mcp_servers, null, 2) : ''
  );
  const [mcpError, setMcpError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(initial);
    setMcp(
      initial.mcp_servers ? JSON.stringify(initial.mcp_servers, null, 2) : ''
    );
  }, [initial]);

  const dirty =
    !isEqual(draft, initial) ||
    mcp !==
      (initial.mcp_servers ? JSON.stringify(initial.mcp_servers, null, 2) : '');
  useEffect(() => onDirtyChange(dirty), [dirty, onDirtyChange]);

  const update = (patch: Partial<AgentDefinition>) =>
    setDraft((prev) => ({ ...prev, ...patch }));

  const models =
    draft.runtime === 'opencode'
      ? meta.opencode_models
      : meta.claude_code_models;
  const filtered =
    draft.capabilities.includes('test') && !draft.capabilities.includes('bash');
  const bashAllow = useMemo(
    () => (draft.bash_allow ?? []).join('\n'),
    [draft.bash_allow]
  );

  const save = () => {
    let mcpServers: AgentDefinition['mcp_servers'] = null;
    if (mcp.trim()) {
      try {
        const parsed: unknown = JSON.parse(mcp);
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
          throw new Error('not an object');
        }
        mcpServers = parsed as Record<string, unknown>;
      } catch {
        setMcpError(t('settings.agentLibrary.agents.mcpInvalid'));
        return;
      }
    }
    setMcpError(null);
    onSave({
      ...draft,
      name: draft.name.trim(),
      description: draft.description?.trim() || null,
      mcp_servers: mcpServers,
    });
  };

  return (
    <div className="space-y-4 rounded-sm border border-border p-base">
      {error && (
        <div className="rounded-sm border border-error/50 bg-error/10 p-base text-sm text-error">
          {error}
        </div>
      )}
      <div className="grid gap-4 md:grid-cols-2">
        <SettingsField
          label={t('settings.agentLibrary.agents.name')}
          description={t('settings.agentLibrary.nameHelper')}
        >
          <SettingsInput
            value={draft.name}
            onChange={(name) => update({ name })}
            placeholder="backend-coder"
          />
        </SettingsField>
        <SettingsField
          label={t('settings.agentLibrary.agents.descriptionLabel')}
        >
          <SettingsInput
            value={draft.description ?? ''}
            onChange={(description) => update({ description })}
          />
        </SettingsField>
        <SettingsField label={t('settings.agentLibrary.agents.role')}>
          <SettingsSelect<AgentRole>
            value={draft.role}
            options={meta.roles.map((role) => ({
              value: role,
              label: t(`settings.agentLibrary.roles.${role}`),
            }))}
            onChange={(role) => update({ role })}
          />
        </SettingsField>
        <SettingsField label={t('settings.agentLibrary.agents.runtime')}>
          <SettingsSelect<AgentRuntime>
            value={draft.runtime}
            options={meta.runtimes.map((runtime) => ({
              value: runtime,
              label: t(`settings.agentLibrary.runtimes.${runtime}`),
            }))}
            onChange={(runtime) => update({ runtime })}
          />
        </SettingsField>
      </div>

      <SettingsField
        label={t('settings.agentLibrary.agents.model')}
        description={t('settings.agentLibrary.agents.modelHelper')}
      >
        <SettingsInput
          value={draft.model}
          onChange={(model) => update({ model })}
        />
        <div className="mt-half flex flex-wrap gap-half">
          {models.map((model) => (
            <button
              key={model.id}
              type="button"
              onClick={() => update({ model: model.id })}
              className="rounded-sm bg-secondary px-half text-xs text-low hover:text-normal"
            >
              {model.label}
            </button>
          ))}
        </div>
      </SettingsField>

      <SettingsField
        label={t('settings.agentLibrary.agents.rolePrompt')}
        description={t('settings.agentLibrary.agents.rolePromptHelper')}
      >
        <SettingsTextarea
          value={draft.role_prompt}
          onChange={(role_prompt) => update({ role_prompt })}
          rows={6}
        />
      </SettingsField>

      <SettingsField
        label={t('settings.agentLibrary.agents.styles')}
        description={t('settings.agentLibrary.agents.stylesHelper')}
      >
        <ChipPicker
          options={styles.map((s) => s.name)}
          selected={draft.styles}
          onToggle={(name) => update({ styles: toggle(draft.styles, name) })}
          ordered
          empty={t('settings.agentLibrary.styles.empty')}
        />
      </SettingsField>

      <SettingsField
        label={t('settings.agentLibrary.agents.skills')}
        description={t('settings.agentLibrary.agents.skillsHelper')}
      >
        <ChipPicker
          options={skills.map((s) => s.name)}
          selected={draft.skills}
          onToggle={(name) => update({ skills: toggle(draft.skills, name) })}
          empty={t('settings.agentLibrary.skills.empty')}
        />
      </SettingsField>

      <SettingsField
        label={t('settings.agentLibrary.agents.capabilities')}
        description={t('settings.agentLibrary.agents.capabilitiesHelper')}
      >
        <ChipPicker
          options={meta.capabilities}
          selected={draft.capabilities}
          onToggle={(cap) =>
            update({
              capabilities: toggle(draft.capabilities, cap as Capability),
            })
          }
          empty=""
          label={(cap) => t(`settings.agentLibrary.capabilities.${cap}`)}
        />
      </SettingsField>

      {filtered && (
        <SettingsField
          label={t('settings.agentLibrary.agents.bashAllow')}
          description={t('settings.agentLibrary.agents.bashAllowHelper')}
        >
          <SettingsTextarea
            value={bashAllow}
            onChange={(value) =>
              update({
                bash_allow: value
                  .split('\n')
                  .map((line) => line.trim())
                  .filter(Boolean),
              })
            }
            placeholder={meta.default_test_commands.join('\n')}
            rows={5}
            monospace
          />
        </SettingsField>
      )}

      <div className="grid gap-4 md:grid-cols-3">
        <SettingsField label={t('settings.agentLibrary.agents.maxSteps')}>
          <SettingsInput
            value={draft.limits.max_steps?.toString() ?? ''}
            onChange={(value) => {
              const n = numberOrNull(value);
              update({
                limits: {
                  ...draft.limits,
                  max_steps: n === null ? null : Math.round(n),
                },
              });
            }}
          />
        </SettingsField>
        <SettingsField label={t('settings.agentLibrary.agents.budget')}>
          <SettingsInput
            value={draft.limits.budget_usd?.toString() ?? ''}
            onChange={(value) =>
              update({
                limits: { ...draft.limits, budget_usd: numberOrNull(value) },
              })
            }
          />
        </SettingsField>
        <SettingsField label={t('settings.agentLibrary.agents.wallMinutes')}>
          <SettingsInput
            value={draft.limits.wall_minutes?.toString() ?? ''}
            onChange={(value) => {
              const n = numberOrNull(value);
              update({
                limits: {
                  ...draft.limits,
                  wall_minutes: n === null ? null : Math.round(n),
                },
              });
            }}
          />
        </SettingsField>
      </div>
      <p className="text-sm text-low">
        {t('settings.agentLibrary.agents.limitsHelper')}
      </p>

      <SettingsField
        label={t('settings.agentLibrary.agents.mcpServers')}
        description={t('settings.agentLibrary.agents.mcpServersHelper')}
        error={mcpError}
      >
        <SettingsTextarea
          value={mcp}
          onChange={setMcp}
          placeholder={
            '{\n  "docs": { "command": "npx", "args": ["-y", "some-mcp"] }\n}'
          }
          rows={4}
          monospace
        />
      </SettingsField>

      <div className="flex flex-wrap items-center gap-base">
        <PrimaryButton
          value={t('settings.agentLibrary.save')}
          onClick={save}
          disabled={busy || !dirty || !draft.name.trim()}
          actionIcon={busy ? 'spinner' : undefined}
        />
        <PrimaryButton
          variant="tertiary"
          value={t('settings.agentLibrary.cancel')}
          onClick={onCancel}
          disabled={busy}
        />
        {originalName && (
          <>
            <PrimaryButton
              variant="secondary"
              value={t('settings.agentLibrary.agents.testRun')}
              onClick={onTestRun}
              disabled={busy || dirty}
            />
            <PrimaryButton
              variant="tertiary"
              value={t('settings.agentLibrary.delete')}
              onClick={onDelete}
              disabled={busy}
            />
          </>
        )}
      </div>
      {originalName && dirty && (
        <p className="text-sm text-low">
          {t('settings.agentLibrary.agents.saveBeforeTest')}
        </p>
      )}
    </div>
  );
}
