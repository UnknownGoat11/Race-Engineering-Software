import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';

const ENERGY_CHANNELS = [
  ch.VIRTUAL_ENERGY,
  ch.BATTERY_SOC,
  ch.BATTERY_CHARGE,
  ch.REGEN_POWER,
  ch.HYBRID_MOTOR_TORQUE,
  ch.HYBRID_MOTOR_RPM,
] as const;

export function EnergyPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <div className="card" data-testid="panel-energy">
        <div className="card-header">
          <span className="card-header-title">VIRTUAL ENERGY & HYBRID</span>
        </div>
        <div className="card-body">
          <span className="card-row-label">No telemetry store active</span>
        </div>
      </div>
    );
  }
  return <EnergyPanelContent />;
}

function EnergyPanelContent(): JSX.Element {
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
  const regenPower = num(ch.REGEN_POWER);
  const motorTorque = num(ch.HYBRID_MOTOR_TORQUE);
  const motorRpm = num(ch.HYBRID_MOTOR_RPM);

  const hasHybridData = ve != null || soc != null || motorTorque != null;

  return (
    <div className="card" data-testid="panel-energy">
      <div className="card-header">
        <span className="card-header-title">VIRTUAL ENERGY & HYBRID</span>
        {hasHybridData && ve != null && (
          <span className="status-badge status-badge-info">
            {(ve * 100).toFixed(1)}% VE
          </span>
        )}
      </div>
      <div className="card-body">
        {!hasHybridData ? (
          <div className="panel-note" style={{ textAlign: 'center', padding: '16px 8px' }}>
            N/A — Non-hybrid vehicle (no ERS / Virtual Energy)
          </div>
        ) : (
          <>
            {ve != null && (
              <div style={{ marginBottom: '12px' }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '4px' }}>
                  <span className="card-row-label">Virtual Energy Remaining</span>
                  <span style={{ fontWeight: 700, fontFamily: 'monospace' }}>
                    {(ve * 100).toFixed(1)}%
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
                      width: `${Math.min(100, Math.max(0, ve * 100))}%`,
                      backgroundColor: 'var(--color-accent)',
                      transition: 'width 0.3s ease',
                    }}
                  />
                </div>
              </div>
            )}

            {soc != null && (
              <div style={{ marginBottom: '12px' }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '4px' }}>
                  <span className="card-row-label">Battery State of Charge (SoC)</span>
                  <span style={{ fontWeight: 700, fontFamily: 'monospace' }}>
                    {(soc * 100).toFixed(1)}%
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
                      width: `${Math.min(100, Math.max(0, soc * 100))}%`,
                      backgroundColor: '#38bdf8',
                      transition: 'width 0.3s ease',
                    }}
                  />
                </div>
              </div>
            )}

            <div className="telemetry-grid-2x2">
              <div className="card-row">
                <span className="card-row-label">Regen Power</span>
                <span className="card-row-value">
                  {regenPower != null ? `${(regenPower / 1000).toFixed(1)} kW` : NO_VALUE}
                </span>
              </div>
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
          </>
        )}
      </div>
    </div>
  );
}
