import { memo, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FileTreeContainer } from './FileTreeContainer';
import { ProcessListContainer } from './ProcessListContainer';
import { PreviewControlsContainer } from './PreviewControlsContainer';
import { GitPanelContainer } from './GitPanelContainer';
import { TerminalPanelContainer } from '@/shared/components/TerminalPanelContainer';
import { WorkspaceNotesContainer } from './WorkspaceNotesContainer';
import { WorkspaceTicketPanel } from './WorkspaceTicketPanel';
import { useDiffs } from '@/shared/stores/useWorkspaceDiffStore';
import { ArrowsOutSimpleIcon } from '@phosphor-icons/react';
import { Button } from '@kanbain/ui/components/Button';
import { useLogsPanel } from '@/shared/hooks/useLogsPanel';
import type { RepoWithTargetBranch, Workspace } from 'shared/types';
import {
  PERSIST_KEYS,
  PersistKey,
  RIGHT_MAIN_PANEL_MODES,
  type RightMainPanelMode,
  usePersistedExpanded,
  useUiPreferencesStore,
} from '@/shared/stores/useUiPreferencesStore';
import {
  CollapsibleSectionHeader,
  type SectionAction,
} from '@kanbain/ui/components/CollapsibleSectionHeader';

type SidebarTab = 'ticket' | 'git' | 'terminal';
const TAB_STORAGE_KEY = 'kanbain.workspaceSidebar.tab';

function readStoredTab(): SidebarTab {
  try {
    const stored = window.localStorage.getItem(TAB_STORAGE_KEY);
    if (stored === 'ticket' || stored === 'git' || stored === 'terminal') {
      return stored;
    }
  } catch {
    // No storage, no memory. Ticket first, that's what you came for.
  }
  return 'ticket';
}

function SidebarTabButton({
  active,
  onClick,
  label,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      data-testid={`workspace-sidebar-tab-${label.toLowerCase()}`}
      onClick={onClick}
      className={`border-b-2 px-base py-base text-sm ${
        active
          ? 'border-brand text-high'
          : 'border-transparent text-low hover:text-normal'
      }`}
    >
      {label}
    </button>
  );
}

type SectionDef = {
  title: string;
  persistKey: PersistKey;
  visible: boolean;
  expanded: boolean;
  content: React.ReactNode;
  actions: SectionAction[];
};

export interface RightSidebarProps {
  rightMainPanelMode: RightMainPanelMode | null;
  selectedWorkspace: Workspace | undefined;
  repos: RepoWithTargetBranch[];
}

export const RightSidebar = memo(function RightSidebar({
  rightMainPanelMode,
  selectedWorkspace,
  repos,
}: RightSidebarProps) {
  const { t } = useTranslation(['tasks', 'common']);
  const diffs = useDiffs();
  const isTerminalVisible = useUiPreferencesStore((s) => s.isTerminalVisible);
  const { expandTerminal, isTerminalExpanded } = useLogsPanel();

  const [changesExpanded] = usePersistedExpanded(
    PERSIST_KEYS.changesSection,
    true
  );
  const [processesExpanded] = usePersistedExpanded(
    PERSIST_KEYS.processesSection,
    true
  );
  const [devServerExpanded] = usePersistedExpanded(
    PERSIST_KEYS.devServerSection,
    true
  );
  const [gitExpanded] = usePersistedExpanded(
    PERSIST_KEYS.gitPanelRepositories,
    true
  );
  const [storedTab, setStoredTab] = useState<SidebarTab>(readStoredTab);
  const issueId = selectedWorkspace?.issue_id ?? null;
  const hasTerminal = isTerminalVisible;
  // A tab that can't exist here (no linked ticket, terminal switched off)
  // falls back to Git without forgetting the choice for the next workspace.
  const tab: SidebarTab =
    (storedTab === 'ticket' && !issueId) ||
    (storedTab === 'terminal' && !hasTerminal)
      ? 'git'
      : storedTab;

  const setTab = (next: SidebarTab) => {
    setStoredTab(next);
    try {
      window.localStorage.setItem(TAB_STORAGE_KEY, next);
    } catch {
      // Fine, we'll forget.
    }
  };

  // Mounting the terminal spawns a shell, so it waits for the first visit
  // and then stays mounted to keep its scrollback.
  const [terminalMounted, setTerminalMounted] = useState(tab === 'terminal');
  useEffect(() => {
    if (tab === 'terminal') setTerminalMounted(true);
  }, [tab]);
  const [notesExpanded] = usePersistedExpanded(
    PERSIST_KEYS.notesSection,
    false
  );

  const hasUpperContent =
    rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.CHANGES ||
    rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.LOGS ||
    rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.PREVIEW;

  const upperExpanded = (() => {
    if (rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.CHANGES)
      return changesExpanded;
    if (rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.LOGS)
      return processesExpanded;
    if (rightMainPanelMode === RIGHT_MAIN_PANEL_MODES.PREVIEW)
      return devServerExpanded;
    return false;
  })();

  const sections: SectionDef[] = useMemo(() => {
    const result: SectionDef[] = [
      {
        title: 'Git',
        persistKey: PERSIST_KEYS.gitPanelRepositories,
        visible: true,
        expanded: gitExpanded,
        content: (
          <GitPanelContainer
            selectedWorkspace={selectedWorkspace}
            repos={repos}
          />
        ),
        actions: [],
      },
      {
        title: t('common:sections.notes'),
        persistKey: PERSIST_KEYS.notesSection,
        visible: true,
        expanded: notesExpanded,
        content: <WorkspaceNotesContainer />,
        actions: [],
      },
    ];

    switch (rightMainPanelMode) {
      case RIGHT_MAIN_PANEL_MODES.CHANGES:
        if (selectedWorkspace) {
          result.unshift({
            title: 'Changes',
            persistKey: PERSIST_KEYS.changesSection,
            visible: hasUpperContent,
            expanded: upperExpanded,
            content: (
              <FileTreeContainer
                key={selectedWorkspace.id}
                workspaceId={selectedWorkspace.id}
                diffs={diffs}
                className=""
              />
            ),
            actions: [],
          });
        }
        break;
      case RIGHT_MAIN_PANEL_MODES.LOGS:
        result.unshift({
          title: 'Logs',
          persistKey: PERSIST_KEYS.rightPanelprocesses,
          visible: hasUpperContent,
          expanded: upperExpanded,
          content: <ProcessListContainer />,
          actions: [],
        });
        break;
      case RIGHT_MAIN_PANEL_MODES.PREVIEW:
        if (selectedWorkspace) {
          result.unshift({
            title: 'Preview',
            persistKey: PERSIST_KEYS.rightPanelPreview,
            visible: hasUpperContent,
            expanded: upperExpanded,
            content: (
              <PreviewControlsContainer
                workspaceId={selectedWorkspace.id}
                className=""
              />
            ),
            actions: [],
          });
        }
        break;
      case null:
        break;
    }

    return result;
  }, [
    rightMainPanelMode,
    selectedWorkspace,
    repos,
    diffs,
    gitExpanded,
    notesExpanded,
    changesExpanded,
    processesExpanded,
    devServerExpanded,
    hasUpperContent,
    upperExpanded,
    t,
  ]);

  return (
    <div className="flex h-full flex-col border-l bg-secondary">
      <nav role="tablist" className="flex shrink-0 border-b bg-primary px-base">
        {issueId && (
          <SidebarTabButton
            label="Ticket"
            active={tab === 'ticket'}
            onClick={() => setTab('ticket')}
          />
        )}
        <SidebarTabButton
          label="Git"
          active={tab === 'git'}
          onClick={() => setTab('git')}
        />
        {hasTerminal && (
          <SidebarTabButton
            label="Terminal"
            active={tab === 'terminal'}
            onClick={() => setTab('terminal')}
          />
        )}
        {tab === 'terminal' && !isTerminalExpanded && (
          <Button
            size="xs"
            variant="ghost"
            className="ml-auto self-center"
            aria-label="Expand terminal"
            title="Expand terminal"
            onClick={expandTerminal}
          >
            <ArrowsOutSimpleIcon className="size-icon-sm" />
          </Button>
        )}
      </nav>

      {/* Inactive tabs stay mounted (hidden): unsaved ticket edits, plan
          drafts and the live terminal all survive a tab switch. */}
      {issueId && selectedWorkspace && (
        <div
          role="tabpanel"
          className={tab === 'ticket' ? 'min-h-0 flex-1' : 'hidden'}
        >
          <WorkspaceTicketPanel
            key={issueId}
            issueId={issueId}
            workspaceId={selectedWorkspace.id}
          />
        </div>
      )}

      {hasTerminal && terminalMounted && (
        <div
          role="tabpanel"
          className={
            tab === 'terminal' ? 'flex min-h-0 flex-1 flex-col' : 'hidden'
          }
        >
          {isTerminalExpanded ? (
            <p className="p-double text-sm text-low">
              The terminal is expanded in the main panel.
            </p>
          ) : (
            <div className="flex min-h-0 flex-1">
              <TerminalPanelContainer />
            </div>
          )}
        </div>
      )}

      <div
        role="tabpanel"
        className={tab === 'git' ? 'min-h-0 flex-1 overflow-y-auto' : 'hidden'}
      >
        <div className="divide-y border-b">
          {sections
            .filter((section) => section.visible)
            .map((section) => (
              <div
                key={section.persistKey}
                className="max-h-[max(50vh,400px)] flex flex-col overflow-hidden"
              >
                <CollapsibleSectionHeader
                  title={section.title}
                  persistKey={section.persistKey}
                  defaultExpanded={section.expanded}
                  actions={section.actions}
                >
                  <div className="flex flex-1 border-t min-h-[200px] w-full overflow-auto">
                    {section.content}
                  </div>
                </CollapsibleSectionHeader>
              </div>
            ))}
        </div>
      </div>
    </div>
  );
});
