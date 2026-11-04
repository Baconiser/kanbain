import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { XIcon } from '@phosphor-icons/react';
import { Input } from '@kanbain/ui/components/Input';
import { Label } from '@kanbain/ui/components/Label';
import type { AgentModelOverrides, AgentRole } from 'shared/types';
import { useAgentLibraryMeta, useAgents } from '@/shared/hooks/useAgentLibrary';
import { agentForRole } from '@/shared/lib/agents';

const DATALIST_ID = 'project-agent-model-options';

/** Claude Code only runs Anthropic models; `provider/model` needs opencode. */
const needsOpencode = (model: string) => model.includes('/');

/** Drops blank entries so the saved settings only hold real overrides. */
export function cleanAgentModels(
  value: AgentModelOverrides
): AgentModelOverrides {
  const roles: AgentModelOverrides['roles'] = {};
  for (const [role, model] of Object.entries(value.roles)) {
    if (model?.trim()) roles[role as AgentRole] = model.trim();
  }
  return { default: value.default?.trim() || null, roles };
}

/**
 * Which model each agent role runs on in this project: a role's own entry,
 * else the project default, else the agent file's model (Settings → Agents).
 */
export function ProjectAgentModels({
  value,
  onChange,
}: {
  value: AgentModelOverrides;
  onChange: (value: AgentModelOverrides) => void;
}) {
  const { t } = useTranslation('settings');
  const { data: meta } = useAgentLibraryMeta();
  const { data: agents = [] } = useAgents();

  const options = useMemo(() => {
    const seen = new Set<string>();
    return [
      ...(meta?.claude_code_models ?? []),
      ...(meta?.opencode_models ?? []),
    ].filter((m) => !seen.has(m.id) && !!seen.add(m.id));
  }, [meta]);

  const setRole = (role: AgentRole, model: string) =>
    onChange({ ...value, roles: { ...value.roles, [role]: model } });

  const projectDefault = value.default?.trim() ?? '';

  return (
    <div className="col-span-2 flex flex-col gap-half">
      <Label htmlFor="project-agent-model-default">Agent models</Label>
      <p className="text-xs text-low">
        Override the model (and with it the provider) this project’s agents run
        on. Empty fields keep the model from Settings → Agents. Models like{' '}
        <code>openai/…</code> or a custom provider’s run on opencode.
      </p>
      <datalist id={DATALIST_ID}>
        {options.map((m) => (
          <option key={m.id} value={m.id}>
            {m.label}
          </option>
        ))}
      </datalist>
      <div className="grid grid-cols-[140px_1fr] items-center gap-half">
        <span className="text-sm">All agents</span>
        <Input
          id="project-agent-model-default"
          list={DATALIST_ID}
          placeholder="Agent file’s model"
          value={value.default ?? ''}
          onChange={(e) => onChange({ ...value, default: e.target.value })}
        />
        {(meta?.roles ?? []).map((role) => {
          const agent = agentForRole(agents, role)?.definition;
          const own = value.roles[role]?.trim() ?? '';
          const inherited = projectDefault || agent?.model || '';
          const effective = own || inherited;
          const switches =
            agent?.runtime === 'claude-code' &&
            !!effective &&
            effective !== agent.model &&
            needsOpencode(effective);
          return (
            <RoleRow
              key={role}
              label={t(`settings.agentLibrary.roles.${role}`, {
                defaultValue: role,
              })}
              value={value.roles[role] ?? ''}
              placeholder={inherited}
              hint={switches ? 'runs on opencode in this project' : null}
              onChange={(model) => setRole(role, model)}
            />
          );
        })}
      </div>
    </div>
  );
}

function RoleRow({
  label,
  value,
  placeholder,
  hint,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  hint: string | null;
  onChange: (value: string) => void;
}) {
  return (
    <>
      <span className="text-sm text-low">{label}</span>
      <div className="flex flex-col gap-half">
        <div className="flex items-center gap-half">
          <Input
            list={DATALIST_ID}
            placeholder={placeholder}
            value={value}
            onChange={(e) => onChange(e.target.value)}
          />
          {value && (
            <button
              type="button"
              aria-label={`Clear ${label} model`}
              className="text-low hover:text-normal"
              onClick={() => onChange('')}
            >
              <XIcon size={14} />
            </button>
          )}
        </div>
        {hint && <span className="text-xs text-low">{hint}</span>}
      </div>
    </>
  );
}
