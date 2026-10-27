import { useMemo, useCallback } from 'react';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { useActions } from '@/shared/hooks/useActions';
import {
  Navbar,
  type NavbarSectionItem,
  type MobileTabId,
} from '@vibe/ui/components/Navbar';
import { NavbarActionGroups } from '@/shared/actions';
import {
  NavbarDivider,
  type ActionDefinition,
  type NavbarItem as ActionNavbarItem,
  type ActionVisibilityContext,
  isSpecialIcon,
  getActionIcon,
  getActionTooltip,
  isActionActive,
  isActionEnabled,
  isActionVisible,
} from '@/shared/types/actions';
import { useActionVisibilityContext } from '@/shared/hooks/useActionVisibilityContext';
import { useMobileActiveTab } from '@/shared/stores/useUiPreferencesStore';
import { CommandBarDialog } from '@/shared/dialogs/command-bar/CommandBarDialog';
import { SettingsDialog } from '@/shared/dialogs/settings/SettingsDialog';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';

/**
 * Check if a NavbarItem is a divider
 */
function isDivider(item: ActionNavbarItem): item is typeof NavbarDivider {
  return 'type' in item && item.type === 'divider';
}

/**
 * Filter navbar items by visibility, keeping dividers but removing them
 * if they would appear at the start, end, or consecutively.
 */
function filterNavbarItems(
  items: readonly ActionNavbarItem[],
  ctx: ActionVisibilityContext
): ActionNavbarItem[] {
  // Filter actions by visibility, keep dividers
  const filtered = items.filter((item) => {
    if (isDivider(item)) return true;
    if (!isActionVisible(item, ctx)) return false;
    return !isSpecialIcon(getActionIcon(item, ctx));
  });

  // Remove leading/trailing dividers and consecutive dividers
  const result: ActionNavbarItem[] = [];
  for (const item of filtered) {
    if (isDivider(item)) {
      // Only add divider if we have items before it and last item wasn't a divider
      if (result.length > 0 && !isDivider(result[result.length - 1])) {
        result.push(item);
      }
    } else {
      result.push(item);
    }
  }

  // Remove trailing divider
  if (result.length > 0 && isDivider(result[result.length - 1])) {
    result.pop();
  }

  return result;
}

function toNavbarSectionItems(
  items: readonly ActionNavbarItem[],
  ctx: ActionVisibilityContext,
  onExecuteAction: (action: ActionDefinition) => void
): NavbarSectionItem[] {
  return items.reduce<NavbarSectionItem[]>((result, item) => {
    if (isDivider(item)) {
      result.push({ type: 'divider' });
      return result;
    }

    const icon = getActionIcon(item, ctx);
    if (isSpecialIcon(icon)) {
      return result;
    }

    result.push({
      type: 'action',
      id: item.id,
      icon,
      isActive: isActionActive(item, ctx),
      tooltip: getActionTooltip(item, ctx),
      shortcut: item.shortcut,
      disabled: !isActionEnabled(item, ctx),
      onClick: () => onExecuteAction(item),
    });
    return result;
  }, []);
}

export function NavbarContainer({
  mobileMode = false,
  onOpenDrawer,
}: {
  mobileMode?: boolean;
  onOpenDrawer?: () => void;
}) {
  const { executeAction } = useActions();
  const { workspace: selectedWorkspace, isCreateMode } = useWorkspaceContext();
  const appNavigation = useAppNavigation();
  const [mobileActiveTab, setMobileActiveTab] = useMobileActiveTab();

  // Get action visibility context (includes all state for visibility/active/enabled)
  const actionCtx = useActionVisibilityContext();

  // Action handler - all actions go through the standard executeAction
  const handleExecuteAction = useCallback(
    (action: ActionDefinition) => {
      if (action.requiresTarget && selectedWorkspace?.id) {
        executeAction(action, selectedWorkspace.id);
      } else {
        executeAction(action);
      }
    },
    [executeAction, selectedWorkspace?.id]
  );

  const leftItems = useMemo(
    () =>
      toNavbarSectionItems(
        filterNavbarItems(NavbarActionGroups.left, actionCtx),
        actionCtx,
        handleExecuteAction
      ),
    [actionCtx, handleExecuteAction]
  );

  const rightItems = useMemo(
    () =>
      toNavbarSectionItems(
        filterNavbarItems(NavbarActionGroups.right, actionCtx),
        actionCtx,
        handleExecuteAction
      ),
    [actionCtx, handleExecuteAction]
  );

  const navbarTitle = isCreateMode
    ? 'Create Workspace'
    : selectedWorkspace?.branch;

  // Mobile-specific callbacks
  const handleOpenCommandBar = useCallback(() => {
    CommandBarDialog.show();
  }, []);

  const handleOpenSettings = useCallback(() => {
    SettingsDialog.show();
  }, []);

  const handleNavigateBack = useCallback(() => {
    appNavigation.goToWorkspaces();
  }, [appNavigation]);

  return (
    <Navbar
      workspaceTitle={navbarTitle}
      leftItems={leftItems}
      rightItems={rightItems}
      mobileMode={mobileMode}
      onOpenCommandBar={handleOpenCommandBar}
      onOpenSettings={handleOpenSettings}
      onNavigateBack={handleNavigateBack}
      onOpenDrawer={onOpenDrawer}
      mobileActiveTab={mobileActiveTab as MobileTabId}
      onMobileTabChange={(tab) => setMobileActiveTab(tab)}
    />
  );
}
