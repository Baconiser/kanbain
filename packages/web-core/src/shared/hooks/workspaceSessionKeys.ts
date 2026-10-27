export const workspaceSessionKeys = {
  byWorkspace: (workspaceId: string | undefined) =>
    ['workspaceSessions', workspaceId] as const,
};
