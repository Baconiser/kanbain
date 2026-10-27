import {
  localMachineClient,
  type MachineClient,
} from '@/shared/lib/machineClient';

export function useSettingsMachineClient(): MachineClient {
  return localMachineClient;
}
