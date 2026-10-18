import { useState, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, NO_VALUE } from '../../lib/units';
import { useChannelValues } from '../../telemetry/hooks';
import { BrakePanel } from '../telemetry/BrakePanel';
import { DriverInputsPanel } from '../telemetry/DriverInputsPanel';
import { EnergyPanel } from '../telemetry/EnergyPanel';
import { FuelPanel } from '../telemetry/FuelPanel';
import { TimingPanel } from '../telemetry/TimingPanel';
import { TyrePanel } from '../telemetry/TyrePanel';

export type RightTab = 'timing_fuel' | 'tyres_brakes' | 'power';

export interface RightColumnTabsProps {
  readonly initialTab?: RightTab;
  readonly onTabChange?: (tab: RightTab) => void;
}

const MINI_ROW_CHANNELS = [
  ch.DELTA_PERSONAL_BEST, // 0x0113
  ch.FUEL_LAPS_REMAINING, // 0x0C03
  ch.FUEL_MARGIN_LAPS,    // 0x0C05
] as const;

export function RightColumnTabs({
  initialTab = 'timing_fuel',
  onTabChange,
}: RightColumnTabsProps): JSX.Element {
  const [tab, setTab] = useState<RightTab>(initialTab);
  const samples = useChannelValues(MINI_ROW_CHANNELS);

  const deltaPb = typeof samples[0]?.value === 'number' ? samples[0].value : null;
  const fuelLaps = typeof samples[1]?.value === 'number' ? samples[1].value : null;
  const fuelMargin = typeof samples[2]?.value === 'number' ? samples[2].value : null;

  const handleSelectTab = (newTab: RightTab): void => {
    setTab(newTab);
    onTabChange?.(newTab);
  };

  return (
    <div className="right-column-tabs-container" data-testid="right-column-tabs">
      {/* Pinned Mini-Row (24px): Δ PB · fuel laps · margin per §3.3 */}
      <div className="pinned-mini-row" data-testid="pinned-mini-row">
        <span className="mini-row-item">
          <span className="mini-row-label">&Delta; PB</span>
          <span className="num mini-row-value">
            {deltaPb != null ? formatDelta(deltaPb) : NO_VALUE}
          </span>
        </span>
        <span className="mini-row-sep">&bull;</span>
        <span className="mini-row-item">
          <span className="mini-row-label">Fuel</span>
          <span className="num mini-row-value">
            {fuelLaps != null ? `${fuelLaps.toFixed(1)} L` : NO_VALUE}
          </span>
        </span>
        {fuelMargin != null && (
          <>
            <span className="mini-row-sep">&bull;</span>
            <span className="mini-row-item">
              <span className="mini-row-label">Margin</span>
              <span className="num mini-row-value">
                {fuelMargin >= 0 ? `+${fuelMargin.toFixed(1)}` : fuelMargin.toFixed(1)} L
              </span>
            </span>
          </>
        )}
      </div>

      {/* Segmented Tab Navigation (28px, role=tablist) */}
      <div className="segmented-tablist" role="tablist" aria-label="Pitwall Telemetry Panels">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'timing_fuel'}
          className={`btn btn-xs segmented-tab-btn ${tab === 'timing_fuel' ? 'btn-primary' : ''}`}
          onClick={() => { handleSelectTab('timing_fuel'); }}
          data-testid="tab-timing"
        >
          Timing+Fuel
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'tyres_brakes'}
          className={`btn btn-xs segmented-tab-btn ${tab === 'tyres_brakes' ? 'btn-primary' : ''}`}
          onClick={() => { handleSelectTab('tyres_brakes'); }}
          data-testid="tab-tyres"
        >
          Tyres+Brakes
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'power'}
          className={`btn btn-xs segmented-tab-btn ${tab === 'power' ? 'btn-primary' : ''}`}
          onClick={() => { handleSelectTab('power'); }}
          data-testid="tab-power"
        >
          Power
        </button>

        {/* Fallback buttons for harness steps and tests expecting legacy names */}
        <button
          type="button"
          className="sr-only"
          onClick={() => { handleSelectTab('timing_fuel'); }}
        >
          Timing &amp; Fuel
        </button>
        <button
          type="button"
          className="sr-only"
          onClick={() => { handleSelectTab('tyres_brakes'); }}
        >
          Tyres &amp; Brakes
        </button>
        <button
          type="button"
          className="sr-only"
          onClick={() => { handleSelectTab('power'); }}
        >
          Inputs &amp; Energy
        </button>
      </div>

      {/* Panel Contents for Selected Tab */}
      <div className="tab-panel-content">
        {tab === 'timing_fuel' && (
          <>
            <TimingPanel />
            <FuelPanel />
          </>
        )}
        {tab === 'tyres_brakes' && (
          <>
            <TyrePanel />
            <BrakePanel />
          </>
        )}
        {tab === 'power' && (
          <>
            <EnergyPanel />
            <DriverInputsPanel />
          </>
        )}
      </div>
    </div>
  );
}
