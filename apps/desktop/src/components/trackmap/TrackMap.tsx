/**
 * Live Track Map Component (spec §23–24, §79, §155–157, §342).
 * Renders circuit vector geometry, sectors, pit marks, competitor positions, driver vehicle marker,
 * or automatic online reconstructed track geometry and LapDistanceBar fallback.
 */
import { useContext, useEffect, useRef, useState, type JSX } from 'react';
import { useScoring } from '../../hooks/useScoring';
import { TelemetryStoreContext } from '../../telemetry/hooks';
import { LapDistanceBar } from './LapDistanceBar';
import { buildSvgPath, classToColor, positionAt } from './trackGeometry';
import { applyTransform, computeBounds, fitMapTransform } from './transform';
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
  /** Additional container CSS class. */
  readonly className?: string;
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
  const rawDisplayName = effectiveMap?.trackId ?? trackName ?? null;
  const displayName =
    rawDisplayName && rawDisplayName.trim() !== '' && rawDisplayName !== '—' && rawDisplayName !== 'Unknown Track'
      ? rawDisplayName.trim()
      : null;
  const displayLengthM = effectiveMap?.lengthM ?? trackLengthM ?? null;

  const isStale = dq === 'stale';
  const isEnded = dq === 'ended';

  // Compute transform when dimensions and map are valid
  const transform = (() => {
    if (!hasMapPoints || dimensions.width <= 0 || dimensions.height <= 0) {
      return null;
    }
    const bounds = computeBounds(effectiveMap.points);
    if (!bounds) {
      return null;
    }
    return fitMapTransform(bounds, effectiveMap.points, dimensions.width, dimensions.height, 36, rotationRad);
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
        {onRotationChange && hasMapPoints && (
          <div className="trackmap-controls">
            <button
              type="button"
              className="btn btn-sm"
              title="Rotate counter-clockwise 15°"
              onClick={() => {
                handleRotate(Math.PI / 12);
              }}
            >
              ↺
            </button>
            <button
              type="button"
              className="btn btn-sm"
              title="Rotate clockwise 15°"
              onClick={() => {
                handleRotate(-Math.PI / 12);
              }}
            >
              ↻
            </button>
            <button
              type="button"
              className="btn btn-sm"
              title="Reset rotation"
              onClick={() => {
                onRotationChange(0);
              }}
            >
              0°
            </button>
          </div>
        )}
      </div>

      {transform && effectiveMap ? (
        <svg
          className="trackmap-svg"
          viewBox={`0 0 ${dimensions.width} ${dimensions.height}`}
          data-testid="trackmap-svg"
        >
          {/* Circuit polyline base and groove */}
          <path d={buildSvgPath(effectiveMap.points, transform)} className="trackmap-path-base" />
          <path d={buildSvgPath(effectiveMap.points, transform)} className="trackmap-path-inner" />

          {/* Sector Marks */}
          {effectiveMap.sectors.map((sector) => {
            const [u, v] = applyTransform(transform, sector.x, sector.z);
            return (
              <g key={`sector-${sector.index}`} data-testid={`sector-${sector.index}`}>
                <circle cx={u} cy={v} r={3} fill="#7a8492" />
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
                <circle cx={u} cy={v} r={4} fill="#ffffff" />
                <line x1={u - 6} y1={v} x2={u + 6} y2={v} className="trackmap-start-finish" />
              </g>
            );
          })()}

          {/* Pit Entry & Exit Markers */}
          {effectiveMap.pitEntry && (() => {
            const [u, v] = applyTransform(transform, effectiveMap.pitEntry.x, effectiveMap.pitEntry.z);
            return (
              <g data-testid="pit-entry">
                <circle cx={u} cy={v} r={3} fill="#f5b83d" />
                <text x={u} y={v - 6} className="trackmap-sector-text" fill="#f5b83d">
                  IN
                </text>
              </g>
            );
          })()}

          {effectiveMap.pitExit && (() => {
            const [u, v] = applyTransform(transform, effectiveMap.pitExit.x, effectiveMap.pitExit.z);
            return (
              <g data-testid="pit-exit">
                <circle cx={u} cy={v} r={3} fill="#4c9dff" />
                <text x={u} y={v - 6} className="trackmap-sector-text" fill="#4c9dff">
                  OUT
                </text>
              </g>
            );
          })()}

          {/* Competitor Markers */}
          {competitorMarkers.map(({ u, v, competitor }) => {
            const color = classToColor(competitor.vehicleClass);
            return (
              <g
                key={`car-${competitor.id}`}
                className={`trackmap-competitor-group ${competitor.inPits ? 'in-pits' : ''} ${isStale ? 'stale' : ''}`}
                data-testid={`competitor-${competitor.carNumber}`}
              >
                <circle
                  cx={u}
                  cy={v}
                  r={5}
                  fill={isStale ? 'transparent' : color}
                  stroke={color}
                  className={`trackmap-car-competitor ${competitor.inPits ? 'in-pits' : ''}`}
                />
                <text x={u} y={v} className="trackmap-car-label">
                  {competitor.carNumber}
                </text>
              </g>
            );
          })}

          {/* Driver Vehicle Marker */}
          {driverScreenPos && (
            <g data-testid="driver-marker">
              <circle
                cx={driverScreenPos[0]}
                cy={driverScreenPos[1]}
                r={10}
                className="trackmap-car-halo"
              />
              <circle
                cx={driverScreenPos[0]}
                cy={driverScreenPos[1]}
                r={6}
                className={`trackmap-car-driver ${driverPosition?.state === 'stale' || isStale ? 'stale' : ''}`}
              />
              {driverPosition?.headingRad !== null && driverPosition?.headingRad !== undefined && (
                <line
                  x1={driverScreenPos[0]}
                  y1={driverScreenPos[1]}
                  x2={driverScreenPos[0] + 12 * Math.sin(driverPosition.headingRad - rotationRad)}
                  y2={driverScreenPos[1] - 12 * Math.cos(driverPosition.headingRad - rotationRad)}
                  stroke="#ffffff"
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
        />
      )}
    </div>
  );
}
