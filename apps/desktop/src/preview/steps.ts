/**
 * PREVIEW HARNESS ONLY. Runs a scenario's UI steps against the real DOM (clicks and typing the way a user
 * would, through React's own event handling), then waits for the screen to settle.
 */
import type { Step } from './scenarios';

const FIND_TIMEOUT_MS = 8000;
/** How long an optional step looks for its target before it is skipped. */
const OPTIONAL_TIMEOUT_MS = 600;
const POLL_MS = 50;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor<T>(what: string, find: () => T | null, timeoutMs = FIND_TIMEOUT_MS): Promise<T> {
  const deadline = performance.now() + timeoutMs;
  for (;;) {
    const found = find();
    if (found !== null) return found;
    if (performance.now() > deadline) throw new Error(`preview step: ${what} not found`);
    await sleep(POLL_MS);
  }
}

function enabled(el: Element): boolean {
  return !(el instanceof HTMLButtonElement && el.disabled);
}

function byText(text: string): HTMLElement | null {
  const candidates = document.querySelectorAll<HTMLElement>('button, a, [role="tab"]');
  for (const el of candidates) {
    if (el.textContent.trim() === text && enabled(el)) return el;
  }
  return null;
}

function setInputValue(input: HTMLInputElement, value: string): void {
  // React tracks the value property; set it through the native setter so onChange fires.
  // eslint-disable-next-line @typescript-eslint/unbound-method -- the setter is called with `input` as `this`
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => { resolve(); }));
}

export async function runSteps(steps: readonly Step[], settleMs: number): Promise<void> {
  // Let the app finish its initial load (get_app_info / get_config) before the first step.
  await sleep(150);
  for (const step of steps) {
    switch (step.kind) {
      case 'click': {
        const el = await waitFor(step.selector, () => {
          const found = document.querySelector<HTMLElement>(step.selector);
          return found !== null && enabled(found) ? found : null;
        });
        el.click();
        break;
      }
      case 'clickText': {
        if (step.optional === true) {
          const el = await waitFor(`button "${step.text}"`, () => byText(step.text), OPTIONAL_TIMEOUT_MS).catch(
            () => null,
          );
          el?.click();
          break;
        }
        const el = await waitFor(`button "${step.text}"`, () => byText(step.text));
        el.click();
        break;
      }
      case 'type': {
        const el = await waitFor(step.selector, () => document.querySelector<HTMLInputElement>(step.selector));
        el.focus();
        setInputValue(el, step.text);
        break;
      }
      case 'wait':
        await sleep(step.ms);
        break;
    }
    await nextFrame();
  }
  await sleep(settleMs);
  await nextFrame();
  await nextFrame();
}
