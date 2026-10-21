import type { JSX } from 'react';
import { NO_HISTORY_REASON, ViewNoData } from './ViewNoData';

/**
 * Gap-to-leader chart over race laps. It needs each car's gap at every lap, which the app does not record yet,
 * so the view shows an explicit no-data state instead of example traces.
 */
export function FieldSpreadChartView(): JSX.Element {
  return (
    <div className="field-spread-view" data-testid="field-spread-view">
      <div className="spread-header">
        <div className="spread-title-group">
          <h2 className="spread-title">Multi-Car Race Field Spread (Gap to Leader vs Laps)</h2>
        </div>
      </div>
      <ViewNoData title="Gap history" reason={NO_HISTORY_REASON} testId="field-spread-no-data" />
    </div>
  );
}
