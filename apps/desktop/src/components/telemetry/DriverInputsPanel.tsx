import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatSpeed, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { useScopeDq } from './presentation';
import './telemetry.css';

const INPUT_CHANNELS = [
  ch.SPEED,
  ch.RPM,
  ch.RPM_MAX,
  ch.GEAR,
  ch.THROTTLE,
  ch.BRAKE,
  ch.CLUTCH,
  ch.STEERING_ANGLE,
] as const;

function formatGear(gear: number | null | undefined): string {
  if (gear == null || isNaN(gear)) return NO_VALUE;
  const g = Math.round(gear);
  if (g < 0) return 'R';
  if (g === 0) return 'N';
  return String(g);
}

export function DriverInputsPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="INPUTS" testId="panel-driver-inputs">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <DriverInputsPanelContent />;
}

function DriverInputsPanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(INPUT_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < INPUT_CHANNELS.length; i++) {
      const id = INPUT_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const speed = num(ch.SPEED);
  const rpm = num(ch.RPM);
  const maxRpm = num(ch.RPM_MAX);
  const gear = num(ch.GEAR);
  const throttle = num(ch.THROTTLE);
  const brake = num(ch.BRAKE);
  const clutch = num(ch.CLUTCH);
  const steeringRad = num(ch.STEERING_ANGLE);

  const steeringDeg = steeringRad != null ? Math.round((steeringRad * 180) / Math.PI) : null;
  const hasRpmMax = maxRpm != null && maxRpm > 0;
  const rpmPct = hasRpmMax && rpm != null ? (rpm / maxRpm) * 100 : null;
  const throttlePct = throttle != null ? Math.min(100, Math.max(0, throttle * 100)) : 0;
  const brakePct = brake != null ? Math.min(100, Math.max(0, brake * 100)) : 0;

  const gearClass = useMemo(() => {
    if (gear == null || isNaN(gear) || Math.round(gear) === 0) {
      return 'inputs-gear-val inputs-gear-val--neutral';
    }
    if (Math.round(gear) < 0) {
      return 'inputs-gear-val inputs-gear-val--reverse';
    }
    return 'inputs-gear-val';
  }, [gear]);

  const rpmFillClass = useMemo(() => {
    if (rpmPct == null) return 'progress-fill--safe';
    if (rpmPct > 92) return 'progress-fill--crit';
    if (rpmPct > 80) return 'progress-fill--warn';
    return 'progress-fill--safe';
  }, [rpmPct]);

  return (
    <Panel title="INPUTS" dq={dq} testId="panel-driver-inputs">
      {/* Cockpit HUD: Gear and Speed */}
      <div className="inputs-cockpit-grid">
        <div className="inputs-col">
          <div className="card-row-label">GEAR</div>
          <div className={gearClass} data-testid="inputs-gear">
            {formatGear(gear)}
          </div>
        </div>
        <div className="inputs-col">
          <div className="card-row-label">SPEED</div>
          <div className="inputs-speed-val" data-testid="inputs-speed">
            {formatSpeed(speed, { withUnit: true })}
          </div>
        </div>
      </div>

      {/* RPM Bar — rendered only when RPM_MAX is known (T15) */}
      <div className="energy-section">
        <div className="card-row energy-header-row">
          <span className="card-row-label">RPM</span>
          <span className="card-row-value">
            {rpm != null ? Math.round(rpm).toLocaleString() : NO_VALUE}
          </span>
        </div>
        {hasRpmMax && rpmPct != null && (
          <div data-testid="rpm-bar" className="progress-track">
            <div
              className={`progress-fill ${rpmFillClass}`}
              style={{ width: `${Math.min(100, Math.max(0, rpmPct))}%` }}
            />
          </div>
        )}
      </div>

      {/* Throttle & Brake Pedals (Dual linear bars) */}
      <div className="pedal-bar-container">
        {/* Throttle */}
        <div className="pedal-col">
          <div className="pedal-header">
            <span className="card-row-label pedal-label--throttle">THROTTLE</span>
            <span className="card-row-value">
              {throttle != null ? `${throttlePct.toFixed(0)}%` : NO_VALUE}
            </span>
          </div>
          <div className="pedal-bar-track">
            <div
              className="pedal-bar-fill--throttle"
              style={{ width: `${throttlePct}%` }}
            />
          </div>
        </div>

        {/* Brake */}
        <div className="pedal-col">
          <div className="pedal-header">
            <span className="card-row-label pedal-label--brake">BRAKE</span>
            <span className="card-row-value">
              {brake != null ? `${brakePct.toFixed(0)}%` : NO_VALUE}
            </span>
          </div>
          <div className="pedal-bar-track">
            <div
              className="pedal-bar-fill--brake"
              style={{ width: `${brakePct}%` }}
            />
          </div>
        </div>
      </div>

      {/* Steering Angle and Clutch */}
      <div className="telemetry-grid-2x2">
        <div className="card-row">
          <span className="card-row-label">Steering Angle</span>
          <span className="card-row-value">
            {steeringDeg != null ? `${steeringDeg}°` : NO_VALUE}
          </span>
        </div>
        <div className="card-row">
          <span className="card-row-label">Clutch</span>
          <span className="card-row-value">
            {clutch != null ? `${(clutch * 100).toFixed(0)}%` : NO_VALUE}
          </span>
        </div>
      </div>
    </Panel>
  );
}
