/**
 * Live Track Map Component (spec §23–24, §79, §155–157, §342).
 * Renders circuit vector geometry, sectors, pit marks, competitor positions, driver vehicle marker,
 * dynamic class legend chips, or automatic online reconstructed track geometry and LapDistanceBar fallback.
 */
import { useContext, useEffect, useRef, useState, type JSX } from 'react';
import { useScoring } from '../../hooks/useScoring';
import {
  classColorVar,
  parseVehicleClass,
  vehicleClassCode,
  vehicleClassName,
  type VehicleClass,
} from '../../lib/classes';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { LapDistanceBar } from './LapDistanceBar';
import { buildSvgPath, classToColor, positionAt } from './trackGeometry';
import { carMarkerLabel, carVehicleTitle } from './carNumber';
import { applyTransform, computeBounds, fitMapTransform, fitPointsFor } from './transform';
import type { CompetitorMarker, DriverPosition, TrackMap as TrackMapData, TrackMapDq } from './types';
import { useTrackGeometry } from './useTrackGeometry';
import './trackmap.css';

export interface TrackMapProps {
  /** Static, verified, or dynamic track map geometry. If undefined, reconstructs online from telemetry. */
  readonly map?: TrackMapData | null;
  /** Data quality state of the track map (spec §2.2). */
  readonly dq?: TrackMapDq;
  /** Driver vehicle live position & orientation. */
  readonly driverPosition?: DriverPosition | null;
  /** Competitor vehicles from live scoring. */
  readonly competitors?: readonly CompetitorMarker[];
  /** Manual or layout rotation in radians (counter-clockwise). */
  readonly rotationRad?: number;
  /** Callback when rotation changes via user controls. */
  readonly onRotationChange?: (rad: number) => void;
  /** Track name override if map metadata is not yet populated. */
  readonly trackName?: string | null;
  /** Track length in metres override. */
  readonly trackLengthM?: number | null;
  /** Driver position rank / standing (e.g. 7 for P7). Optional, auto-inferred from scoring if omitted. */
  readonly driverPlace?: number | null;
  /** Driver current sector (e.g. 1, 2, or 3). Optional, auto-inferred from scoring or lapDistM if omitted. */
  readonly driverSector?: number | null;
  /**
   * Driver vehicle car number (e.g. "92"). Optional, auto-inferred from scoring if omitted. A full vehicle name is
   * accepted too: the marker shows only the extracted number (v1.3 U10).
   */
  readonly driverCarNumber?: string | null;
  /** Driver vehicle class. Optional, auto-inferred from scoring if omitted. */
  readonly driverVehicleClass?: VehicleClass | null;
  /** Additional container CSS class. */
  readonly className?: string;
}

/**
 * Builds descriptive aria-label summary for the track map (e.g. "Sebring, 25 cars, driver P7 in sector 2").
 */
function buildAriaSummary({
  trackName,
  totalCars,
  driverPlace,
  driverSector,
}: {
  trackName: string | null;
  totalCars: number;
  driverPlace: number | null;
  driverSector: number | null;
}): string {
  const parts: string[] = [];

  const name =
    trackName && trackName.trim() !== '' && trackName !== '—' && trackName !== 'Unknown Track'
      ? trackName.trim()
      : 'Circuit';
  parts.push(name);

  if (totalCars === 1) {
    parts.push('1 car');
  } else {
    parts.push(`${totalCars} cars`);
  }

  if (driverPlace != null && driverSector != null) {
    parts.push(`driver P${driverPlace} in sector ${driverSector}`);
  } else if (driverPlace != null) {
    parts.push(`driver P${driverPlace}`);
  } else if (driverSector != null) {
    parts.push(`driver in sector ${driverSector}`);
  }

  return parts.join(', ');
}

/** Marker tooltip: "#23 Mercedes-AMG LMGT3 · A. Driver"; a bare number gets its '#'. */
function markerTooltip(vehicleName: string | null | undefined, driverName: string | null | undefined): string {
  return [carVehicleTitle(vehicleName), (driverName ?? '').trim()].filter((part) => part !== '').join(' · ');
}

export function TrackMap({
  map,
  dq,
  driverPosition,
  competitors = [],
  rotationRad = 0,
  onRotationChange,
  trackName,
  trackLengthM,
  driverPlace,
  driverSector,
  driverCarNumber: explicitDriverCarNumber,
  driverVehicleClass,
  className = '',
}: TrackMapProps): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [dimensions, setDimensions] = useState<{ width: number; height: number }>({ width: 0, height: 0 });

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect;
        if (width > 0 && height > 0) {
          setDimensions({ width, height });
        }
      }
    });

    observer.observe(el);
    return () => {
      observer.disconnect();
    };
  }, []);

  // Online geometry reconstruction when map is not explicitly provided
  const store = useContext(TelemetryStoreContext);
  const scoring = useScoring();
  const autoGeometry = useTrackGeometry(
    map === undefined ? store : null,
    map === undefined ? scoring : null,
    trackLengthM,
    trackName,
  );

  const effectiveMap = map !== undefined ? map : autoGeometry.map;
  const effectiveProgress = autoGeometry.progress;

  const hasMapPoints = effectiveMap !== null && effectiveMap.points.length >= 2;
  const rawDisplayName =
    (trackName && trackName.trim() !== '' && trackName !== '—' && trackName !== 'Unknown Track'
      ? trackName.trim()
      : null) ??
    effectiveMap?.trackId ??
    null;
  const displayName =
    rawDisplayName && rawDisplayName.trim() !== '' && rawDisplayName !== '—' && rawDisplayName !== 'Unknown Track'
      ? rawDisplayName.trim()
      : null;
  const displayLengthM = effectiveMap?.lengthM ?? trackLengthM ?? null;

  const isStale = dq === 'stale';
  const isEnded = dq === 'ended';

  // Driver car number derivation: the marker shows the number only, the full name goes in the tooltip (U10)
  const driverVehicleName =
    explicitDriverCarNumber ??
    (scoring?.vehicles ? scoring.vehicles.find((v) => v.is_player)?.vehicle_name ?? null : null);
  const driverCarNumber = driverVehicleName ? carMarkerLabel(driverVehicleName) : '';

  // Classes currently present in the session (A09 legend)
  const presentClasses = (() => {
    const set = new Set<VehicleClass>();
    for (const c of competitors) {
      if (c.vehicleClass !== 'unknown') {
        set.add(c.vehicleClass);
      }
    }
    if (driverVehicleClass !== undefined && driverVehicleClass !== null && driverVehicleClass !== 'unknown') {
      set.add(driverVehicleClass);
    }
    if (scoring?.vehicles) {
      for (const v of scoring.vehicles) {
        const cls = parseVehicleClass(v.class_name);
        if (cls !== 'unknown') {
          set.add(cls);
        }
      }
    }
    const order: VehicleClass[] = ['hypercar', 'lmp2', 'gt3', 'gte'];
    const ordered = order.filter((cls) => set.has(cls));
    if (set.has('unknown')) {
      ordered.push('unknown');
    }
    return ordered;
  })();

  // Standing place and current sector for aria summary
  const effectiveDriverPlace = (() => {
    if (driverPlace != null) return driverPlace;
    if (scoring?.vehicles) {
      const p = scoring.vehicles.find((v) => v.is_player);
      if (p?.place != null) return p.place;
    }
    return null;
  })();

  const effectiveDriverSector = (() => {
    if (driverSector != null) return driverSector;
    if (scoring?.vehicles) {
      const p = scoring.vehicles.find((v) => v.is_player);
      if (p?.sector != null) return p.sector;
    }
    if (driverPosition?.lapDistM != null && Number.isFinite(driverPosition.lapDistM)) {
      const dist = driverPosition.lapDistM;
      if (effectiveMap?.sectors && effectiveMap.sectors.length >= 2) {
        const sec0 = effectiveMap.sectors[0];
        const sec1 = effectiveMap.sectors[1];
        if (sec0 && dist < sec0.distM) return 1;
        if (sec1 && dist < sec1.distM) return 2;
        return 3;
      }
      const len = displayLengthM ?? 5000;
      if (dist < len / 3) return 1;
      if (dist < (2 * len) / 3) return 2;
      return 3;
    }
    return null;
  })();

  const totalCars = competitors.length + (driverPosition ? 1 : 0);
  const ariaSummary = buildAriaSummary({
    trackName: displayName,
    totalCars,
    driverPlace: effectiveDriverPlace,
    driverSector: effectiveDriverSector,
  });

  // Compute transform when dimensions and map are valid
  const transform = (() => {
    if (!hasMapPoints || dimensions.width <= 0 || dimensions.height <= 0) {
      return null;
    }
    // Generated geometry may be partial (it locks at 65 % coverage): fit to geometry + cars so none is off-map (V04)
    const fitPoints = fitPointsFor(
      effectiveMap.points,
      driverPosition ? [...competitors, driverPosition] : competitors,
      effectiveMap.source === 'verified',
    );
    const bounds = computeBounds(fitPoints);
    if (!bounds) {
      return null;
    }
    return fitMapTransform(bounds, fitPoints, dimensions.width, dimensions.height, 36, rotationRad);
  })();

  // Driver marker coordinates
  const driverScreenPos = (() => {
    if (!transform || !driverPosition) return null;
    let wx = driverPosition.x;
    let wz = driverPosition.z;

    if ((wx === null || wz === null || !Number.isFinite(wx) || !Number.isFinite(wz)) && effectiveMap) {
      if (driverPosition.lapDistM !== null && driverPosition.lapDistM !== undefined) {
        const pt = positionAt(effectiveMap, driverPosition.lapDistM);
        if (pt) {
          [wx, wz] = pt;
        }
      }
    }

    if (wx !== null && wz !== null && Number.isFinite(wx) && Number.isFinite(wz)) {
      return applyTransform(transform, wx, wz);
    }
    return null;
  })();

  // Competitor marker coordinates
  const competitorMarkers = (() => {
    if (!transform) return [];
    const results: { u: number; v: number; competitor: CompetitorMarker }[] = [];

    for (const c of competitors) {
      if (c.isDriver) continue;
      let wx = c.x ?? null;
      let wz = c.z ?? null;

      if ((wx === null || wz === null || !Number.isFinite(wx) || !Number.isFinite(wz)) && effectiveMap) {
        if (c.lapDistM !== null && c.lapDistM !== undefined) {
          const pt = positionAt(effectiveMap, c.lapDistM);
          if (pt) {
            [wx, wz] = pt;
          }
        }
      }

      if (wx !== null && wz !== null && Number.isFinite(wx) && Number.isFinite(wz)) {
        const [u, v] = applyTransform(transform, wx, wz);
        results.push({ u, v, competitor: c });
      }
    }
    return results;
  })();

  const handleRotate = (delta: number) => {
    if (onRotationChange) {
      onRotationChange(rotationRad + delta);
    }
  };

  const showTitleBox = displayName !== null || displayLengthM !== null || isStale;

  return (
    <div
      className={`trackmap-container ${isEnded ? 'trackmap-ended' : ''} ${className}`}
      ref={containerRef}
      data-testid="trackmap-container"
      role="region"
      aria-label={`Track map: ${ariaSummary}`}
    >
      <div className="trackmap-header">
        {showTitleBox && (
          <div className="trackmap-title-box">
            {displayName !== null && (
              <span className="trackmap-track-name" data-testid="trackmap-title">
                {displayName}
              </span>
            )}
            {displayLengthM !== null && (
              <span className="trackmap-track-length" data-testid="trackmap-length">
                {(displayLengthM / 1000).toFixed(3)} km
              </span>
            )}
            {isStale && (
              <span className="trackmap-stale-tag" data-testid="trackmap-stale-tag">
                STALE
              </span>
            )}
          </div>
        )}

        <div className="trackmap-header-right">
          {presentClasses.length > 0 && (
            <div className="trackmap-legend" data-testid="trackmap-legend" role="list" aria-label="Class legend">
              {presentClasses.map((cls) => {
                const code = vehicleClassCode(cls);
                const name = vehicleClassName(cls);
                return (
                  <span
                    key={cls}
                    className="trackmap-legend-chip"
                    data-testid={`trackmap-legend-${code.toLowerCase()}`}
                    data-class={cls}
                    title={name}
                    role="listitem"
                  >
                    <span
                      className="trackmap-legend-dot"
                      style={{ backgroundColor: classColorVar(cls) }}
                      aria-hidden="true"
                    />
                    <span className="trackmap-legend-code">{code}</span>
                  </span>
                );
              })}
            </div>
          )}

          {onRotationChange && (
            <div className="trackmap-controls" data-testid="trackmap-controls">
              <button
                type="button"
                className="btn btn-sm"
                data-testid="trackmap-btn-rotate-ccw"
                title={hasMapPoints ? 'Rotate counter-clockwise 15°' : 'Rotate counter-clockwise (no geometry)'}
                disabled={!hasMapPoints}
                aria-disabled={!hasMapPoints}
                onClick={() => {
                  if (hasMapPoints) handleRotate(Math.PI / 12);
                }}
              >
                ↺
              </button>
              <button
                type="button"
                className="btn btn-sm"
                data-testid="trackmap-btn-rotate-cw"
                title={hasMapPoints ? 'Rotate clockwise 15°' : 'Rotate clockwise (no geometry)'}
                disabled={!hasMapPoints}
                aria-disabled={!hasMapPoints}
                onClick={() => {
                  if (hasMapPoints) handleRotate(-Math.PI / 12);
                }}
              >
                ↻
              </button>
              <button
                type="button"
                className="btn btn-sm"
                data-testid="trackmap-btn-rotate-reset"
                title={hasMapPoints ? 'Reset rotation' : 'Reset rotation (no geometry)'}
                disabled={!hasMapPoints}
                aria-disabled={!hasMapPoints}
                onClick={() => {
                  if (hasMapPoints) onRotationChange(0);
                }}
              >
                0°
              </button>
            </div>
          )}
        </div>
      </div>

      {transform && effectiveMap ? (
        <svg
          className="trackmap-svg"
          viewBox={`0 0 ${dimensions.width} ${dimensions.height}`}
          data-testid="trackmap-svg"
          role="img"
          aria-label={ariaSummary}
        >
          {/* Circuit polyline base and groove */}
          <path d={buildSvgPath(effectiveMap.points, transform)} className="trackmap-path-base" />
          <path d={buildSvgPath(effectiveMap.points, transform)} className="trackmap-path-inner" />

          {/* Sector Marks */}
          {effectiveMap.sectors.map((sector) => {
            const [u, v] = applyTransform(transform, sector.x, sector.z);
            return (
              <g key={`sector-${sector.index}`} data-testid={`sector-${sector.index}`}>
                <circle cx={u} cy={v} r={3} className="trackmap-sector-dot" />
                <text x={u} y={v - 8} className="trackmap-sector-text">
                  S{sector.index + 1}
                </text>
              </g>
            );
          })}

          {/* Start/Finish Line */}
          {(() => {
            const [u, v] = applyTransform(transform, effectiveMap.startFinish.x, effectiveMap.startFinish.z);
            return (
              <g data-testid="start-finish-line">
                <circle cx={u} cy={v} r={4} className="trackmap-start-finish-dot" />
                <line x1={u - 6} y1={v} x2={u + 6} y2={v} className="trackmap-start-finish" />
              </g>
            );
          })()}

          {/* Pit Entry & Exit Markers */}
          {effectiveMap.pitEntry && (() => {
            const [u, v] = applyTransform(transform, effectiveMap.pitEntry.x, effectiveMap.pitEntry.z);
            return (
              <g data-testid="pit-entry" className="trackmap-pit-marker">
                <title>Pit entry</title>
                <circle cx={u} cy={v} r={3} className="trackmap-pit-dot" />
                <text x={u} y={v - 6} className="trackmap-sector-text trackmap-pit-text">
                  IN
                </text>
              </g>
            );
          })()}

          {effectiveMap.pitExit && (() => {
            const [u, v] = applyTransform(transform, effectiveMap.pitExit.x, effectiveMap.pitExit.z);
            return (
              <g data-testid="pit-exit" className="trackmap-pit-marker">
                <title>Pit exit</title>
                <circle cx={u} cy={v} r={3} className="trackmap-pit-dot" />
                <text x={u} y={v - 6} className="trackmap-sector-text trackmap-pit-text">
                  OUT
                </text>
              </g>
            );
          })()}

          {/* Competitor Markers: number only on the dot, full vehicle + driver name in the tooltip (U10/V04) */}
          {competitorMarkers.map(({ u, v, competitor }) => {
            const color = classToColor(competitor.vehicleClass);
            const label = carMarkerLabel(competitor.carNumber);
            const tooltip = markerTooltip(competitor.carNumber, competitor.driverName);
            return (
              <g
                key={`car-${competitor.id}`}
                className={`trackmap-competitor-group ${competitor.inPits ? 'in-pits' : ''} ${isStale ? 'stale' : ''}`}
                data-testid={`competitor-${label}`}
              >
                {tooltip !== '' && <title>{tooltip}</title>}
                <circle
                  cx={u}
                  cy={v}
                  r={10}
                  fill={isStale ? 'transparent' : color}
                  stroke="var(--surface-app, #0b0d10)"
                  strokeWidth={1.5}
                  className={`trackmap-car-competitor ${competitor.inPits ? 'in-pits' : ''}`}
                />
                <text
                  x={u}
                  y={v}
                  className="trackmap-car-label"
                  fill={isStale ? 'var(--dq-stale-value, #8b95a3)' : 'var(--text-inverse, #0b0d10)'}
                >
                  {label}
                </text>
              </g>
            );
          })}

          {/* Driver Vehicle Marker */}
          {driverScreenPos && (
            <g data-testid="driver-marker">
              {driverVehicleName && <title>{markerTooltip(driverVehicleName, null)}</title>}
              <circle
                cx={driverScreenPos[0]}
                cy={driverScreenPos[1]}
                r={15}
                className="trackmap-car-halo"
              />
              <circle
                cx={driverScreenPos[0]}
                cy={driverScreenPos[1]}
                r={10}
                className={`trackmap-car-driver ${driverPosition?.state === 'stale' || isStale ? 'stale' : ''}`}
              />
              {driverCarNumber && (
                <text
                  x={driverScreenPos[0]}
                  y={driverScreenPos[1]}
                  className="trackmap-car-label trackmap-driver-label"
                >
                  {driverCarNumber}
                </text>
              )}
              {driverPosition?.headingRad !== null && driverPosition?.headingRad !== undefined && (
                <line
                  x1={driverScreenPos[0]}
                  y1={driverScreenPos[1]}
                  x2={driverScreenPos[0] + 15 * Math.sin(driverPosition.headingRad - rotationRad)}
                  y2={driverScreenPos[1] - 15 * Math.cos(driverPosition.headingRad - rotationRad)}
                  className="trackmap-car-heading"
                  strokeWidth={2}
                  strokeLinecap="round"
                />
              )}
            </g>
          )}
        </svg>
      ) : (
        /* Fallback HUD: LapDistanceBar whenever vector geometry is not yet available */
        <LapDistanceBar
          trackLengthM={displayLengthM ?? 5000}
          driverPosition={driverPosition}
          competitors={competitors}
          sectors={effectiveMap?.sectors}
          dq={dq}
          progress={effectiveProgress}
          summary={ariaSummary}
        />
      )}
    </div>
  );
}
