/**
 * Driver pit board (spec §88, §1814–1875, REDESIGN.md §3.1 & C7; v1.3 audit U04, U05, D01, D03, D04, V10).
 * High-visibility, glanceable display read from ~1 m. It fills the height below the app bar:
 * 0. Optional status row (LMU / REC / STREAM / ENG / CODE chips), passed in by the driver screen (D01)
 * 1. Flag strip from flags.summary (0x0B0C), always rendered: green for 0, neutral "Flag —" when unknown
 * 2. Delta to personal best: hero number centred, qualifier beside it, unit at ~40 % size (±0.050 s deadband, C12)
 * 3. Laps left on the limiting resource, fuel or VE (amber <= 3 laps, red <= 1 lap)
 * 4. Pit limiter: word plus a one-line caption; SPEED_LIMITER_ACTIVE (0x010D) first, legacy 0x010C as fallback
 * Strict requirement: no text < 16 px on this board. Missing values render "—", never an invented state.
 */
import { useContext, useMemo, type JSX, type ReactNode } from 'react';
import { ch } from '../../generated/channels';
import { formatDelta, formatFuel, formatFuelPerLap, formatLapTime, NO_VALUE } from '../../lib/units';
import { TelemetryStoreContext, useChannelValues } from '../../telemetry/hooks';
import { flagStripView, limiterView } from '../driver/driverBoardModel';

const DELTA_DEADBAND_S = 0.050;

const DRIVER_BOARD_CHANNELS = [
  ch.DELTA_PERSONAL_BEST,
  ch.DELTA_BEST,
  ch.LAP_TIME,
  ch.LAST_LAP_TIME,
  ch.BEST_LAP_TIME,
  ch.LAP_NUMBER,
  ch.FUEL,
  ch.FUEL_PER_LAP,
  ch.FUEL_LAPS_REMAINING,
  ch.FUEL_TO_ADD,
  ch.VE_LAPS_REMAINING,
  ch.SPEED_LIMITER_ACTIVE,
  ch.SPEED_LIMITER,
  ch.FLAG_SUMMARY,
  ch.YELLOW_FLAG_STATE,
  ch.BLUE_FLAG,
] as const;

export interface DriverTelemetryTabProps {
  /** Connection status row shown at the top of the board (D01). */
  readonly statusRow?: ReactNode;
}

export function DriverTelemetryTab({ statusRow }: DriverTelemetryTabProps = {}): JSX.Element {
  const store = useContext(TelemetryStoreContext);
  return store ? <DriverTelemetryContent statusRow={statusRow} /> : <DriverTelemetryEmpty statusRow={statusRow} />;
}

function DriverTelemetryEmpty({ statusRow }: DriverTelemetryTabProps): JSX.Element {
  return (
    <div className="pit-board pit-board--empty" data-testid="driver-telemetry-empty">
      {statusRow}
      <div className="pit-board-empty-message">No live telemetry store active.</div>
    </div>
  );
}

/** A number in the data font inside sans text (mono only for numbers and units). */
function Num({ children }: { readonly children: ReactNode }): JSX.Element {
  return <span className="pit-board-num">{children}</span>;
}

function DriverTelemetryContent({ statusRow }: DriverTelemetryTabProps): JSX.Element {
  const samples = useChannelValues(DRIVER_BOARD_CHANNELS);

  const channelMap = useMemo(() => {
    const map = new Map<number, number>();
    for (let i = 0; i < DRIVER_BOARD_CHANNELS.length; i++) {
      const id = DRIVER_BOARD_CHANNELS[i];
      const s = samples[i];
      if (id === undefined || !s) continue;
      if (typeof s.value === 'number' && Number.isFinite(s.value)) {
        map.set(id, s.value);
      } else if (typeof s.value === 'boolean') {
        // Bool channels (the speed limiter) arrive as booleans; without this they would read as "no data".
        map.set(id, s.value ? 1 : 0);
      }
    }
    return map;
  }, [samples]);

  const num = (id: number): number | null => channelMap.get(id) ?? null;

  // Delta to personal best and lap times
  const deltaPb = num(ch.DELTA_PERSONAL_BEST) ?? num(ch.DELTA_BEST);
  const lapTime = num(ch.LAP_TIME);
  const lastLapTime = num(ch.LAST_LAP_TIME);
  const bestLapTime = num(ch.BEST_LAP_TIME);
  const lapNumber = num(ch.LAP_NUMBER);

  // Limiting resource: fuel vs VE (C7)
  const fuelL = num(ch.FUEL);
  const fuelPerLap = num(ch.FUEL_PER_LAP);
  const fuelLapsRemaining = num(ch.FUEL_LAPS_REMAINING);
  const fuelToAdd = num(ch.FUEL_TO_ADD);
  const veLapsLeft = num(ch.VE_LAPS_REMAINING);

  const isVeLimiting = veLapsLeft !== null && (fuelLapsRemaining === null || veLapsLeft < fuelLapsRemaining);
  const limitingResourceLabel = isVeLimiting ? 'VE' : 'fuel';
  const limitingLapsRemaining = isVeLimiting ? veLapsLeft : fuelLapsRemaining;

  const lapsLevelClass = (() => {
    if (limitingLapsRemaining === null) return 'pit-board-level-off';
    if (limitingLapsRemaining <= 1.0) return 'pit-board-level-critical';
    if (limitingLapsRemaining <= 3.0) return 'pit-board-level-warn';
    return 'pit-board-level-ok';
  })();

  const flag = flagStripView(num(ch.FLAG_SUMMARY), num(ch.YELLOW_FLAG_STATE), num(ch.BLUE_FLAG));
  const limiter = limiterView(num(ch.SPEED_LIMITER_ACTIVE), num(ch.SPEED_LIMITER));
  // Amber only when the limiter is on (it matters while driving); off and unknown stay neutral.
  const limiterLevelClass = limiter.state === 'on' ? 'pit-board-level-warn' : 'pit-board-level-off';

  // Delta deadband & qualifier (C12)
  const deltaInfo = (() => {
    if (deltaPb === null) return { className: 'delta-neutral', qualifier: 'No delta', value: NO_VALUE };
    const value = formatDelta(deltaPb);
    if (Math.abs(deltaPb) <= DELTA_DEADBAND_S) return { className: 'delta-zero', qualifier: 'On pace', value };
    if (deltaPb < -DELTA_DEADBAND_S) return { className: 'delta-negative', qualifier: '▼ Faster', value };
    return { className: 'delta-positive', qualifier: '▲ Slower', value };
  })();

  return (
    <div className="pit-board" data-testid="driver-telemetry-board">
      {statusRow}

      {/* Flag strip: always present, same height, so the board never has a hole (U04, V10) */}
      <div className="pit-board-flag-banner" data-testid="driver-flag-banner">
        <div className={`flag-strip flag-strip-${flag.kind}`} data-testid="driver-flag-strip" data-flag={flag.kind}>
          {flag.glyph !== '' && (
            <span className="flag-strip-glyph" aria-hidden="true">
              {flag.glyph}
            </span>
          )}
          <span>{flag.text}</span>
        </div>
      </div>

      {/* Delta to personal best: number centred, qualifier beside it (D03) */}
      <div className="pit-board-hero" data-testid="driver-delta-hero">
        <div className="pit-board-hero-header">
          <span className="pit-board-hero-title">Delta to personal best</span>
        </div>

        <div className="pit-board-hero-main">
          <div className={`pit-board-hero-value ${deltaInfo.className}`} data-testid="driver-delta-value">
            {deltaInfo.value}
            {deltaInfo.value !== NO_VALUE && <span className="pit-board-hero-unit">s</span>}
          </div>
          <span className={`pit-board-hero-badge ${deltaInfo.className}`} data-testid="driver-delta-qualifier">
            {deltaInfo.qualifier}
          </span>
        </div>

        <div className="pit-board-hero-footer">
          <span>
            Lap <Num>{lapNumber !== null ? Math.round(lapNumber) : NO_VALUE}</Num>
          </span>
          <span>
            Current <Num>{formatLapTime(lapTime, { allowZero: true })}</Num>
          </span>
          <span data-testid="driver-last-lap">
            Last lap <Num>{formatLapTime(lastLapTime)}</Num>
          </span>
          <span>
            Best <Num>{formatLapTime(bestLapTime)}</Num>
          </span>
        </div>
      </div>

      <div className="pit-board-grid">
        {/* Laps left on the limiting resource */}
        <div className={`pit-board-tile ${lapsLevelClass}`} data-testid="driver-fuel-tile">
          <div className="pit-board-tile-header">
            <span className="pit-board-tile-title">Laps left ({limitingResourceLabel})</span>
            <span className="pit-board-tile-glyph" aria-hidden="true">
              {limitingLapsRemaining === null
                ? ''
                : limitingLapsRemaining <= 1.0
                  ? '■'
                  : limitingLapsRemaining <= 3.0
                    ? '◆'
                    : '●'}
            </span>
          </div>

          <div className="pit-board-tile-body">
            <div className="pit-board-tile-value" data-testid="driver-laps-remaining">
              {limitingLapsRemaining !== null ? limitingLapsRemaining.toFixed(1) : NO_VALUE}
            </div>
          </div>

          <div className="pit-board-tile-footer">
            <div className="pit-board-tile-detail">
              Fuel <Num>{formatFuel(fuelL, { withUnit: true })}</Num> · avg{' '}
              <Num>{formatFuelPerLap(fuelPerLap, { withUnit: true })}</Num>
            </div>
            <div className="pit-board-tile-subtext">
              Fuel to add <Num>{formatFuel(fuelToAdd, { withUnit: true })}</Num>
            </div>
          </div>
        </div>

        {/* Pit limiter: word plus one caption (D04) */}
        <div className={`pit-board-tile ${limiterLevelClass}`} data-testid="driver-limiter-tile">
          <div className="pit-board-tile-header">
            <span className="pit-board-tile-title">Pit limiter</span>
            <span className="pit-board-tile-glyph" aria-hidden="true">
              {limiter.state === 'on' ? '◆' : ''}
            </span>
          </div>

          <div className="pit-board-tile-body">
            <div className="pit-board-tile-value pit-board-limiter-value" data-testid="driver-limiter-state">
              {limiter.word}
            </div>
          </div>

          <div className="pit-board-tile-footer">
            <div className="pit-board-tile-subtext" data-testid="driver-limiter-caption">
              {limiter.caption}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
