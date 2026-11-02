import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useMemo } from 'react';
import { agentLibraryApi } from '@/shared/lib/api';
import { agentForRole } from '@/shared/lib/agents';
import type { AgentRole } from 'shared/types';

export const agentLibraryKeys = {
  all: ['agent-library'] as const,
  meta: ['agent-library', 'meta'] as const,
  agents: ['agent-library', 'agents'] as const,
  styles: ['agent-library', 'styles'] as const,
  skills: ['agent-library', 'skills'] as const,
  skillFiles: (name: string) =>
    ['agent-library', 'skills', name, 'files'] as const,
};

/** The library changes on disk too (file watcher), so refetch on focus. */
const LIVE = { refetchOnWindowFocus: true, staleTime: 5_000 };

export function useAgentLibraryMeta() {
  return useQuery({
    queryKey: agentLibraryKeys.meta,
    queryFn: () => agentLibraryApi.meta(),
    staleTime: Infinity,
  });
}

export function useAgents() {
  return useQuery({
    queryKey: agentLibraryKeys.agents,
    queryFn: () => agentLibraryApi.listAgents(),
    ...LIVE,
  });
}

/**
 * The agent a role resolves to (named after the role, else the first valid
 * definition with that role), for system-initiated runs like conflict
 * resolution and review. Null when no agent has the role.
 */
export function useAgentForRole(role: AgentRole | null) {
  const { data: agents = [], isLoading } = useAgents();
  const agent = useMemo(
    () => (role ? agentForRole(agents, role) : null),
    [agents, role]
  );
  return { agent, isLoading: role != null && isLoading };
}

export function useStyles() {
  return useQuery({
    queryKey: agentLibraryKeys.styles,
    queryFn: () => agentLibraryApi.listStyles(),
    ...LIVE,
  });
}

export function useSkills() {
  return useQuery({
    queryKey: agentLibraryKeys.skills,
    queryFn: () => agentLibraryApi.listSkills(),
    ...LIVE,
  });
}

export function useSkillFiles(name: string | null) {
  return useQuery({
    queryKey: agentLibraryKeys.skillFiles(name ?? ''),
    queryFn: () => agentLibraryApi.listSkillFiles(name!),
    enabled: !!name,
  });
}

/** Refetch everything after a change; styles and skills list their users. */
export function useInvalidateAgentLibrary() {
  const queryClient = useQueryClient();
  return useCallback(
    () => queryClient.invalidateQueries({ queryKey: agentLibraryKeys.all }),
    [queryClient]
  );
}
