import type { JSX } from 'react';
import { formatPressure, formatTemperature, NO_VALUE } from '../../lib/units';
import type { WheelReadings, WheelSet } from '../types';
import { getBrakeDiscColor, getTyreTempColor } from '../tyreColors';

export interface TyreCarTopViewProps {
  readonly wheels: WheelSet;
  /** Engine water temperature, °C (WATER_TEMP); null when not reported. */
  readonly engineWaterTempC: number | null;
  /** Engine oil temperature, °C (OIL_TEMP); null when not reported. */
  readonly engineOilTempC: number | null;
  /** Clutch speed, rpm (CLUTCH_RPM); null when not reported. */
  readonly clutchRpm: number | null;
  readonly className?: string | undefined;
}

interface SingleWheelProps {
  readonly position: 'FL' | 'FR' | 'RL' | 'RR';
  readonly label: string;
  readonly data: WheelReadings;
}

/** Neutral styling for a tyre zone without a live temperature. */
const NO_DATA_ZONE = { bg: 'transparent', text: 'inherit', label: 'NO DATA' } as const;

function zoneColor(tempC: number | null): { readonly bg: string; readonly text: string; readonly label: string } {
  return tempC === null ? NO_DATA_ZONE : getTyreTempColor(tempC);
}

function deg(tempC: number | null): string {
  const t = formatTemperature(tempC);
  return t === NO_VALUE ? NO_VALUE : `${t}°`;
}

function TyreZone({
  className,
  name,
  tempC,
  extra,
}: {
  readonly className: string;
  readonly name: string;
  readonly tempC: number | null;
  readonly extra?: JSX.Element;
}): JSX.Element {
  const color = zoneColor(tempC);
  return (
    <div
      className={`tyre-part ${className}`}
      style={{ backgroundColor: color.bg, color: color.text }}
      title={`${name}: ${formatTemperature(tempC, { withUnit: true })} (${color.label})`}
    >
      <span className="tyre-part-temp">{deg(tempC)}</span>
      {extra}
    </div>
  );
}

function BrakeDisc({ data, side }: { readonly data: WheelReadings; readonly side: 'left' | 'right' }): JSX.Element {
  const brakeC = data.brakeC;
  return (
    <div
      className={`brake-disc-widget brake-disc-${side}`}
      title={`Brake: ${formatTemperature(brakeC, { withUnit: true })} (Peak: ${formatTemperature(data.brakePeakC, { withUnit: true })})`}
    >
      {brakeC !== null && (
        <div
          className="brake-disc-bar"
          style={{ backgroundColor: getBrakeDiscColor(brakeC), height: `${Math.max(0, Math.min(100, (brakeC / 800) * 100))}%` }}
        />
      )}
      <span className="brake-disc-num">{formatTemperature(brakeC, { withUnit: true })}</span>
    </div>
  );
}

function SingleWheelTopView({ position, label, data }: SingleWheelProps): JSX.Element {
  const isLeft = position === 'FL' || position === 'RL';

  return (
    <div className={`tyre-wheel-card tyre-wheel-${position.toLowerCase()}`} data-testid={`wheel-${position.toLowerCase()}`}>
      <div className="tyre-wheel-header">
        <span className="tyre-pos-badge">{label}</span>
      </div>

      <div className="tyre-wheel-layout">
        {/* Brake disc sits on the inboard side of the wheel. */}
        {!isLeft && <BrakeDisc data={data} side="left" />}

        {/* Top view of the tyre split into 3 surface zones: left edge, middle, right edge (car frame). */}
        <div
          className="tyre-topview-rect"
          role="img"
          aria-label={`${label} tyre surface temperatures: left ${formatTemperature(data.leftC, { withUnit: true })}, middle ${formatTemperature(data.middleC, { withUnit: true })}, right ${formatTemperature(data.rightC, { withUnit: true })}`}
        >
          <TyreZone className="tyre-part-left" name="Left edge" tempC={data.leftC} />
          <TyreZone
            className="tyre-part-core"
            name="Middle"
            tempC={data.middleC}
            extra={<span className="tyre-core-label">MID</span>}
          />
          <TyreZone className="tyre-part-right" name="Right edge" tempC={data.rightC} />
        </div>

        {isLeft && <BrakeDisc data={data} side="right" />}
      </div>

      <div className="tyre-metrics-grid">
        <div className="tyre-metric-compact-row">
          <span className="tyre-metric-item" title="Carcass temperature">
            C: <strong>{deg(data.carcassC)}</strong>
          </span>
          <span className="tyre-metric-item" title="Mean surface temperature">
            S: <strong>{deg(data.surfaceAvgC)}</strong>
          </span>
        </div>
        <div className="tyre-metric-compact-row">
          <span className="tyre-metric-item num-cyan" title="Tyre pressure">
            {formatPressure(data.pressurePa, { withUnit: true })}
          </span>
          <span className="tyre-metric-item" title="Tyre wear as reported by LMU (raw value)">
            Wear (raw): {data.wearRaw !== null ? data.wearRaw.toFixed(2) : NO_VALUE}
          </span>
        </div>
      </div>
    </div>
  );
}

export function TyreCarTopView({
  wheels,
  engineWaterTempC,
  engineOilTempC,
  clutchRpm,
  className = '',
}: TyreCarTopViewProps): JSX.Element {
  return (
    <div className={`tyre-car-topview-container ${className}`} data-testid="tyre-car-topview">
      <div className="tyre-legend-bar">
        <span className="tyre-legend-title">Tyre Heat Bands:</span>
        <span className="tyre-legend-chip chip-dark-blue">Dark Blue &lt;65° (Cold)</span>
        <span className="tyre-legend-chip chip-light-blue">Light Blue 65-75° (Slight Cold)</span>
        <span className="tyre-legend-chip chip-green">Green 75-95° (Optimal)</span>
        <span className="tyre-legend-chip chip-light-orange">Light Orange 95-105° (Getting Warm)</span>
        <span className="tyre-legend-chip chip-orange">Orange 105-115° (Warm)</span>
        <span className="tyre-legend-chip chip-red">Red &gt;115° (Hot)</span>
      </div>

      <div className="tyre-chassis-grid">
        <div className="tyre-axle-row">
          <SingleWheelTopView position="FL" label="Front Left" data={wheels.fl} />
          <div className="chassis-front-nose">
            <div className="chassis-wireframe-nose" />
          </div>
          <SingleWheelTopView position="FR" label="Front Right" data={wheels.fr} />
        </div>

        <div className="tyre-center-row">
          <div className="engine-thermals-block" data-testid="engine-thermals-block">
            <div className="engine-block-header">
              <span className="engine-block-title">ENGINE TEMPS</span>
            </div>
            <div className="engine-block-body">
              <div className="engine-sub-item">
                <span className="engine-sub-label">Oil Temp:</span>
                <span className="engine-sub-val" data-testid="engine-oil-temp">
                  {formatTemperature(engineOilTempC, { withUnit: true })}
                </span>
              </div>
              <div className="engine-sub-item">
                <span className="engine-sub-label">Water Temp:</span>
                <span className="engine-sub-val" data-testid="engine-water-temp">
                  {formatTemperature(engineWaterTempC, { withUnit: true })}
                </span>
              </div>
              <div className="engine-sub-item">
                <span className="engine-sub-label">Clutch RPM:</span>
                <span className="engine-sub-val num-cyan">
                  {clutchRpm !== null ? Math.round(clutchRpm).toString() : NO_VALUE}
                </span>
              </div>
            </div>
          </div>
        </div>

        <div className="tyre-axle-row">
          <SingleWheelTopView position="RL" label="Rear Left" data={wheels.rl} />
          <div className="chassis-rear-diff">
            <div className="chassis-wireframe-rear" />
          </div>
          <SingleWheelTopView position="RR" label="Rear Right" data={wheels.rr} />
        </div>
      </div>
    </div>
  );
}
