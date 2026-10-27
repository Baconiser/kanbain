import { ReactNode } from 'react';
import { UserSystemContext } from '@/shared/hooks/useUserSystem';
import { useUserSystemController } from '@/shared/hooks/useUserSystemController';
import { localMachineClient } from '@/shared/lib/machineClient';

const QUERY_KEY = ['user-system', 'local'] as const;

export function SettingsMachineUserSystemProvider({
  children,
}: {
  children: ReactNode;
}) {
  const { value } = useUserSystemController({
    queryKey: QUERY_KEY,
    load: localMachineClient.getConfig,
    save: localMachineClient.saveConfig,
  });

  return (
    <UserSystemContext.Provider value={value}>
      {children}
    </UserSystemContext.Provider>
  );
}
