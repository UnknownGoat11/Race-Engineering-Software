// Preview harness self-check: scenario catalogue integrity, origin handling and capture determinism of the fake
// backend (REDESIGN W0.1).
import { afterEach, describe, expect, it } from 'vitest';
import type { BridgeSubscribeResponse, CoreStatus } from '../lib/ipc';
import { FakeBackend, type ChannelLike } from './backend';
import { SCENARIOS, captureTimeS, previewTagText, scenarioFromQuery, type Scenario, type ScenarioGroup } from './scenarios';

const GROUPS: readonly ScenarioGroup[] = ['driver', 'engineer', 'mode', 'settings', 'app'];

/** The scenarios W0.1 adds (REDESIGN §4 W0.1); later waves rely on every one of them. */
const W0_SCENARIOS = [
  'driver-not-logged-in',
  'driver-live-lmu-origin',
  'driver-streaming-full',
  'driver-code-expiring',
  'driver-reconnecting-long',
  'driver-disk-low',
  'driver-dashboard-live',
  'engineer-not-logged-in',
  'engineer-live-lmu-origin',
  'engineer-kicked',
  'engineer-join-session-full',
  'engineer-join-expired-retry',
  'engineer-quali',
  'engineer-class-filter',
  'engineer-fcy',
  'engineer-gt3-ve',
  'engineer-first-lap-no-geometry',
  'engineer-geometry-built',
  'settings-ui-scale-125',
] as const;

function byId(id: string): Scenario {
  const s = SCENARIOS.find((x) => x.id === id);
  if (s === undefined) throw new Error(`scenario ${id} missing`);
  return s;
}

const backends: FakeBackend[] = [];
function startBackend(scenario: Scenario): FakeBackend {
  const backend = new FakeBackend();
  backends.push(backend);
  backend.start(scenario);
  return backend;
}

afterEach(() => {
  // freeze() stops the backend's interval timer.
  for (const b of backends.splice(0)) b.freeze(0);
});

describe('preview scenario catalogue', () => {
  it('ids are unique and URL-safe; group + state is unique', () => {
    const ids = SCENARIOS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(id).toMatch(/^[a-z0-9-]+$/);
    const keys = SCENARIOS.map((s) => `${s.group}/${s.state}`);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it('every group has at least one scenario', () => {
    for (const g of GROUPS) expect(SCENARIOS.filter((s) => s.group === g).length).toBeGreaterThanOrEqual(1);
  });

  it('contains every W0.1 scenario', () => {
    for (const id of W0_SCENARIOS) expect(() => byId(id)).not.toThrow();
    expect(SCENARIOS.length).toBeGreaterThanOrEqual(31 + W0_SCENARIOS.length);
  });

  it('selects by ?scenario and by ?screen&state', () => {
    expect(scenarioFromQuery('?scenario=engineer-fcy')?.id).toBe('engineer-fcy');
    expect(scenarioFromQuery('?screen=driver&state=disk-low')?.id).toBe('driver-disk-low');
    expect(scenarioFromQuery('?scenario=nope')).toBeNull();
    expect(scenarioFromQuery('')).toBeNull();
  });

  it('the corner tag always says PREVIEW; LMU-origin scenarios say NOT REAL DATA', () => {
    for (const s of SCENARIOS) {
      const text = previewTagText(s);
      expect(text).toContain('PREVIEW');
      expect(text).toContain(s.id);
      if (s.origin === 'lmu') expect(text).toContain('NOT REAL DATA');
      else expect(text).toContain('SYNTHETIC');
    }
    expect(SCENARIOS.filter((s) => s.origin === 'lmu').map((s) => s.id).sort()).toEqual([
      'driver-live-lmu-origin',
      'engineer-live-lmu-origin',
    ]);
  });

  it('the capture time is later than the settle time plus the steps', () => {
    for (const s of SCENARIOS) {
      expect(captureTimeS(s)).toBeGreaterThan((s.settleMs ?? 800) / 1000 + 0.15 + (s.steps?.length ?? 0) * 0.3);
    }
  });
});

describe('preview fake backend', () => {
  it('reports origin synthetic by default and lmu for an lmu-origin scenario (status and subscription)', async () => {
    const synthetic = startBackend(byId('engineer-live'));
    const lmu = startBackend(byId('engineer-live-lmu-origin'));
    const channel: ChannelLike = { onmessage: () => undefined };
    expect(((await synthetic.invoke('get_core_status', {})) as CoreStatus).origin).toBe('synthetic');
    expect(((await lmu.invoke('get_core_status', {})) as CoreStatus).origin).toBe('lmu');
    const sub = (await lmu.invoke('bridge_subscribe', { on_batch: channel })) as BridgeSubscribeResponse;
    expect(sub.origin).toBe('lmu');
    const sub2 = (await synthetic.invoke('bridge_subscribe', { on_batch: channel })) as BridgeSubscribeResponse;
    expect(sub2.origin).toBe('synthetic');
  });

  it('every scenario builds a status with the scenario origin', async () => {
    for (const s of SCENARIOS) {
      if (s.backendDown === true) continue;
      const b = startBackend(s);
      const status = (await b.invoke('get_core_status', {})) as CoreStatus;
      expect(status.origin).toBe(s.origin ?? 'synthetic');
    }
  });

  it('a join that fails only on the first attempt then returns the identity card', async () => {
    const b = startBackend(byId('engineer-join-expired-retry'));
    await expect(b.invoke('engineer_join', { code: 'H3WZ9P' })).rejects.toMatchObject({ code: 'code_expired' });
    await expect(b.invoke('engineer_join', { code: 'K7Q2XM' })).resolves.toMatchObject({ driver_username: 'sam_driver' });
  });

  it('session full is refused every time', async () => {
    const b = startBackend(byId('engineer-join-session-full'));
    for (let i = 0; i < 3; i++) {
      await expect(b.invoke('engineer_join', { code: 'P4HN7W' })).rejects.toMatchObject({ code: 'session_full' });
    }
  });

  it('freezing at the capture time gives identical data on every run', async () => {
    const run = async (): Promise<{ bytes: number[]; status: CoreStatus; events: number }> => {
      const scenario = byId('engineer-live');
      const b = startBackend(scenario);
      const chunks: Uint8Array[] = [];
      let events = 0;
      const last: { status: CoreStatus | null } = { status: null };
      b.listen('core:status', (p) => {
        last.status = p as CoreStatus;
      });
      b.listen('core:scoring', () => {
        events++;
      });
      await b.invoke('bridge_subscribe', { on_batch: { onmessage: (m: ArrayBuffer) => chunks.push(new Uint8Array(m)) } });
      b.freeze(captureTimeS(scenario));
      if (last.status === null) throw new Error('no final status');
      const all = chunks.flatMap((c) => Array.from(c));
      return { bytes: all, status: last.status, events };
    };
    const a = await run();
    const b = await run();
    expect(a.bytes.length).toBeGreaterThan(0);
    expect(a.bytes).toEqual(b.bytes);
    expect(a.status).toEqual(b.status);
    expect(a.events).toBe(b.events);
  }, 15000);
});
