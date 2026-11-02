import { BaseCodingAgent } from 'shared/types';
import type { AgentRole, AgentRuntime, AgentSummary } from 'shared/types';

/** The executor that runs an agent runtime. */
export const RUNTIME_EXECUTORS: Record<AgentRuntime, BaseCodingAgent> = {
  'claude-code': BaseCodingAgent.CLAUDE_CODE,
  opencode: BaseCodingAgent.OPENCODE,
};

/**
 * The agent a role resolves to: the one named after the role
 * (`conflict-resolver` matches `conflict_resolver`), else the first valid
 * definition with that role. Mirrors `AgentLibrary::agent_name_for_role`.
 */
export function agentForRole(
  agents: AgentSummary[],
  role: AgentRole
): AgentSummary | null {
  const withRole = agents.filter(
    (a) => a.definition && a.definition.role === role
  );
  return (
    withRole.find((a) => a.name.replace(/-/g, '_') === role) ??
    withRole[0] ??
    null
  );
}
