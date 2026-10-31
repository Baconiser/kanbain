import { useTranslation } from 'react-i18next';
import { WarningIcon } from '@phosphor-icons/react';
import { useSandboxStatus } from '@/shared/hooks/useSandboxStatus';
import { SettingsDialog } from '@/shared/dialogs/settings/SettingsDialog';

/**
 * Setup notice shown while the sandbox is on but can't run agents: Docker is
 * missing, or no model credential (API key, Claude account or custom
 * provider) is set up.
 */
export function SandboxBanner() {
  const { t } = useTranslation('common');
  const { data: status } = useSandboxStatus();

  if (!status?.enabled) return null;
  const hasCredential =
    status.anthropic_key_configured ||
    status.claude_account_connected ||
    status.openai_key_configured ||
    status.custom_provider_count > 0;
  const message = !status.docker_available
    ? t('sandboxBanner.noDocker')
    : !hasCredential
      ? t('sandboxBanner.noKey')
      : null;
  if (!message) return null;

  return (
    <div className="absolute top-base right-base z-40 max-w-sm rounded-sm border border-border bg-panel shadow-lg p-base text-sm flex gap-base">
      <WarningIcon className="size-icon-base text-error shrink-0" />
      <div className="space-y-half">
        <p className="text-normal">{message}</p>
        <button
          type="button"
          className="text-brand underline cursor-pointer"
          onClick={() => SettingsDialog.show({ initialSection: 'sandbox' })}
        >
          {t('sandboxBanner.openSettings')}
        </button>
      </div>
    </div>
  );
}
