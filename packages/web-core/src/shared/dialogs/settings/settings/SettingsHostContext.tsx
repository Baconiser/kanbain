import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useAppRuntime } from '@/shared/hooks/useAppRuntime';
import {
  createMachineClient,
  type MachineClient,
} from '@/shared/lib/machineClient';

export function useSettingsMachineClient(): MachineClient {
  const { t } = useTranslation('settings');
  const runtime = useAppRuntime();
  const label = t('settings.hostPicker.thisMachine', 'This machine');

  return useMemo(
    () =>
      createMachineClient(runtime, {
        kind: 'local',
        id: 'local',
        apiHostId: null,
        label,
      }),
    [runtime, label]
  );
}
