import { createContext, useContext, type ReactNode } from 'react';

// Module-level getter so the API transport can read the hostId outside React
const _hostId: string | null = null;
export function getCurrentHostId(): string | null {
  return _hostId;
}

const HostIdContext = createContext<string | null>(null);

export function useHostId(): string | null {
  return useContext(HostIdContext);
}

export function HostIdProvider({ children }: { children: ReactNode }) {
  return (
    <HostIdContext.Provider value={_hostId}>{children}</HostIdContext.Provider>
  );
}
