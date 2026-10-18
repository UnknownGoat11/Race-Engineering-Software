/**
 * PREVIEW HARNESS ONLY: entry of preview/index.html (`pnpm preview:ui`, port 1431). Never part of `pnpm build`.
 *
 * Without a scenario query it renders the scenario index. With one (`?scenario=<id>` or
 * `?screen=<group>&state=<state>`) it starts the fake backend for that scenario and mounts the real `<App />`
 * exactly as src/main.tsx does; the Tauri API modules are replaced by the shims in ./tauri (vite alias), so every
 * screen runs its production code against SYNTHETIC data. Optional scenario steps (clicks, typing) then bring the
 * screen into the wanted state; the fake backend then jumps to the scenario's planned capture time, emits its final
 * state and stops, the page clocks are held (./clock.ts) and the page reports `data-preview-ready="true"` on
 * <html> for the screenshot script (preview/capture.mjs). Same scenario, same pixels.
 *
 * Owner rule: preview data is never presented as real data. Every scenario page carries a fixed PREVIEW tag in
 * the bottom-right corner; an `origin: 'lmu'` scenario's tag says "SIMULATED LMU ORIGIN · NOT REAL DATA".
 */
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../App';
import { AppErrorBoundary } from '../components/ErrorBoundary';
import { logUiError } from '../lib/ipc';
import '../styles/fonts.css';
import '../styles/tokens.css';
import '../styles/app.css';
import { fakeBackend } from './backend';
import { holdPreviewClock, installPreviewClock } from './clock';
import { ScenarioIndex } from './ScenarioIndex';
import { captureTimeS, previewTagText, scenarioFromQuery, type Scenario } from './scenarios';
import { runSteps } from './steps';
import './preview.css';

const container = document.getElementById('root');
if (container === null) throw new Error('Root element #root is missing from preview/index.html');

installPreviewClock();

function markReady(error: string | null): void {
  const root = document.documentElement;
  if (error !== null) root.dataset['previewError'] = error;
  root.dataset['previewReady'] = 'true';
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => { resolve(); }));
}

/** Freezes the backend at the planned capture time, holds the clocks and lets the app render the final state. */
async function freezeForCapture(scenario: Scenario): Promise<void> {
  const atS = captureTimeS(scenario);
  const late = fakeBackend.freeze(atS);
  holdPreviewClock(atS);
  if (late) document.documentElement.dataset['previewFreezeLate'] = 'true';
  // React commits the final events, then the graph and map draw on their next frames.
  for (let i = 0; i < 4; i++) await nextFrame();
  await new Promise((resolve) => setTimeout(resolve, 400));
  await nextFrame();
}

function mountScenario(scenario: Scenario, target: HTMLElement): void {
  document.title = `Preview: ${scenario.title} (${scenario.origin === 'lmu' ? 'SIMULATED LMU ORIGIN' : 'SYNTHETIC'})`;
  // Same forwarding as src/main.tsx (the fake backend logs it to the console).
  window.addEventListener('error', (event: ErrorEvent) => {
    logUiError({
      message: event.message,
      stack: event.error instanceof Error ? (event.error.stack ?? null) : null,
      component: null,
    }).catch(() => undefined);
  });
  fakeBackend.start(scenario);
  createRoot(target).render(
    <StrictMode>
      <AppErrorBoundary>
        <App />
      </AppErrorBoundary>
    </StrictMode>,
  );
  const tag = document.createElement('div');
  tag.className = scenario.origin === 'lmu' ? 'preview-tag preview-tag-lmu' : 'preview-tag';
  tag.setAttribute('aria-hidden', 'true');
  tag.dataset['previewTag'] = scenario.origin ?? 'synthetic';
  tag.textContent = previewTagText(scenario);
  document.body.appendChild(tag);

  runSteps(scenario.steps ?? [], scenario.settleMs ?? 800)
    .then(() => freezeForCapture(scenario))
    .then(
    () => {
      markReady(null);
    },
    (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      console.error('[preview] scenario steps failed:', message);
      markReady(message);
    },
  );
}

const search = window.location.search;
const scenario = scenarioFromQuery(search);
const hasQuery = new URLSearchParams(search).has('scenario') || new URLSearchParams(search).has('screen');

if (scenario === null) {
  document.title = 'UI preview scenarios (SYNTHETIC)';
  createRoot(container).render(
    <StrictMode>
      <ScenarioIndex unknownQuery={hasQuery ? search : null} />
    </StrictMode>,
  );
  markReady(null);
} else {
  mountScenario(scenario, container);
}
