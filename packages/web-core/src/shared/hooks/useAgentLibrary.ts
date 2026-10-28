import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback } from 'react';
import { agentLibraryApi } from '@/shared/lib/api';

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
