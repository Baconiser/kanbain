import {
  GearIcon,
  ListMagnifyingGlassIcon,
  MagnifyingGlassIcon,
  GitBranchIcon,
  CpuIcon,
  PlugIcon,
  ShieldCheckIcon,
} from '@phosphor-icons/react';
import type { Icon } from '@phosphor-icons/react';
import { GeneralSettingsSection } from './GeneralSettingsSection';
import { ReposSettingsSection } from './ReposSettingsSection';
import { AgentsSettingsSection } from './AgentsSettingsSection';
import { LogsSettingsSection } from './LogsSettingsSection';
import { McpSettingsSection } from './McpSettingsSection';
import { SandboxSettingsSection } from './SandboxSettingsSection';
import { SearchSettingsSection } from './SearchSettingsSection';

export type SettingsSectionType =
  'general' | 'repos' | 'agents' | 'sandbox' | 'search' | 'mcp' | 'logs';

export type SettingsSectionInitialState = {
  general: undefined;
  repos: { repoId?: string } | undefined;
  agents: undefined;
  sandbox: undefined;
  search: undefined;
  mcp: undefined;
  logs: undefined;
};

export interface SettingsSectionDefinition {
  id: SettingsSectionType;
  icon: Icon;
}

export const SETTINGS_SECTION_DEFINITIONS: SettingsSectionDefinition[] = [
  { id: 'general', icon: GearIcon },
  { id: 'repos', icon: GitBranchIcon },
  { id: 'agents', icon: CpuIcon },
  { id: 'sandbox', icon: ShieldCheckIcon },
  { id: 'search', icon: MagnifyingGlassIcon },
  { id: 'mcp', icon: PlugIcon },
  { id: 'logs', icon: ListMagnifyingGlassIcon },
];

export function renderSettingsSection(
  type: SettingsSectionType,
  initialState?: SettingsSectionInitialState[SettingsSectionType],
  onClose?: () => void
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
      return <AgentsSettingsSection onClose={onClose} />;
    case 'sandbox':
      return <SandboxSettingsSection />;
    case 'search':
      return <SearchSettingsSection />;
    case 'mcp':
      return <McpSettingsSection />;
    case 'logs':
      return <LogsSettingsSection />;
    default:
      return <GeneralSettingsSection />;
  }
}
