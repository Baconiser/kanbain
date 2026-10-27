import { RouterProvider } from '@tanstack/react-router';
import { HotkeysProvider } from 'react-hotkeys-hook';
import { UserSystemProvider } from '@web/app/providers/ConfigProvider';
import { ClickedElementsProvider } from '@web/app/providers/ClickedElementsProvider';
import { localAppNavigation } from '@web/app/navigation/AppNavigation';
import { AppNavigationProvider } from '@/shared/hooks/useAppNavigation';
import { useTauriNotificationNavigation } from '@web/app/hooks/useTauriNotificationNavigation';
import { router } from '@web/app/router';

function TauriListeners() {
  useTauriNotificationNavigation();
  return null;
}

function App() {
  return (
    <AppNavigationProvider value={localAppNavigation}>
      <TauriListeners />
      <UserSystemProvider>
        <ClickedElementsProvider>
          <HotkeysProvider
            initiallyActiveScopes={[
              'global',
              'workspace',
              'kanban',
              'projects',
            ]}
          >
            <RouterProvider router={router} />
          </HotkeysProvider>
        </ClickedElementsProvider>
      </UserSystemProvider>
    </AppNavigationProvider>
  );
}

export default App;
