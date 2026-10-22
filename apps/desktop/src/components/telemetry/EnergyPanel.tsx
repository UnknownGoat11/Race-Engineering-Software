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
          <dl className="tel-kv energy-header-row">
            <div className="tel-kv-row">
              <dt>Virtual energy remaining</dt>
              <dd>{(ve * 100).toFixed(1)}%</dd>
            </div>
          </dl>
          <div className="progress-track">
            <div
              className="progress-fill progress-fill--ve"
              style={{ width: `${Math.min(100, Math.max(0, ve * 100))}%` }}
            />
          </div>
        </div>
      ) : (
        <dl className="tel-kv energy-fallback-row">
          <div className="tel-kv-row tel-kv-row--prose">
            <dt>Virtual energy</dt>
            <dd className="tel-kv-prose">VE: not reported by this car</dd>
          </div>
        </dl>
      )}

      {/* Hybrid Section */}
      {hasHybrid ? (
        <div className="hybrid-section">
          {soc != null && (
            <div className="energy-section">
              <dl className="tel-kv energy-header-row">
                <div className="tel-kv-row">
                  <dt title="Battery state of charge">Battery SoC</dt>
                  <dd>{(soc * 100).toFixed(1)}%</dd>
                </div>
              </dl>
              <div className="progress-track">
                <div
                  className="progress-fill progress-fill--safe"
                  style={{ width: `${Math.min(100, Math.max(0, soc * 100))}%` }}
                />
              </div>
            </div>
          )}

          <dl className="tel-kv">
            <div className="tel-kv-row">
              <dt>Motor torque</dt>
              <dd>{motorTorque != null ? `${motorTorque.toFixed(1)} Nm` : NO_VALUE}</dd>
            </div>
            <div className="tel-kv-row">
              <dt>Motor RPM</dt>
              <dd>{motorRpm != null ? Math.round(motorRpm).toLocaleString() : NO_VALUE}</dd>
            </div>
          </dl>
        </div>
      ) : (
        <dl className="tel-kv">
          <div className="tel-kv-row tel-kv-row--prose">
            <dt>Hybrid ERS</dt>
            <dd className="tel-kv-prose">Hybrid: none</dd>
          </div>
        </dl>
      )}
    </Panel>
  );
}
