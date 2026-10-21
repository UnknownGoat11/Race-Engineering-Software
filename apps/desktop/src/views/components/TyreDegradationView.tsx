import type { JSX } from 'react';
import { TyreDegQuickSummary } from './TyreDegQuickSummary';
import { ViewNoData } from './ViewNoData';

/**
 * Tyre degradation view. Current wear per wheel is live (TyreDegQuickSummary). Wear curves, the cliff
 * predictor, double-stint assessment and the lock-up / flatspot log all need lap-by-lap history or lock-up
 * detection the app does not provide yet, so each shows an explicit no-data state instead of example data.
 */
export function TyreDegradationView(): JSX.Element {
  return (
    <div className="view-card tyre-degradation-view" data-testid="tyre-degradation-view">
      <div className="view-card-header">
        <div className="view-card-title-group">
          <span className="view-card-glyph">🛞</span>
          <h2 className="view-card-title">Tyre Degradation & Lock-Up Logger</h2>
        </div>
      </div>

      <div className="tyre-deg-quick-banner">
        <TyreDegQuickSummary />
      </div>

      <div className="tyre-deg-content-grid">
        <div className="tyre-deg-chart-card">
          <div className="chart-card-header">
            <span className="chart-card-title">TYRE WEAR VS STINT LAPS</span>
          </div>
          <ViewNoData
            title="Wear curves"
            reason="Wear curves need per-lap wear history, which the app does not record yet."
            testId="deg-curves-no-data"
          />

          <div className="double-stint-card">
            <div className="double-stint-header">
              <span className="double-stint-title">DOUBLE-STINT ASSESSMENT</span>
            </div>
            <ViewNoData
              title="Double-stint assessment"
              reason="Needs a measured degradation rate over at least one stint; not available yet."
              testId="deg-double-stint-no-data"
            />
          </div>
        </div>

        <div className="tyre-deg-log-card">
          <div className="log-card-header">
            <div className="log-title-group">
              <span className="log-card-title">LOCK-UP & FLATSPOT EVENT LOG</span>
            </div>
          </div>
          <ViewNoData
            title="Lock-up log"
            reason="Lock-up and flatspot detection is not implemented; no events are recorded."
            testId="deg-lockup-no-data"
          />
        </div>
      </div>
    </div>
  );
}
