import { Outlet, createRootRoute } from '@tanstack/react-router';
import { I18nextProvider } from 'react-i18next';
import { ThemeMode } from 'shared/types';
import i18n from '@/i18n';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { ThemeProvider } from '@web/app/providers/ThemeProvider';
import { useUiPreferencesScratch } from '@/shared/hooks/useUiPreferencesScratch';
import '@/app/styles/new/index.css';

function RootRouteComponent() {
  const { config } = useUserSystem();

  useUiPreferencesScratch();

  return (
    <I18nextProvider i18n={i18n}>
      <ThemeProvider initialTheme={config?.theme || ThemeMode.SYSTEM}>
        <Outlet />
      </ThemeProvider>
    </I18nextProvider>
  );
}

export const Route = createRootRoute({
  component: RootRouteComponent,
});
