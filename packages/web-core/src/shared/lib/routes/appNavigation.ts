export type AppDestination =
  | { kind: 'root' }
  | { kind: 'onboarding' }
  | { kind: 'workspaces' }
  | { kind: 'workspaces-create' }
  | { kind: 'workspace'; workspaceId: string }
  | { kind: 'workspace-vscode'; workspaceId: string }
  | { kind: 'tickets' };

export type NavigationTransition = {
  replace?: boolean;
};

export interface AppNavigation {
  resolveFromPath(path: string): AppDestination | null;
  goToRoot(transition?: NavigationTransition): void;
  goToOnboarding(transition?: NavigationTransition): void;
  goToWorkspaces(transition?: NavigationTransition): void;
  goToWorkspacesCreate(transition?: NavigationTransition): void;
  goToWorkspace(workspaceId: string, transition?: NavigationTransition): void;
  goToWorkspaceVsCode(
    workspaceId: string,
    transition?: NavigationTransition
  ): void;
  goToTickets(transition?: NavigationTransition): void;
}

type WorkspaceDestinationKind =
  'workspaces' | 'workspaces-create' | 'workspace' | 'workspace-vscode';

export type WorkspaceDestination = Extract<
  AppDestination,
  { kind: WorkspaceDestinationKind }
>;

export function isWorkspacesDestination(
  destination: AppDestination | null
): destination is WorkspaceDestination {
  if (!destination) {
    return false;
  }

  switch (destination.kind) {
    case 'workspaces':
    case 'workspaces-create':
    case 'workspace':
    case 'workspace-vscode':
      return true;
    default:
      return false;
  }
}
