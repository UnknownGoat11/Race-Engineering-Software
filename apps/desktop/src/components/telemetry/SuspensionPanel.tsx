import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
import { useScopeDq } from './presentation';
import './telemetry.css';

const SUSPENSION_CHANNELS = [
  // Ride Heights (Corner & Axle)
  ch.RIDE_HEIGHT_FL,
  ch.RIDE_HEIGHT_FR,
  ch.RIDE_HEIGHT_RL,
  ch.RIDE_HEIGHT_RR,
  ch.RIDE_HEIGHT_FRONT,
  ch.RIDE_HEIGHT_REAR,
  // Deflections / Travel
  ch.SUSP_POS_FL,
  ch.SUSP_POS_FR,
  ch.SUSP_POS_RL,
  ch.SUSP_POS_RR,
  // Pushrod forces
  ch.PUSHROD_FORCE_FL,
  ch.PUSHROD_FORCE_FR,
  ch.PUSHROD_FORCE_RL,
  ch.PUSHROD_FORCE_RR,
  // Damage
  ch.DAMAGE_SUSPENSION_FL,
  ch.DAMAGE_SUSPENSION_FR,
  ch.DAMAGE_SUSPENSION_RL,
  ch.DAMAGE_SUSPENSION_RR,
  ch.DAMAGE_AERO,
] as const;

function formatMm(m: number | null | undefined): string {
  if (m == null || isNaN(m)) return NO_VALUE;
  return `${(m * 1000).toFixed(1)} mm`;
}

function formatForceN(n: number | null | undefined): string {
  if (n == null || isNaN(n)) return NO_VALUE;
  return `${Math.round(n).toLocaleString()} N`;
}

/** Damage fraction (0..1) as a percentage; NO_VALUE when the channel is not reported. */
function formatDamage(frac: number | null): string {
  return frac === null ? NO_VALUE : `${(frac * 100).toFixed(0)}%`;
}

function damageClass(frac: number | null): string {
  if (frac === null) return '';
  return frac > 0 ? 'susp-dmg-bad' : 'susp-dmg-ok';
}

export function SuspensionPanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="CHASSIS & SUSPENSION" testId="panel-suspension">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <SuspensionPanelContent />;
}

function SuspensionPanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(SUSPENSION_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < SUSPENSION_CHANNELS.length; i++) {
      const id = SUSPENSION_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  // Corner Ride Heights
  const rhFL = num(ch.RIDE_HEIGHT_FL);
  const rhFR = num(ch.RIDE_HEIGHT_FR);
  const rhRL = num(ch.RIDE_HEIGHT_RL);
  const rhRR = num(ch.RIDE_HEIGHT_RR);

  // Axle Heights
  const rhFront = num(ch.RIDE_HEIGHT_FRONT) ?? (rhFL != null && rhFR != null ? (rhFL + rhFR) / 2 : null);
  const rhRear = num(ch.RIDE_HEIGHT_REAR) ?? (rhRL != null && rhRR != null ? (rhRL + rhRR) / 2 : null);

  // Deflection / Travel
  const dFL = num(ch.SUSP_POS_FL);
  const dFR = num(ch.SUSP_POS_FR);
  const dRL = num(ch.SUSP_POS_RL);
  const dRR = num(ch.SUSP_POS_RR);

  // Pushrod force
  const fFL = num(ch.PUSHROD_FORCE_FL);
  const fFR = num(ch.PUSHROD_FORCE_FR);
  const fRL = num(ch.PUSHROD_FORCE_RL);
  const fRR = num(ch.PUSHROD_FORCE_RR);

  // Damage
  // Damage: LMU does not provide these yet (registry `missing`), so they are usually null -> shown as NO_VALUE,
  // never as "intact".
  const dmgFL = num(ch.DAMAGE_SUSPENSION_FL);
  const dmgFR = num(ch.DAMAGE_SUSPENSION_FR);
  const dmgRL = num(ch.DAMAGE_SUSPENSION_RL);
  const dmgRR = num(ch.DAMAGE_SUSPENSION_RR);
  const dmgAero = num(ch.DAMAGE_AERO);

  const rakeMm = rhFront != null && rhRear != null ? (rhRear - rhFront) * 1000 : null;

  const metaContent = (
    <>
      {rakeMm != null && (
        <StatusBadge level="info" glyph={false}>
          RAKE {rakeMm >= 0 ? `+${rakeMm.toFixed(1)}` : rakeMm.toFixed(1)} mm
        </StatusBadge>
      )}
      <StatusBadge level={dmgAero === null ? 'off' : dmgAero > 0 ? 'critical' : 'ok'} glyph={false}>
        {`AERO DAMAGE: ${formatDamage(dmgAero)}`}
      </StatusBadge>
    </>
  );

  return (
    <Panel title="CHASSIS & SUSPENSION" dq={dq} meta={metaContent} testId="panel-suspension">
      {/* 2x2 Corner Suspension & Ride Heights */}
      <div className="suspension-corners-grid">
        {/* FL */}
        <div className="susp-corner-card" role="group" aria-label="Front left suspension">
          <div className="susp-corner-head">
            <span className="susp-corner-tag" title="Front left">
              FL
            </span>
            <span className={`susp-dmg-badge ${damageClass(dmgFL)}`} data-testid="susp-dmg-fl">
              {`DMG ${formatDamage(dmgFL)}`}
            </span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Ride height</span>
            <span className="susp-metric-val">{formatMm(rhFL)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Deflection</span>
            <span className="susp-metric-val">{formatMm(dFL)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Pushrod</span>
            <span className="susp-metric-val">{formatForceN(fFL)}</span>
          </div>
        </div>

        {/* FR */}
        <div className="susp-corner-card" role="group" aria-label="Front right suspension">
          <div className="susp-corner-head">
            <span className="susp-corner-tag" title="Front right">
              FR
            </span>
            <span className={`susp-dmg-badge ${damageClass(dmgFR)}`} data-testid="susp-dmg-fr">
              {`DMG ${formatDamage(dmgFR)}`}
            </span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Ride height</span>
            <span className="susp-metric-val">{formatMm(rhFR)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Deflection</span>
            <span className="susp-metric-val">{formatMm(dFR)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Pushrod</span>
            <span className="susp-metric-val">{formatForceN(fFR)}</span>
          </div>
        </div>

        {/* RL */}
        <div className="susp-corner-card" role="group" aria-label="Rear left suspension">
          <div className="susp-corner-head">
            <span className="susp-corner-tag" title="Rear left">
              RL
            </span>
            <span className={`susp-dmg-badge ${damageClass(dmgRL)}`} data-testid="susp-dmg-rl">
              {`DMG ${formatDamage(dmgRL)}`}
            </span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Ride height</span>
            <span className="susp-metric-val">{formatMm(rhRL)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Deflection</span>
            <span className="susp-metric-val">{formatMm(dRL)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Pushrod</span>
            <span className="susp-metric-val">{formatForceN(fRL)}</span>
          </div>
        </div>

        {/* RR */}
        <div className="susp-corner-card" role="group" aria-label="Rear right suspension">
          <div className="susp-corner-head">
            <span className="susp-corner-tag" title="Rear right">
              RR
            </span>
            <span className={`susp-dmg-badge ${damageClass(dmgRR)}`} data-testid="susp-dmg-rr">
              {`DMG ${formatDamage(dmgRR)}`}
            </span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Ride height</span>
            <span className="susp-metric-val">{formatMm(rhRR)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Deflection</span>
            <span className="susp-metric-val">{formatMm(dRR)}</span>
          </div>
          <div className="susp-metric-row">
            <span className="susp-metric-lbl">Pushrod</span>
            <span className="susp-metric-val">{formatForceN(fRR)}</span>
          </div>
        </div>
      </div>

      {/* Axle Level Ride Height Comparison */}
      <div className="suspension-axle-summary">
        <div className="axle-rh-box">
          <span className="card-row-label">Front axle height</span>
          <span className="axle-rh-val">{formatMm(rhFront)}</span>
        </div>
        <div className="axle-rh-box">
          <span className="card-row-label">Rear axle height</span>
          <span className="axle-rh-val">{formatMm(rhRear)}</span>
        </div>
      </div>
    </Panel>
  );
}
