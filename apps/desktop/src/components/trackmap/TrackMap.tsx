/**
 * Live Track Map Component (spec §23–24, §79, §155–157, §342).
 * Renders circuit vector geometry, sectors, pit marks, competitor positions and the driver vehicle marker.
 */
import { useEffect, useRef, useState, type JSX } from 'react';
import { applyTransform, computeBounds, fitMapTransform } from './transform';
import { buildSvgPath, classToColor, positionAt } from './trackGeometry';
import type { CompetitorMarker, DriverPosition, TrackMap as TrackMapData } from './types';
import './trackmap.css';

export interface TrackMapProps {
  /** Static or dynamic track map geometry. */
  readonly map?: TrackMapData | null;
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

  const hasMapPoints = map !== undefined && map !== null && map.points.length >= 2;
  const displayName = map?.trackId ?? trackName ?? 'Unknown Track';
  const displayLengthM = map?.lengthM ?? trackLengthM ?? null;

  // Compute transform when dimensions and map are valid
  const transform = (() => {
    if (!hasMapPoints || dimensions.width <= 0 || dimensions.height <= 0) {
      return null;
    }
    const bounds = computeBounds(map.points);
    if (!bounds) {
      return null;
    }
    return fitMapTransform(bounds, map.points, dimensions.width, dimensions.height, 36, rotationRad);
  })();

  // Driver marker coordinates
  const driverScreenPos = (() => {
    if (!transform || !driverPosition) return null;
    let wx = driverPosition.x;
    let wz = driverPosition.z;

    if ((wx === null || wz === null || !Number.isFinite(wx) || !Number.isFinite(wz)) && map) {
      if (driverPosition.lapDistM !== null && driverPosition.lapDistM !== undefined) {
        const pt = positionAt(map, driverPosition.lapDistM);
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

      if ((wx === null || wz === null || !Number.isFinite(wx) || !Number.isFinite(wz)) && map) {
        if (c.lapDistM !== null && c.lapDistM !== undefined) {
          const pt = positionAt(map, c.lapDistM);
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

  return (
    <div className={`trackmap-container ${className}`} ref={containerRef} data-testid="trackmap-container">
      <div className="trackmap-header">
        <div className="trackmap-title-box">
          <span className="trackmap-track-name" data-testid="trackmap-title">
            {displayName}
          </span>
          {displayLengthM !== null && (
            <span className="trackmap-track-length" data-testid="trackmap-length">
              {(displayLengthM / 1000).toFixed(3)} km
            </span>
          )}
        </div>
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

      {transform && map ? (
        <svg
          className="trackmap-svg"
          viewBox={`0 0 ${dimensions.width} ${dimensions.height}`}
          data-testid="trackmap-svg"
        >
          {/* Circuit polyline base and groove */}
          <path d={buildSvgPath(map.points, transform)} className="trackmap-path-base" />
          <path d={buildSvgPath(map.points, transform)} className="trackmap-path-inner" />

          {/* Sector Marks */}
          {map.sectors.map((sector) => {
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
            const [u, v] = applyTransform(transform, map.startFinish.x, map.startFinish.z);
            return (
              <g data-testid="start-finish-line">
                <circle cx={u} cy={v} r={4} fill="#ffffff" />
                <line x1={u - 6} y1={v} x2={u + 6} y2={v} className="trackmap-start-finish" />
              </g>
            );
          })()}

          {/* Pit Entry & Exit Markers */}
          {map.pitEntry && (() => {
            const [u, v] = applyTransform(transform, map.pitEntry.x, map.pitEntry.z);
            return (
              <g data-testid="pit-entry">
                <circle cx={u} cy={v} r={3} fill="#f5b83d" />
                <text x={u} y={v - 6} className="trackmap-sector-text" fill="#f5b83d">
                  IN
                </text>
              </g>
            );
          })()}

          {map.pitExit && (() => {
            const [u, v] = applyTransform(transform, map.pitExit.x, map.pitExit.z);
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
                className={`trackmap-competitor-group ${competitor.inPits ? 'in-pits' : ''}`}
                data-testid={`competitor-${competitor.carNumber}`}
              >
                <circle
                  cx={u}
                  cy={v}
                  r={5}
                  fill={color}
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
                className={`trackmap-car-driver ${driverPosition?.state === 'stale' ? 'stale' : ''}`}
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
        /* Fallback HUD when full track map geometry is unavailable */
        <div className="trackmap-fallback" data-testid="trackmap-fallback">
          <p className="trackmap-fallback-message">
            {hasMapPoints ? 'Awaiting display dimensions...' : 'Circuit geometry not loaded.'}
          </p>
          {driverPosition?.lapDistM !== null && driverPosition?.lapDistM !== undefined && displayLengthM !== null && displayLengthM > 0 && (
            <div className="trackmap-progress-bar-container">
              <div
                className="trackmap-progress-fill"
                style={{
                  width: `${Math.min(100, Math.max(0, (driverPosition.lapDistM / displayLengthM) * 100))}%`,
                }}
              />
              <div className="trackmap-progress-label">
                Lap Progress: {Math.round(driverPosition.lapDistM)} m / {Math.round(displayLengthM)} m
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
