import { useState } from 'react';
import type { AgentRole } from 'shared/types';
import { roleColor } from '@/shared/lib/agents';

export type RoleSetup = Partial<Record<AgentRole, string>>;

/**
 * Roles whose runs get a workspace sandbox, so role setup applies to them.
 * Mirrors `project_setup::SETUP_ROLES`; headless jobs (intake, scribe,
 * setup, triage, history) never run it.
 */
export const SETUP_ROLES: AgentRole[] = [
  'planner',
  'coder',
  'tester',
  'reviewer',
  'conflict_resolver',
];

export const roleName = (role: AgentRole) => role.replace(/_/g, ' ');

/** Drops blank entries so the saved settings only hold real commands. */
export function cleanRoleSetup(value: RoleSetup): RoleSetup {
  const out: RoleSetup = {};
  for (const [role, commands] of Object.entries(value)) {
    if (commands?.trim()) out[role as AgentRole] = commands.trim();
  }
  return out;
}

const textareaClass =
  'min-h-12 rounded-sm border border-border bg-secondary px-base py-half font-mono text-sm';

/**
 * One commands box per sandboxed role. Roles with commands start open;
 * the rest stay folded so five empty textareas don't eat the dialog.
 */
export function RoleSetupEditor({
  value,
  onChange,
  idPrefix,
}: {
  value: RoleSetup;
  onChange: (value: RoleSetup) => void;
  idPrefix: string;
}) {
  return (
    <div className="flex flex-col gap-half">
      {SETUP_ROLES.map((role) => (
        <RoleRow
          key={role}
          role={role}
          id={`${idPrefix}-${role}`}
          commands={value[role] ?? ''}
          onChange={(commands) => onChange({ ...value, [role]: commands })}
        />
      ))}
    </div>
  );
}

function RoleRow({
  role,
  id,
  commands,
  onChange,
}: {
  role: AgentRole;
  id: string;
  commands: string;
  onChange: (commands: string) => void;
}) {
  // Owned here, not derived from `commands`: clearing the box mid-edit must
  // not slam it shut under the cursor.
  const [open, setOpen] = useState(() => !!commands.trim());
  const lineCount = commands.trim() ? commands.trim().split('\n').length : 0;
  return (
    <details
      open={open}
      onToggle={(e) => setOpen(e.currentTarget.open)}
      className="rounded-sm border border-border"
      style={{ borderLeft: `3px solid ${roleColor(role)}` }}
    >
      <summary className="cursor-pointer px-base py-half text-sm">
        <span className="capitalize">{roleName(role)}</span>
        <span className="ml-base text-xs text-low">
          {lineCount
            ? `${lineCount} line${lineCount === 1 ? '' : 's'}`
            : 'nothing extra'}
        </span>
      </summary>
      <div className="px-base pb-half">
        <textarea
          id={id}
          data-testid={id}
          aria-label={`${roleName(role)} setup commands`}
          className={`w-full ${textareaClass}`}
          placeholder={
            role === 'tester'
              ? 'cd /workspace/web && npx playwright install chromium'
              : ''
          }
          value={commands}
          onChange={(e) => onChange(e.target.value)}
        />
      </div>
    </details>
  );
}
