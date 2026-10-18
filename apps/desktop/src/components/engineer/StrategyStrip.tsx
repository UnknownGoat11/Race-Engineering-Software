import { type JSX } from 'react';
import { ch } from '../../generated/channels';
import { useChannelValues } from '../../telemetry/hooks';
import { NO_VALUE } from '../../lib/units';

export interface StrategyStripProps {
  readonly sessionClockS?: number | null;
  readonly completedLaps?: number | null;
  readonly trackTempC?: number | null;
  readonly airTempC?: number | null;
  readonly weather?: string | null;
  readonly flagText?: string | null;
}

function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return NO_VALUE;
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const remS = s % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, '0')}:${remS.toString().padStart(2, '0')}`;
  }
  return `${m}:${remS.toString().padStart(2, '0')}`;
}

const STRATEGY_CHANNEL_IDS = [
  ch.SESSION_TIME_REMAINING, // 0x0803
  ch.FUEL_LAPS_REMAINING,     // 0x0C03
  ch.FUEL_TO_ADD,             // 0x0C06
  ch.VE_LAPS_REMAINING,       // 0x0C13
  ch.STINT_NUMBER,            // 0x0C20
  ch.STINT_LAPS,              // 0x0C21
  ch.STINT_TIME,              // 0x0C22
  ch.STINT_MAX_LAPS,          // 0x0C23
  ch.TYRE_SET_LAPS,           // 0x0C24
  ch.RACE_LAPS_REMAINING,     // 0x0C25
  ch.FLAG_SUMMARY,            // 0x0B0C
] as const;

export function StrategyStrip({
  sessionClockS,
  completedLaps,
  trackTempC,
  airTempC,
  weather,
  flagText,
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
  const isFuelLimit = hasFuel && (!hasVe || fuelLaps <= veLaps);
  const isVeLimit = hasVe && (!hasFuel || veLaps < fuelLaps);

  // Build items array, strictly omitting items that have no data source per §3.2
  const items: JSX.Element[] = [];

  if (sessionRemS != null) {
    items.push(
      <span key="session" className="strategy-item">
        <span className="strategy-label">Session</span>
        <span className="num strategy-value">{formatClock(sessionRemS)}</span>
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
          {stintMax != null ? `/${Math.floor(stintMax)}` : ''}
        </span>
      </span>,
    );
  }

  if (fuelLaps != null) {
    items.push(
      <span key="fuel" className="strategy-item">
        <span className="strategy-label">Fuel</span>
        <span className="num strategy-value">
          {fuelLaps.toFixed(1)} L{isFuelLimit && <span className="strategy-limit"> ◂limit</span>}
        </span>
      </span>,
    );
  }

  if (veLaps != null) {
    items.push(
      <span key="ve" className="strategy-item">
        <span className="strategy-label">VE</span>
        <span className="num strategy-value">
          {veLaps.toFixed(1)}
          {isVeLimit && <span className="strategy-limit"> ◂limit</span>}
        </span>
      </span>,
    );
  }

  if (fuelToAdd != null && fuelToAdd > 0) {
    items.push(
      <span key="fuel_add" className="strategy-item">
        <span className="strategy-label">Fuel+</span>
        <span className="num strategy-value">{fuelToAdd.toFixed(1)} L</span>
      </span>,
    );
  }

  if (tyreSetLaps != null) {
    items.push(
      <span key="tyres" className="strategy-item">
        <span className="strategy-label">Tyres</span>
        <span className="num strategy-value">{Math.floor(tyreSetLaps)} L</span>
      </span>,
    );
  }

  if (trackTempC != null || airTempC != null) {
    items.push(
      <span key="temps" className="strategy-item strategy-temps">
        {trackTempC != null && (
          <>
            <span className="strategy-label">Track</span>
            <span className="num strategy-value">{trackTempC.toFixed(0)} &deg;C</span>
          </>
        )}
        {airTempC != null && (
          <>
            <span className="strategy-label">Air</span>
            <span className="num strategy-value">{airTempC.toFixed(0)} &deg;C</span>
          </>
        )}
      </span>,
    );
  }

  if (weather) {
    items.push(
      <span key="weather" className="strategy-item strategy-weather">
        <span className="strategy-value">{weather}</span>
      </span>,
    );
  }

  if (flagText) {
    items.push(
      <span key="flag" className="strategy-item strategy-flag">
        <span className="badge badge-subtle">{flagText}</span>
      </span>,
    );
  }

  return (
    <div className="strategy-strip" data-testid="strategy-strip">
      {items.map((item, idx) => (
        <span key={item.key} className="strategy-item-wrapper">
          {idx > 0 && <span className="strategy-sep">&bull;</span>}
          {item}
        </span>
      ))}
    </div>
  );
}
