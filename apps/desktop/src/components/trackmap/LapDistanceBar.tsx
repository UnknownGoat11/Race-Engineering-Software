import type { JSX } from 'react';
import { classColorVar } from '../../lib/classes';
import { carMarkerLabel, carVehicleTitle } from './carNumber';
import type { CompetitorMarker, DriverPosition, SectorMark, TrackMapDq } from './types';
import './trackmap.css';

export interface LapDistanceBarProps {
  readonly trackLengthM: number;
  readonly driverPosition?: DriverPosition | null | undefined;
  readonly competitors?: readonly CompetitorMarker[] | undefined;
  readonly sectors?: readonly SectorMark[] | undefined;
  readonly dq?: TrackMapDq | undefined;
  readonly progress?: number | undefined;
  readonly summary?: string | undefined;
  readonly className?: string | undefined;
}

/**
 * Fallback linear HUD representing circuit lap distance when 2D vector geometry is not yet available.
 * Shows S1/S2/S3 sector boundary ticks, competitors positioned along the lap coloured by class,
 * and the driver highlighted in white.
 */
export function LapDistanceBar({
  trackLengthM,
  driverPosition,
  competitors = [],
  sectors,
  dq,
  progress,
  summary,
  className = '',
}: LapDistanceBarProps): JSX.Element {
  const lengthM = trackLengthM > 0 ? trackLengthM : 5000;

  // Default sectors to 1/3 and 2/3 if not provided
  const s1Dist = sectors?.[0]?.distM ?? Math.round(lengthM / 3);
  const s2Dist = sectors?.[1]?.distM ?? Math.round((2 * lengthM) / 3);
  const s1Pct = Math.min(100, Math.max(0, (s1Dist / lengthM) * 100));
  const s2Pct = Math.min(100, Math.max(0, (s2Dist / lengthM) * 100));

  const totalCars = competitors.length + (driverPosition ? 1 : 0);
  const captionText = totalCars > 0 ? `${totalCars} cars (positions only)` : 'Awaiting competitor positions...';

  const isStale = dq === 'stale';
  const isEnded = dq === 'ended';

  return (
    <div
      className={`lap-distance-bar-wrapper ${isStale ? 'lap-dist-stale' : ''} ${isEnded ? 'lap-dist-ended' : ''} ${className}`}
      data-testid="lap-distance-bar"
      role="img"
      aria-label={summary ?? `Lap distance view: ${captionText}`}
    >
      <div className="lap-distance-bar-header">
        <span className="lap-dist-caption" data-testid="lap-dist-caption">
          {captionText}
        </span>
        {progress !== undefined && progress > 0 && progress < 1 && (
          <span className="lap-dist-builder-progress">
            Reconstructing circuit: {Math.round(progress * 100)}%
          </span>
        )}
      </div>

      <div className="lap-distance-bar-track">
        {/* S/F start line */}
        <div className="lap-dist-line-sf" style={{ left: '0%' }} title="Start / Finish line" />

        {/* Sector marks */}
        <div className="lap-dist-sector-mark" style={{ left: `${s1Pct}%` }} data-testid="sector-mark-1">
          <span className="lap-dist-sector-label">S2</span>
        </div>
        <div className="lap-dist-sector-mark" style={{ left: `${s2Pct}%` }} data-testid="sector-mark-2">
          <span className="lap-dist-sector-label">S3</span>
        </div>

        {/* Competitor cars */}
        {competitors.map((car) => {
          if (car.isDriver) return null;
          if (car.lapDistM === null || car.lapDistM === undefined || !Number.isFinite(car.lapDistM)) return null;

          const pct = Math.min(100, Math.max(0, ((car.lapDistM % lengthM) / lengthM) * 100));
          const color = classColorVar(car.vehicleClass);
          // Number only on the chip; the full vehicle name (if that is what was passed) goes in the tooltip (U10)
          const label = carMarkerLabel(car.carNumber);
          const vehicle = carVehicleTitle(car.carNumber);

          return (
            <div
              key={`car-${car.id}`}
              className={`lap-dist-car-chip ${car.inPits ? 'in-pits' : ''} ${isStale ? 'hollow' : ''}`}
              style={{
                left: `${pct}%`,
                borderColor: color,
                backgroundColor: isStale ? 'transparent' : color,
              }}
              data-testid={`competitor-chip-${label}`}
              title={`${vehicle} ${car.driverName} (${car.vehicleClass.toUpperCase()})`.trim()}
            >
              <span className="lap-dist-car-num">{label}</span>
            </div>
          );
        })}

        {/* Driver car */}
        {driverPosition?.lapDistM !== null &&
          driverPosition?.lapDistM !== undefined &&
          Number.isFinite(driverPosition.lapDistM) && (() => {
            const pct = Math.min(100, Math.max(0, ((driverPosition.lapDistM % lengthM) / lengthM) * 100));
            return (
              <div
                className={`lap-dist-car-driver ${isStale ? 'hollow' : ''}`}
                style={{ left: `${pct}%` }}
                data-testid="driver-lap-chip"
                title="Driver"
              >
                <div className="lap-dist-driver-halo" />
                <div className="lap-dist-driver-core" />
              </div>
            );
          })()}
      </div>

      <div className="lap-distance-bar-footer">
        <span className="lap-dist-scale-label">0 m (S/F)</span>
        <span className="lap-dist-scale-label">{(lengthM / 1000).toFixed(3)} km</span>
      </div>
    </div>
  );
}
