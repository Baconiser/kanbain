import { useCallback, useEffect, useState } from 'react';
import { Outlet } from '@tanstack/react-router';
import { XIcon, LayoutIcon, KanbanIcon } from '@phosphor-icons/react';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import { useUiPreferencesStore } from '@/shared/stores/useUiPreferencesStore';
import { cn } from '@/shared/lib/utils';
import { isTauriMac } from '@/shared/lib/platform';

import { NavbarContainer } from './NavbarContainer';
import { AppBar } from '@kanbain/ui/components/AppBar';
import { MobileDrawer } from '@kanbain/ui/components/MobileDrawer';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useCurrentAppDestination } from '@/shared/hooks/useCurrentAppDestination';
import { isWorkspacesDestination } from '@/shared/lib/routes/appNavigation';
import { CommandBarDialog } from '@/shared/dialogs/command-bar/CommandBarDialog';
import { useCommandBarShortcut } from '@/shared/hooks/useCommandBarShortcut';
import { useWorkspaceSidebarPreviewController } from '@/shared/hooks/useWorkspaceSidebarPreviewController';
import { WorkspacesSidebarContainer } from '@/pages/workspaces/WorkspacesSidebarContainer';
import { WorkspacesSidebarReopenTag } from '@kanbain/ui/components/WorkspacesSidebar';

export function SharedAppLayout() {
  const appNavigation = useAppNavigation();
  const currentDestination = useCurrentAppDestination();
  const isMobile = useIsMobile();
  const mobileFontScale = useUiPreferencesStore((s) => s.mobileFontScale);
  const isLeftSidebarVisible = useUiPreferencesStore(
    (s) => s.isLeftSidebarVisible
  );
  const { appVersion } = useUserSystem();
  const [isDrawerOpen, setIsDrawerOpen] = useState(false);
  const [isAppBarHovered, setIsAppBarHovered] = useState(false);

  // Register CMD+K shortcut globally for all routes under SharedAppLayout
  useCommandBarShortcut(() => CommandBarDialog.show());

  // Apply mobile font scale CSS variable
  useEffect(() => {
    if (!isMobile) {
      document.documentElement.style.removeProperty('--mobile-font-scale');
      return;
    }
    const scaleMap = { default: '1', small: '0.9', smaller: '0.8' } as const;
    document.documentElement.style.setProperty(
      '--mobile-font-scale',
      scaleMap[mobileFontScale]
    );
    return () => {
      document.documentElement.style.removeProperty('--mobile-font-scale');
    };
  }, [isMobile, mobileFontScale]);

  const isWorkspacesActive = isWorkspacesDestination(currentDestination);
  const isTicketsActive = currentDestination?.kind === 'tickets';
  const isWorkspaceSidebarPreviewEnabled =
    !isMobile && isWorkspacesActive && !isLeftSidebarVisible;
  const sidebarPreview = useWorkspaceSidebarPreviewController({
    enabled: isWorkspaceSidebarPreviewEnabled,
    isAppBarHovered,
  });

  const handleWorkspacesClick = useCallback(() => {
    appNavigation.goToWorkspaces();
  }, [appNavigation]);

  const handleTicketsClick = useCallback(() => {
    appNavigation.goToTickets();
  }, [appNavigation]);

  return (
    <div
      className={cn(
        'bg-primary',
        isMobile
          ? 'flex fixed inset-0 pb-[env(safe-area-inset-bottom)]'
          : 'grid grid-cols-[auto_1fr] grid-rows-[auto_1fr] h-screen'
      )}
    >
      {!isMobile && (
        <>
          {/* Desktop corner spacer. */}
          <div
            data-tauri-drag-region
            className="bg-secondary"
            style={isTauriMac() ? { minWidth: 56 } : undefined}
          />
          {/* Desktop navbar. */}
          <NavbarContainer onOpenDrawer={() => setIsDrawerOpen(true)} />
          {/* Desktop AppBar sidebar. */}
          <AppBar
            projects={[]}
            onWorkspacesClick={handleWorkspacesClick}
            onTicketsClick={handleTicketsClick}
            isTicketsActive={isTicketsActive}
            onProjectClick={() => {}}
            onProjectsDragEnd={() => {}}
            isWorkspacesActive={isWorkspacesActive}
            activeProjectId={null}
            onHoverStart={() => setIsAppBarHovered(true)}
            onHoverEnd={() => setIsAppBarHovered(false)}
            appVersion={appVersion}
          />
          {/* Desktop content. */}
          <div className="relative min-h-0 overflow-hidden">
            {isWorkspaceSidebarPreviewEnabled && (
              <div className="absolute inset-y-0 left-0 z-20 flex items-center">
                <WorkspacesSidebarReopenTag
                  active={sidebarPreview.isPreviewOpen}
                  onHoverStart={sidebarPreview.handleHandleHoverStart}
                  onHoverEnd={sidebarPreview.handleHandleHoverEnd}
                  ariaLabel="Workspaces"
                />
              </div>
            )}

            {isWorkspaceSidebarPreviewEnabled && (
              <div
                className={cn(
                  'absolute left-0 top-0 z-30 h-full w-[300px] transition-transform duration-150 ease-out',
                  sidebarPreview.isPreviewOpen
                    ? 'translate-x-0 pointer-events-auto'
                    : '-translate-x-full pointer-events-none'
                )}
                onMouseEnter={sidebarPreview.handlePreviewHoverStart}
                onMouseLeave={sidebarPreview.handlePreviewHoverEnd}
              >
                <div className="h-full w-full overflow-hidden border-r border-border bg-secondary shadow-lg">
                  <WorkspacesSidebarContainer />
                </div>
              </div>
            )}

            <Outlet />
          </div>
        </>
      )}

      {isMobile && (
        <div className="flex flex-col flex-1 min-w-0 overflow-hidden">
          <NavbarContainer
            mobileMode={isMobile}
            onOpenDrawer={() => setIsDrawerOpen(true)}
          />
          <div className="flex-1 min-h-0 overflow-hidden">
            <Outlet />
          </div>
        </div>
      )}

      {/* Mobile project navigation drawer */}
      <MobileDrawer
        open={isDrawerOpen && isMobile}
        onClose={() => setIsDrawerOpen(false)}
      >
        <div className="flex flex-col h-full">
          <div className="flex items-center justify-between p-4 border-b border-border">
            <span className="text-sm font-medium text-high truncate">
              KanbAIn
            </span>
            <button
              type="button"
              onClick={() => setIsDrawerOpen(false)}
              className="p-1 rounded-sm text-low hover:text-normal cursor-pointer"
            >
              <XIcon className="h-4 w-4" weight="bold" />
            </button>
          </div>

          <button
            type="button"
            onClick={() => {
              handleWorkspacesClick();
              setIsDrawerOpen(false);
            }}
            className="flex items-center gap-2 px-4 py-3 text-sm text-normal hover:bg-secondary cursor-pointer"
          >
            <LayoutIcon className="h-4 w-4" />
            Workspaces
          </button>
          <button
            type="button"
            onClick={() => {
              handleTicketsClick();
              setIsDrawerOpen(false);
            }}
            className="flex items-center gap-2 px-4 py-3 text-sm text-normal hover:bg-secondary cursor-pointer"
          >
            <KanbanIcon className="h-4 w-4" />
            Tickets
          </button>
        </div>
      </MobileDrawer>
    </div>
  );
}
