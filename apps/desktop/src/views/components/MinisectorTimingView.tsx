import type { JSX } from 'react';
import { NO_FIELD_DATA_REASON, ViewNoData } from './ViewNoData';

/**
 * Field minisector board. Per-car minisector splits are not part of the live scoring feed (it carries only
 * sector 1 / sector 1+2 times), so this view shows an explicit no-data state instead of example splits.
 */
export function MinisectorTimingView(): JSX.Element {
  return (
    <div className="minisector-timing-view" data-testid="minisector-timing-view">
      <div className="minisector-header">
        <div className="minisector-title-group">
          <h2 className="minisector-title">Multi-Car Minisector Timing Board</h2>
        </div>
      </div>
      <ViewNoData title="Minisector splits" reason={NO_FIELD_DATA_REASON} testId="minisector-no-data" />
    </div>
  );
}
