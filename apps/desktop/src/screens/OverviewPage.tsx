import type { JSX } from 'react';
import type { Mode } from '../lib/ipc';
import { modeLabel } from '../lib/labels';

interface OverviewPageProps {
  readonly mode: Mode;
}

const MODE_DESCRIPTION: Record<Mode, string> = {
  driver: 'This PC reads Le Mans Ultimate telemetry, records it locally and streams it to the relay.',
  engineer: "This PC receives a driver's live telemetry from the relay for remote analysis.",
};

/** Gate 0 placeholder. Live panels are added in later gates; nothing here shows fabricated data. */
export function OverviewPage({ mode }: OverviewPageProps): JSX.Element {
  return (
    <section className="page">
      <h2 className="page-title">Overview</h2>
      <div className="panel">
        <div className="panel-header">
          <span>Status</span>
          <span>{modeLabel(mode)}</span>
        </div>
        <div className="panel-body">
          <p>{MODE_DESCRIPTION[mode]}</p>
          <p className="panel-note">
            No telemetry source is connected. Acquisition, recording and streaming are not part of this build yet.
          </p>
        </div>
      </div>
    </section>
  );
}
