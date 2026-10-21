import type { ConnectivityReportDto } from './ipc';

/** State of the "Test connection" action in the settings. */
export type TestState =
  | { readonly kind: 'idle' }
  | { readonly kind: 'testing' }
  | { readonly kind: 'success'; readonly message: string; readonly rtt: number | null }
  | { readonly kind: 'error'; readonly message: string };

/** Outcome of a finished connectivity report: success only when every stage passed. */
export function describeConnectivityReport(report: ConnectivityReportDto): TestState {
  if (!report.passed) {
    const failed = report.stages.find((stage) => stage.outcome === 'failed');
    const reason = failed
      ? `${failed.stage_label}: ${failed.error ?? failed.detail ?? 'failed'}`
      : 'the test did not complete';
    return { kind: 'error', message: `Connection to ${report.endpoint} failed at ${reason}` };
  }
  const rtt = report.rtt?.avg_ms ?? null;
  return {
    kind: 'success',
    rtt,
    message:
      rtt !== null
        ? `${report.endpoint} passed all stages. Round trip ${rtt.toFixed(1)} ms.`
        : `${report.endpoint} passed all stages.`,
  };
}

