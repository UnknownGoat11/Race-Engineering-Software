import type { JSX } from 'react';
import { ViewNoData } from './ViewNoData';

/**
 * Driver-trait profile (trail-braking, throttle aggression, consistency). These scores need an analysis of
 * recorded laps that the app does not perform yet, so the view shows an explicit no-data state instead of
 * example scores.
 */
export function DriverDnaView(): JSX.Element {
  return (
    <div className="driver-dna-view" data-testid="driver-dna-view">
      <div className="dna-hero-card">
        <div className="dna-hero-left">
          <h2 className="dna-title">Driver DNA & Performance Signature</h2>
        </div>
      </div>
      <ViewNoData
        title="Driver trait scores"
        reason="Trait scores need an analysis of recorded laps, which is not implemented yet. No example scores are shown."
        testId="driver-dna-no-data"
      />
    </div>
  );
}
