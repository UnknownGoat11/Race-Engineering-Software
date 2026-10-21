import { describe, expect, it } from 'vitest';
import type { ConnectivityReportDto } from './ipc';
import { describeConnectivityReport } from './connectivity';

const base: ConnectivityReportDto = {
  endpoint: 'relay.example.com:443',
  passed: false,
  stages: [
    { stage: 'dns', stage_label: 'DNS', outcome: 'failed', detail: null, error: 'no such host', duration_ms: 12 },
    { stage: 'tcp', stage_label: 'TCP', outcome: 'skipped', detail: null, error: null, duration_ms: 0 },
  ],
  rtt: null,
} as unknown as ConnectivityReportDto;

describe('describeConnectivityReport', () => {
  it('reports a failed test as an error naming the first failed stage', () => {
    const state = describeConnectivityReport(base);
    expect(state.kind).toBe('error');
    expect(state.kind === 'error' && state.message).toContain('DNS: no such host');
  });

  it('reports success only when every stage passed', () => {
    const state = describeConnectivityReport({
      ...base,
      passed: true,
      rtt: { samples: 5, min_ms: 10, avg_ms: 12.34, max_ms: 15 } as ConnectivityReportDto['rtt'],
    });
    expect(state.kind).toBe('success');
    expect(state.kind === 'success' && state.message).toContain('12.3 ms');
  });
});
