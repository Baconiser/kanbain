import {
  GearIcon,
  GitBranchIcon,
  CpuIcon,
  PlugIcon,
} from '@phosphor-icons/react';
import type { Icon } from '@phosphor-icons/react';
import { GeneralSettingsSection } from './GeneralSettingsSection';
import { ReposSettingsSection } from './ReposSettingsSection';
import { AgentsSettingsSection } from './AgentsSettingsSection';
import { McpSettingsSection } from './McpSettingsSection';

export type SettingsSectionType = 'general' | 'repos' | 'agents' | 'mcp';

export type SettingsSectionInitialState = {
  general: undefined;
  repos: { repoId?: string } | undefined;
  agents: { executor?: string; variant?: string } | undefined;
  mcp: undefined;
};

export interface SettingsSectionDefinition {
  id: SettingsSectionType;
  icon: Icon;
}

export const SETTINGS_SECTION_DEFINITIONS: SettingsSectionDefinition[] = [
  { id: 'general', icon: GearIcon },
  { id: 'repos', icon: GitBranchIcon },
  { id: 'agents', icon: CpuIcon },
  { id: 'mcp', icon: PlugIcon },
];

export function renderSettingsSection(
  type: SettingsSectionType,
  initialState?: SettingsSectionInitialState[SettingsSectionType]
) {
  switch (type) {
    case 'general':
      return <GeneralSettingsSection />;
    case 'repos':
      return (
        <ReposSettingsSection
          initialState={initialState as SettingsSectionInitialState['repos']}
        />
      );
    case 'agents':
      return <AgentsSettingsSection />;
    case 'mcp':
      return <McpSettingsSection />;
    default:
      return <GeneralSettingsSection />;
  }
}
