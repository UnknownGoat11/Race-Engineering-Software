import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { AppErrorBoundary } from './components/ErrorBoundary';
import { logUiError } from './lib/ipc';
import './styles/fonts.css';
import './styles/tokens.css';
import './styles/app.css';
import './styles/views.css';

// Forward unhandled window errors and promise rejections to the native log sink (DEFERRED 15)
window.addEventListener('error', (event: ErrorEvent) => {
  logUiError({
    message: event.message,
    stack: event.error instanceof Error ? (event.error.stack ?? null) : null,
    component: null,
  }).catch(() => undefined);
});

window.addEventListener('unhandledrejection', (event: PromiseRejectionEvent) => {
  const reason: unknown = event.reason;
  logUiError({
    message: reason instanceof Error ? reason.message : String(reason),
    stack: reason instanceof Error ? (reason.stack ?? null) : null,
    component: null,
  }).catch(() => undefined);
});

const container = document.getElementById('root');
if (container === null) {
  throw new Error('Root element #root is missing from index.html');
}

createRoot(container).render(
  <StrictMode>
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>
  </StrictMode>,
);
