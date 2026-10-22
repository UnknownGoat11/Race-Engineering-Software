import { type JSX } from 'react';
import { ch } from '../../generated/channels';
import { useChannelValues } from '../../telemetry/hooks';
import { formatClock, formatLapCountText, formatLapsText, formatWeather, joinUnit, NO_VALUE } from '../../lib/units';

export interface StrategyStripProps {
  readonly sessionClockS?: number | null;
  readonly completedLaps?: number | null;
  readonly trackTempC?: number | null;
  readonly airTempC?: number | null;
  /** Measured rain intensity 0..1; unknown (null) omits the weather item (audit U03). */
  readonly rainFrac?: number | null;
  /** Session flag text ("No caution", "Pending", …); null = not known ("—"). */
  readonly flagText?: string | null;
  /** Active caution: the flag gets a pill (warning amber for FCY / SC, critical for a red flag). */
  readonly flagLevel?: 'warn' | 'critical' | null;
}

const STRATEGY_CHANNEL_IDS = [
  ch.SESSION_TIME_REMAINING, // 0x0803
  ch.FUEL_LAPS_REMAINING,     // 0x0C03
  ch.FUEL_TO_ADD,             // 0x0C06
  ch.VE_LAPS_REMAINING,       // 0x0C13
  ch.STINT_LAPS,              // 0x0C21
  ch.STINT_MAX_LAPS,          // 0x0C23
  ch.TYRE_SET_LAPS,           // 0x0C24
] as const;

function LimitBadge(): JSX.Element {
  return (
    <span className="badge strategy-limit-badge" title="The resource that limits the stint">
      Limit
    </span>
  );
}

export function StrategyStrip({
  sessionClockS,
  completedLaps,
  trackTempC,
  airTempC,
  rainFrac,
  flagText,
  flagLevel,
}: StrategyStripProps): JSX.Element {
  const samples = useChannelValues(STRATEGY_CHANNEL_IDS);

  const sampleMap = new Map<number, number>();
  for (const s of samples) {
    if (typeof s.value === 'number' && Number.isFinite(s.value)) {
      sampleMap.set(s.id, s.value);
    }
  }

  const fuelLaps = sampleMap.get(ch.FUEL_LAPS_REMAINING) ?? null;
  const veLaps = sampleMap.get(ch.VE_LAPS_REMAINING) ?? null;
  const fuelToAdd = sampleMap.get(ch.FUEL_TO_ADD) ?? null;
  const stintLaps = sampleMap.get(ch.STINT_LAPS) ?? null;
  const stintMax = sampleMap.get(ch.STINT_MAX_LAPS) ?? null;
  const tyreSetLaps = sampleMap.get(ch.TYRE_SET_LAPS) ?? null;
  const sessionRemS = sampleMap.get(ch.SESSION_TIME_REMAINING) ?? sessionClockS ?? null;

  // Determine limiting resource for strategy
  const hasFuel = fuelLaps != null;
  const hasVe = veLaps != null;
  const isFuelLimit = hasFuel && hasVe && fuelLaps <= veLaps;
  const isVeLimit = hasFuel && hasVe && veLaps < fuelLaps;
  const weather = formatWeather(rainFrac);

  // Build items array, strictly omitting items that have no data source per §3.2
  const items: JSX.Element[] = [];

  if (sessionRemS != null) {
    items.push(
      <span key="session" className="strategy-item">
        <span className="strategy-label">Session</span>
        <span className="num strategy-value">{formatClock(Math.max(0, sessionRemS))}</span>
      </span>,
    );
  }

  if (completedLaps != null) {
    items.push(
      <span key="laps" className="strategy-item">
        <span className="strategy-label">Laps</span>
        <span className="num strategy-value">{completedLaps}</span>
      </span>,
    );
  }

  if (stintLaps != null) {
    items.push(
      <span key="stint" className="strategy-item">
        <span className="strategy-label">Stint</span>
        <span className="num strategy-value">
          {Math.floor(stintLaps)}
          {stintMax != null ? `/${String(Math.floor(stintMax))}` : ''}
        </span>
      </span>,
    );
  }

  if (fuelLaps != null) {
    items.push(
      <span key="fuel" className="strategy-item" data-testid="strategy-fuel">
        <span className="strategy-label">Fuel</span>
        <span className="num strategy-value">{formatLapsText(fuelLaps)}</span>
        {isFuelLimit && <LimitBadge />}
      </span>,
    );
  }

  if (veLaps != null) {
    items.push(
      <span key="ve" className="strategy-item" data-testid="strategy-ve">
        <span className="strategy-label">VE</span>
        <span className="num strategy-value">{formatLapsText(veLaps)}</span>
        {isVeLimit && <LimitBadge />}
      </span>,
    );
  }

  if (fuelToAdd != null && fuelToAdd > 0) {
    items.push(
      <span key="fuel_add" className="strategy-item" data-testid="strategy-fuel-add">
        <span className="strategy-label">Fuel to add</span>
        <span className="num strategy-value">{joinUnit(fuelToAdd.toFixed(1), 'L')}</span>
      </span>,
    );
  }

  if (tyreSetLaps != null) {
    items.push(
      <span key="tyres" className="strategy-item" data-testid="strategy-tyres">
        <span className="strategy-label">Tyres</span>
        <span className="num strategy-value">{formatLapCountText(tyreSetLaps)}</span>
      </span>,
    );
  }

  if (trackTempC != null || airTempC != null) {
    items.push(
      <span key="temps" className="strategy-item strategy-temps">
        {trackTempC != null && (
          <>
            <span className="strategy-label">Track</span>
            <span className="num strategy-value">{joinUnit(trackTempC.toFixed(0), '°C')}</span>
          </>
        )}
        {airTempC != null && (
          <>
            <span className="strategy-label">Air</span>
            <span className="num strategy-value">{joinUnit(airTempC.toFixed(0), '°C')}</span>
          </>
        )}
      </span>,
    );
  }

  if (weather !== null) {
    items.push(
      <span key="weather" className="strategy-item strategy-weather">
        <span className="strategy-text">{weather}</span>
      </span>,
    );
  }

  items.push(
    <span key="flag" className="strategy-item strategy-flag" data-testid="strategy-flag">
      <span className="strategy-label">Flag</span>
      {flagLevel != null && flagText != null ? (
        <span className={`badge ${flagLevel === 'critical' ? 'badge-status-critical' : 'badge-status-warn'}`}>
          {flagText}
        </span>
      ) : (
        <span className="strategy-text">{flagText ?? NO_VALUE}</span>
      )}
    </span>,
  );

  return (
    <div className="strategy-strip" data-testid="strategy-strip">
      {items.map((item, idx) => (
        <span key={item.key} className="strategy-item-wrapper">
          {idx > 0 && <span className="strategy-sep" aria-hidden="true">&bull;</span>}
          {item}
        </span>
      ))}
    </div>
  );
}
