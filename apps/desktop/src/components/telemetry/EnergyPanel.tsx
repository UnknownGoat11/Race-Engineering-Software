import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
import { useScopeDq } from './presentation';
import './telemetry.css';

const ENERGY_CHANNELS = [
  ch.VIRTUAL_ENERGY,
  ch.BATTERY_SOC,
  ch.BATTERY_CHARGE,
  ch.HYBRID_MOTOR_TORQUE,
  ch.HYBRID_MOTOR_RPM,
] as const;

export function EnergyPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="POWER" testId="panel-energy">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <EnergyPanelContent />;
}

function EnergyPanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(ENERGY_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < ENERGY_CHANNELS.length; i++) {
      const id = ENERGY_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const ve = num(ch.VIRTUAL_ENERGY);
  const soc = num(ch.BATTERY_SOC) ?? num(ch.BATTERY_CHARGE);
  const motorTorque = num(ch.HYBRID_MOTOR_TORQUE);
  const motorRpm = num(ch.HYBRID_MOTOR_RPM);

  const hasVe = ve != null;
  const hasHybrid = soc != null || motorTorque != null || motorRpm != null;

  const metaContent = (
    <>
      {hasVe && (
        <StatusBadge level="info" glyph={false}>
          {(ve * 100).toFixed(1)}% VE
        </StatusBadge>
      )}
      {!hasHybrid ? (
        <StatusBadge level="off" glyph={false}>
          Hybrid: none
        </StatusBadge>
      ) : soc != null ? (
        <StatusBadge level="info" glyph={false}>
          SoC {(soc * 100).toFixed(0)}%
        </StatusBadge>
      ) : null}
    </>
  );

  return (
    <Panel title="POWER" dq={dq} meta={metaContent} testId="panel-energy">
      {/* Virtual Energy Section */}
      {hasVe ? (
        <div className="energy-section">
          <div className="card-row energy-header-row">
            <span className="card-row-label">Virtual Energy Remaining</span>
            <span className="card-row-value">
              {(ve * 100).toFixed(1)}%
            </span>
          </div>
          <div className="progress-track">
            <div
              className="progress-fill progress-fill--ve"
              style={{ width: `${Math.min(100, Math.max(0, ve * 100))}%` }}
            />
          </div>
        </div>
      ) : (
        <div className="card-row energy-fallback-row">
          <span className="card-row-label">Virtual Energy</span>
          <span className="card-row-value">VE: not reported by this car</span>
        </div>
      )}

      {/* Hybrid Section */}
      {hasHybrid ? (
        <div className="hybrid-section">
          {soc != null && (
            <div className="energy-section">
              <div className="card-row energy-header-row">
                <span className="card-row-label">Battery State of Charge (SoC)</span>
                <span className="card-row-value">
                  {(soc * 100).toFixed(1)}%
                </span>
              </div>
              <div className="progress-track">
                <div
                  className="progress-fill progress-fill--safe"
                  style={{ width: `${Math.min(100, Math.max(0, soc * 100))}%` }}
                />
              </div>
            </div>
          )}

          <div className="telemetry-grid-2x2">
            <div className="card-row">
              <span className="card-row-label">Motor Torque</span>
              <span className="card-row-value">
                {motorTorque != null ? `${motorTorque.toFixed(1)} Nm` : NO_VALUE}
              </span>
            </div>
            <div className="card-row">
              <span className="card-row-label">Motor RPM</span>
              <span className="card-row-value">
                {motorRpm != null ? Math.round(motorRpm).toLocaleString() : NO_VALUE}
              </span>
            </div>
          </div>
        </div>
      ) : (
        <div className="card-row">
          <span className="card-row-label">Hybrid ERS</span>
          <span className="card-row-value">Hybrid: none</span>
        </div>
      )}
    </Panel>
  );
}
