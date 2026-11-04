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

/** One fixed hue per role, so the stripe on an agent row says "tester" before
 * you've read a single word. Spread around the wheel on purpose. */
export const ROLE_HUES: Record<AgentRole, number> = {
  planner: 265,
  coder: 210,
  tester: 140,
  reviewer: 45,
  conflict_resolver: 0,
  scribe: 300,
  intake: 180,
  history: 25,
  setup: 95,
  triage: 330,
};

export function roleColor(role: AgentRole): string {
  return `hsl(${ROLE_HUES[role]} 65% 55%)`;
}

// Known model families get a fixed hue so "which of these is the expensive
// one" is answerable at a glance. Everything else gets a hashed hue: stable,
// just not meaningful.
const MODEL_FAMILY_HUES: [RegExp, number][] = [
  [/opus/i, 280],
  [/sonnet/i, 205],
  [/haiku/i, 160],
  [/gpt|o[134]-|codex/i, 120],
  [/gemini/i, 50],
  [/deepseek/i, 230],
  [/qwen/i, 330],
  [/llama|mistral/i, 20],
];

export function modelColor(model: string): string {
  const family = MODEL_FAMILY_HUES.find(([pattern]) => pattern.test(model));
  let hue = family?.[1];
  if (hue === undefined) {
    let hash = 0;
    for (const ch of model) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
    hue = Math.abs(hash) % 360;
  }
  return `hsl(${hue} 65% 55%)`;
}
