import {
  GearIcon,
  GitBranchIcon,
  CpuIcon,
  PlugIcon,
  ShieldCheckIcon,
} from '@phosphor-icons/react';
import type { Icon } from '@phosphor-icons/react';
import { GeneralSettingsSection } from './GeneralSettingsSection';
import { ReposSettingsSection } from './ReposSettingsSection';
import { AgentsSettingsSection } from './AgentsSettingsSection';
import { McpSettingsSection } from './McpSettingsSection';
import { SandboxSettingsSection } from './SandboxSettingsSection';

export type SettingsSectionType =
  'general' | 'repos' | 'agents' | 'sandbox' | 'mcp';

export type SettingsSectionInitialState = {
  general: undefined;
  repos: { repoId?: string } | undefined;
  agents: undefined;
  sandbox: undefined;
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
  { id: 'sandbox', icon: ShieldCheckIcon },
  { id: 'mcp', icon: PlugIcon },
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
    case 'mcp':
      return <McpSettingsSection />;
    default:
      return <GeneralSettingsSection />;
  }
}
