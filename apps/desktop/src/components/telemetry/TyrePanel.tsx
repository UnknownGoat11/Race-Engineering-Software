import { useContext, useMemo, type JSX } from 'react';
import { ch } from '../../generated/channels';
import { formatPressure, formatTemperature, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { Panel } from '../ui/Panel';
import { StatusBadge } from '../ui/StatusBadge';
import { useScopeDq } from './presentation';
import './telemetry.css';

const TYRE_CHANNELS = [
  // FL
  ch.TYRE_TEMP_INNER_FL,
  ch.TYRE_TEMP_MIDDLE_FL,
  ch.TYRE_TEMP_OUTER_FL,
  ch.TYRE_CARCASS_TEMP_FL,
  ch.TYRE_PRESSURE_FL,
  ch.TYRE_WEAR_FL,
  ch.TYRE_OPTIMAL_TEMP_FL,
  // FR
  ch.TYRE_TEMP_INNER_FR,
  ch.TYRE_TEMP_MIDDLE_FR,
  ch.TYRE_TEMP_OUTER_FR,
  ch.TYRE_CARCASS_TEMP_FR,
  ch.TYRE_PRESSURE_FR,
  ch.TYRE_WEAR_FR,
  ch.TYRE_OPTIMAL_TEMP_FR,
  // RL
  ch.TYRE_TEMP_INNER_RL,
  ch.TYRE_TEMP_MIDDLE_RL,
  ch.TYRE_TEMP_OUTER_RL,
  ch.TYRE_CARCASS_TEMP_RL,
  ch.TYRE_PRESSURE_RL,
  ch.TYRE_WEAR_RL,
  ch.TYRE_OPTIMAL_TEMP_RL,
  // RR
  ch.TYRE_TEMP_INNER_RR,
  ch.TYRE_TEMP_MIDDLE_RR,
  ch.TYRE_TEMP_OUTER_RR,
  ch.TYRE_CARCASS_TEMP_RR,
  ch.TYRE_PRESSURE_RR,
  ch.TYRE_WEAR_RR,
  ch.TYRE_OPTIMAL_TEMP_RR,
  // Balance & Stint
  ch.TYRE_TEMP_BALANCE_FRONT_REAR,
  ch.TYRE_TEMP_BALANCE_LEFT_RIGHT,
  ch.TYRE_SET_LAPS,
] as const;

interface CornerData {
  inner: number | null | undefined;
  mid: number | null | undefined;
  outer: number | null | undefined;
  carcass: number | null | undefined;
  pressure: number | null | undefined;
  wear: number | null | undefined;
  optimal: number | null | undefined;
}

interface ThermalBand {
  readonly modifier: string;
  readonly glyph: string;
  readonly tooltip: string;
}

/**
 * Derives thermal band per C11 and A09 relative to measured TYRE_OPTIMAL_TEMP_*.
 * - ±10 °C: optimal (●)
 * - +10 to +20 °C: warm (◆)
 * - -10 to -20 °C: low (◆)
 * - > +20 °C: overheating (■)
 * - < -20 °C: cold (▼)
 * - optimal missing: neutral cell
 */
function getThermalBand(temp: number | null | undefined, optimal: number | null | undefined): ThermalBand {
  if (temp == null || isNaN(temp)) {
    return { modifier: 'tyre-tread-cell--neutral', glyph: '', tooltip: 'Unavailable' };
  }
  if (optimal == null || isNaN(optimal)) {
    return { modifier: 'tyre-tread-cell--neutral', glyph: '', tooltip: 'Optimal window not reported' };
  }

  const diff = temp - optimal;
  if (diff < -20) {
    return { modifier: 'tyre-tread-cell--cold', glyph: '▼', tooltip: `Cold (${temp.toFixed(0)} °C vs opt ${optimal.toFixed(0)} °C)` };
  }
  if (diff < -10) {
    return { modifier: 'tyre-tread-cell--low', glyph: '◆', tooltip: `Low (${temp.toFixed(0)} °C vs opt ${optimal.toFixed(0)} °C)` };
  }
  if (diff <= 10) {
    return { modifier: 'tyre-tread-cell--optimal', glyph: '●', tooltip: `Optimal (${temp.toFixed(0)} °C vs opt ${optimal.toFixed(0)} °C)` };
  }
  if (diff <= 20) {
    return { modifier: 'tyre-tread-cell--warm', glyph: '◆', tooltip: `Warm (${temp.toFixed(0)} °C vs opt ${optimal.toFixed(0)} °C)` };
  }
  return { modifier: 'tyre-tread-cell--over', glyph: '■', tooltip: `Overheating (${temp.toFixed(0)} °C vs opt ${optimal.toFixed(0)} °C)` };
}

function renderCorner(
  title: string,
  code: string,
  data: CornerData,
  isLeft: boolean,
  testId: string,
): JSX.Element {
  // Left tyres: O - M - I. Right tyres: I - M - O (outer edge faces out per §2.6)
  const slots = isLeft
    ? [
        { label: 'O', val: data.outer },
        { label: 'M', val: data.mid },
        { label: 'I', val: data.inner },
      ]
    : [
        { label: 'I', val: data.inner },
        { label: 'M', val: data.mid },
        { label: 'O', val: data.outer },
      ];

  // v1.3 V06/U17: corner code only on screen ("FL"), full name in title / aria-label; value + unit never wrap.
  return (
    <div className="tyre-corner-box" data-testid={testId} role="group" aria-label={`${title} tyre`}>
      <div className="tyre-corner-header">
        <span className="tyre-corner-title" title={title}>
          {code}
        </span>
        <span className="tyre-corner-leader" aria-hidden="true" />
        <span className="tyre-corner-pressure">
          {formatPressure(data.pressure, { withUnit: true })}
        </span>
      </div>

      <div className="tyre-tread-grid">
        {slots.map((slot) => {
          const band = getThermalBand(slot.val, data.optimal);
          return (
            <div
              key={slot.label}
              className={`tyre-tread-cell ${band.modifier}`}
              title={band.tooltip}
            >
              {band.glyph && <span className="tyre-slot-glyph">{band.glyph}</span>}
              <span className="tyre-slot-label">{slot.label}</span>
              <span className="tyre-slot-val">{formatTemperature(slot.val)}</span>
            </div>
          );
        })}
      </div>

      <dl className="tyre-footer-kv">
        <dt>Carcass</dt>
        <dd data-testid={`${testId}-carcass`}>{formatTemperature(data.carcass, { withUnit: true })}</dd>
        <dt>Wear (raw)</dt>
        <dd data-testid={`${testId}-wear`}>{data.wear != null ? data.wear.toFixed(2) : NO_VALUE}</dd>
      </dl>
    </div>
  );
}

export function TyrePanel(): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  if (!store) {
    return (
      <Panel title="TYRES" testId="panel-tyres">
        <span className="card-row-label">No telemetry store active</span>
      </Panel>
    );
  }
  return <TyrePanelContent />;
}

function TyrePanelContent(): JSX.Element {
  const dq = useScopeDq();
  const samples = useChannelValues(TYRE_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < TYRE_CHANNELS.length; i++) {
      const id = TYRE_CHANNELS[i];
      const s = samples[i];
      if (id !== undefined && s && typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  const fl: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_FL),
    mid: num(ch.TYRE_TEMP_MIDDLE_FL),
    outer: num(ch.TYRE_TEMP_OUTER_FL),
    carcass: num(ch.TYRE_CARCASS_TEMP_FL),
    pressure: num(ch.TYRE_PRESSURE_FL),
    wear: num(ch.TYRE_WEAR_FL),
    optimal: num(ch.TYRE_OPTIMAL_TEMP_FL),
  };

  const fr: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_FR),
    mid: num(ch.TYRE_TEMP_MIDDLE_FR),
    outer: num(ch.TYRE_TEMP_OUTER_FR),
    carcass: num(ch.TYRE_CARCASS_TEMP_FR),
    pressure: num(ch.TYRE_PRESSURE_FR),
    wear: num(ch.TYRE_WEAR_FR),
    optimal: num(ch.TYRE_OPTIMAL_TEMP_FR),
  };

  const rl: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_RL),
    mid: num(ch.TYRE_TEMP_MIDDLE_RL),
    outer: num(ch.TYRE_TEMP_OUTER_RL),
    carcass: num(ch.TYRE_CARCASS_TEMP_RL),
    pressure: num(ch.TYRE_PRESSURE_RL),
    wear: num(ch.TYRE_WEAR_RL),
    optimal: num(ch.TYRE_OPTIMAL_TEMP_RL),
  };

  const rr: CornerData = {
    inner: num(ch.TYRE_TEMP_INNER_RR),
    mid: num(ch.TYRE_TEMP_MIDDLE_RR),
    outer: num(ch.TYRE_TEMP_OUTER_RR),
    carcass: num(ch.TYRE_CARCASS_TEMP_RR),
    pressure: num(ch.TYRE_PRESSURE_RR),
    wear: num(ch.TYRE_WEAR_RR),
    optimal: num(ch.TYRE_OPTIMAL_TEMP_RR),
  };

  const balFR = num(ch.TYRE_TEMP_BALANCE_FRONT_REAR);
  const balLR = num(ch.TYRE_TEMP_BALANCE_LEFT_RIGHT);
  const tyreSetLaps = num(ch.TYRE_SET_LAPS);

  const hasOptimal =
    fl.optimal != null || fr.optimal != null || rl.optimal != null || rr.optimal != null;

  const metaContent = (
    <>
      {balFR != null && (
        <StatusBadge level="info" glyph={false}>
          F/R {balFR > 0 ? '+' : ''}{balFR.toFixed(1)}°C
        </StatusBadge>
      )}
      {balLR != null && (
        <StatusBadge level="info" glyph={false}>
          L/R {balLR > 0 ? '+' : ''}{balLR.toFixed(1)}°C
        </StatusBadge>
      )}
      {tyreSetLaps != null && (
        <StatusBadge level="off" glyph={false}>
          {Math.round(tyreSetLaps)} LAPS
        </StatusBadge>
      )}
    </>
  );

  const footerContent = !hasOptimal ? (
    <span>Optimal window not reported</span>
  ) : null;

  return (
    <Panel
      title="TYRES"
      dq={dq}
      meta={metaContent}
      footer={footerContent}
      testId="panel-tyres"
    >
      <div className="tyres-grid-2x2">
        {renderCorner('Front left', 'FL', fl, true, 'tyre-fl')}
        {renderCorner('Front right', 'FR', fr, false, 'tyre-fr')}
        {renderCorner('Rear left', 'RL', rl, true, 'tyre-rl')}
        {renderCorner('Rear right', 'RR', rr, false, 'tyre-rr')}
      </div>
    </Panel>
  );
}
