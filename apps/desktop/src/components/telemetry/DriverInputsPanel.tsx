import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatSpeed, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

const INPUT_CHANNELS = [
  ch.SPEED,
  ch.RPM,
  ch.RPM_MAX,
  ch.GEAR,
  ch.THROTTLE,
  ch.BRAKE,
  ch.CLUTCH,
  ch.STEERING,
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
      <div className="card" data-testid="panel-driver-inputs">
        <div className="card-header">
          <span className="card-header-title">DRIVER INPUTS & POWERTRAIN</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
    );
  }
  return <DriverInputsPanelContent />;
}

function DriverInputsPanelContent(): JSX.Element {
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
  const maxRpm = num(ch.RPM_MAX) ?? 9000;
  const gear = num(ch.GEAR);
  const throttle = num(ch.THROTTLE);
  const brake = num(ch.BRAKE);
  const clutch = num(ch.CLUTCH);
  const steeringRad = num(ch.STEERING);

  const steeringDeg = steeringRad != null ? (steeringRad * 180) / Math.PI : null;
  const rpmPct = rpm != null && maxRpm > 0 ? (rpm / maxRpm) * 100 : 0;
  const throttlePct = throttle != null ? Math.min(100, Math.max(0, throttle * 100)) : 0;
  const brakePct = brake != null ? Math.min(100, Math.max(0, brake * 100)) : 0;

  return (
    <div className="card" data-testid="panel-driver-inputs">
      <div className="card-header">
        <span className="card-header-title">DRIVER INPUTS & POWERTRAIN</span>
      </div>
      <div className="card-body">
        {/* Cockpit HUD: Gear and Speed */}
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-around', margin: '4px 0 12px 0' }}>
          <div style={{ textAlign: 'center' }}>
            <div className="card-row-label">GEAR</div>
            <div
              style={{
                fontSize: '44px',
                fontWeight: 900,
                fontFamily: 'monospace',
                lineHeight: 1,
                color: 'var(--color-accent)',
              }}
              data-testid="inputs-gear"
            >
              {formatGear(gear)}
            </div>
          </div>
          <div style={{ textAlign: 'center' }}>
            <div className="card-row-label">SPEED</div>
            <div
              style={{
                fontSize: '32px',
                fontWeight: 700,
                fontFamily: 'monospace',
                lineHeight: 1,
              }}
              data-testid="inputs-speed"
            >
              {formatSpeed(speed, { withUnit: true })}
            </div>
          </div>
        </div>

        {/* RPM Bar */}
        <div style={{ marginBottom: '12px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '2px' }}>
            <span className="card-row-label">RPM</span>
            <span style={{ fontFamily: 'monospace', fontSize: '13px' }}>
              {rpm != null ? Math.round(rpm).toLocaleString() : NO_VALUE}
            </span>
          </div>
          <div
            style={{
              height: '8px',
              backgroundColor: 'var(--color-surface)',
              borderRadius: '4px',
              overflow: 'hidden',
            }}
          >
            <div
              style={{
                height: '100%',
                width: `${Math.min(100, Math.max(0, rpmPct))}%`,
                backgroundColor:
                  rpmPct > 92
                    ? 'var(--color-error)'
                    : rpmPct > 80
                      ? 'var(--color-warn)'
                      : 'var(--color-ahead)',
                transition: 'width 0.05s linear',
              }}
            />
          </div>
        </div>

        {/* Throttle & Brake Pedals (Dual linear bars) */}
        <div style={{ display: 'flex', gap: '12px', marginBottom: '12px' }}>
          {/* Throttle */}
          <div style={{ flex: 1 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '2px' }}>
              <span className="card-row-label" style={{ color: 'var(--color-ahead)' }}>
                THROTTLE
              </span>
              <span style={{ fontFamily: 'monospace', fontSize: '12px' }}>
                {throttle != null ? `${throttlePct.toFixed(0)}%` : NO_VALUE}
              </span>
            </div>
            <div
              style={{
                height: '10px',
                backgroundColor: 'var(--color-surface)',
                borderRadius: '4px',
                overflow: 'hidden',
              }}
            >
              <div
                style={{
                  height: '100%',
                  width: `${throttlePct}%`,
                  backgroundColor: 'var(--color-ahead)',
                  transition: 'width 0.05s linear',
                }}
              />
            </div>
          </div>

          {/* Brake */}
          <div style={{ flex: 1 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '2px' }}>
              <span className="card-row-label" style={{ color: 'var(--color-error)' }}>
                BRAKE
              </span>
              <span style={{ fontFamily: 'monospace', fontSize: '12px' }}>
                {brake != null ? `${brakePct.toFixed(0)}%` : NO_VALUE}
              </span>
            </div>
            <div
              style={{
                height: '10px',
                backgroundColor: 'var(--color-surface)',
                borderRadius: '4px',
                overflow: 'hidden',
              }}
            >
              <div
                style={{
                  height: '100%',
                  width: `${brakePct}%`,
                  backgroundColor: 'var(--color-error)',
                  transition: 'width 0.05s linear',
                }}
              />
            </div>
          </div>
        </div>

        {/* Steering Angle and Clutch */}
        <div className="telemetry-grid-2x2">
          <div className="card-row">
            <span className="card-row-label">Steering Angle</span>
            <span className="card-row-value">
              {steeringDeg != null ? `${steeringDeg.toFixed(1)}°` : NO_VALUE}
            </span>
          </div>
          <div className="card-row">
            <span className="card-row-label">Clutch</span>
            <span className="card-row-value">
              {clutch != null ? `${(clutch * 100).toFixed(0)}%` : NO_VALUE}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
