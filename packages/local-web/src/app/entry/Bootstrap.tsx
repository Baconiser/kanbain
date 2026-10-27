import React from 'react';
import ReactDOM from 'react-dom/client';
import { ClickToComponent } from 'click-to-react-component';
import { QueryClientProvider } from '@tanstack/react-query';
import App from '@web/app/entry/App';
import { CrashScreen } from '@kanbain/ui/components/CrashScreen';
import '@/i18n';
import '@/shared/types/modals';
import { queryClient } from '@/shared/lib/queryClient';
import { isTauriApp } from '@/shared/lib/platform';
import { initZoom, zoomIn, zoomOut, zoomReset } from '@/shared/lib/zoom';

// In the Tauri desktop app, implement custom zoom (Cmd/Ctrl + =/–/0) via root
// font-size scaling and block trackpad/touchpad pinch-to-zoom.
if (isTauriApp()) {
  initZoom();

  document.addEventListener('keydown', (e) => {
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;

    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      zoomIn();
    } else if (e.key === '-') {
      e.preventDefault();
      zoomOut();
    } else if (e.key === '0') {
      e.preventDefault();
      zoomReset();
    }
  });

  document.addEventListener(
    'wheel',
    (e) => {
      if (e.ctrlKey) e.preventDefault();
    },
    { passive: false }
  );
  document.addEventListener('gesturestart', (e) => e.preventDefault());
  document.addEventListener('gesturechange', (e) => e.preventDefault());
}

class CrashBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null; componentStack: string | null }
> {
  state = {
    error: null as Error | null,
    componentStack: null as string | null,
  };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error(error);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  render() {
    if (this.state.error) {
      return (
        <CrashScreen
          error={this.state.error}
          componentStack={this.state.componentStack}
        />
      );
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <CrashBoundary>
        <ClickToComponent />
        <App />
      </CrashBoundary>
    </QueryClientProvider>
  </React.StrictMode>
);
