import type { JSX } from 'react';
import type { LockupEvent, TyreSectionTemperatures } from '../types';
import { getBrakeDiscColor, getTyreTempColor } from '../tyreColors';
import { TyreDegQuickSummary } from './TyreDegQuickSummary';

export interface TyreCarTopViewProps {
  readonly wheels: {
    readonly fl: TyreSectionTemperatures;
    readonly fr: TyreSectionTemperatures;
    readonly rl: TyreSectionTemperatures;
    readonly rr: TyreSectionTemperatures;
  };
  readonly engineWaterTempC?: number | undefined;
  readonly engineOilTempC?: number | undefined;
  readonly clutchRpm?: number | undefined;
  readonly lockupEvents?: readonly LockupEvent[] | undefined;
  readonly showQuickSummary?: boolean | undefined;
  readonly className?: string | undefined;
}

interface SingleWheelProps {
  readonly position: 'FL' | 'FR' | 'RL' | 'RR';
  readonly label: string;
  readonly data: TyreSectionTemperatures;
}

function SingleWheelTopView({ position, label, data }: SingleWheelProps): JSX.Element {
  const leftColor = getTyreTempColor(data.outerLeftC);
  const coreColor = getTyreTempColor(data.coreC);
  const rightColor = getTyreTempColor(data.outerRightC);
  const brakeColor = getBrakeDiscColor(data.brakeDiscTempC);
  const isLeft = position === 'FL' || position === 'RL';

  return (
    <div className={`tyre-wheel-card tyre-wheel-${position.toLowerCase()}`} data-testid={`wheel-${position.toLowerCase()}`}>
      <div className="tyre-wheel-header">
        <span className="tyre-pos-badge">{label}</span>
        {data.lockupActive && <span className="tyre-lockup-pill">LOCKUP</span>}
        {(data.flatspotPercent ?? 0) > 0 && (
          <span className="tyre-flatspot-pill">⚠️ {data.flatspotPercent}% FLATSPOT</span>
        )}
        {data.slipPercentage > 12 && <span className="tyre-slip-pill">SLIP</span>}
      </div>

      <div className="tyre-wheel-layout">
        {/* If left wheel, brake disc is placed on the inner right side; if right wheel, on inner left side */}
        {!isLeft && (
          <div className="brake-disc-widget brake-disc-left" title={`Brake: ${data.brakeDiscTempC.toFixed(0)}°C (Peak: ${data.brakePeakTempC.toFixed(0)}°C)`}>
            <div className="brake-disc-bar" style={{ backgroundColor: brakeColor, height: `${Math.min(100, (data.brakeDiscTempC / 800) * 100)}%` }} />
            <span className="brake-disc-num">{data.brakeDiscTempC.toFixed(0)}°C</span>
          </div>
        )}

        {/* The 4-wheel top view: almost rectangle split into 3 vertical parts: outer left, core, outer right */}
        <div className="tyre-topview-rect" role="img" aria-label={`${label} Tyre temperatures: Outer Left ${data.outerLeftC.toFixed(1)}°C, Core ${data.coreC.toFixed(1)}°C, Outer Right ${data.outerRightC.toFixed(1)}°C`}>
          {/* Part 1: Outer Left */}
          <div
            className="tyre-part tyre-part-left"
            style={{ backgroundColor: leftColor.bg, color: leftColor.text }}
            title={`Outer Left: ${data.outerLeftC.toFixed(1)}°C (${leftColor.label})`}
          >
            <span className="tyre-part-temp">{data.outerLeftC.toFixed(0)}°</span>
          </div>

          {/* Part 2: Core (Middle) */}
          <div
            className="tyre-part tyre-part-core"
            style={{ backgroundColor: coreColor.bg, color: coreColor.text }}
            title={`Core: ${data.coreC.toFixed(1)}°C (${coreColor.label})`}
          >
            <span className="tyre-part-temp">{data.coreC.toFixed(0)}°</span>
            <span className="tyre-core-label">CORE</span>
          </div>

          {/* Part 3: Outer Right */}
          <div
            className="tyre-part tyre-part-right"
            style={{ backgroundColor: rightColor.bg, color: rightColor.text }}
            title={`Outer Right: ${data.outerRightC.toFixed(1)}°C (${rightColor.label})`}
          >
            <span className="tyre-part-temp">{data.outerRightC.toFixed(0)}°</span>
          </div>
        </div>

        {isLeft && (
          <div className="brake-disc-widget brake-disc-right" title={`Brake: ${data.brakeDiscTempC.toFixed(0)}°C (Peak: ${data.brakePeakTempC.toFixed(0)}°C)`}>
            <div className="brake-disc-bar" style={{ backgroundColor: brakeColor, height: `${Math.min(100, (data.brakeDiscTempC / 800) * 100)}%` }} />
            <span className="brake-disc-num">{data.brakeDiscTempC.toFixed(0)}°C</span>
          </div>
        )}
      </div>

      {/* Numeric Telemetry Breakdown (Compact 2x2 Grid) */}
      <div className="tyre-metrics-grid">
        <div className="tyre-metric-compact-row">
          <span className="tyre-metric-item" title="Carcass Temperature">C: <strong>{data.carcassC.toFixed(0)}°</strong></span>
          <span className="tyre-metric-item" title="Surface Temperature">S: <strong>{data.surfaceC.toFixed(0)}°</strong></span>
        </div>
        <div className="tyre-metric-compact-row">
          <span className="tyre-metric-item num-cyan" title="Tyre Pressure">{data.pressureKpa.toFixed(1)} kPa</span>
          <span className="tyre-metric-item" title="Tyre Tread Remaining">{(data.wearFraction * 100).toFixed(1)}%</span>
        </div>
        <div className="tyre-wear-bar-track">
          <div
            className="tyre-wear-bar-fill"
            style={{
              width: `${Math.max(0, Math.min(100, data.wearFraction * 100))}%`,
              backgroundColor: data.wearFraction < 0.3 ? '#ef4444' : data.wearFraction < 0.6 ? '#f59e0b' : '#10b981',
            }}
          />
        </div>
      </div>
    </div>
  );
}

export function TyreCarTopView({
  wheels,
  engineWaterTempC = 88,
  engineOilTempC = 96,
  clutchRpm = 0,
  lockupEvents,
  showQuickSummary = true,
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
        {/* Front Axle */}
        <div className="tyre-axle-row">
          <SingleWheelTopView position="FL" label="Front Left" data={wheels.fl} />
          <div className="chassis-front-nose">
            <span className="chassis-aero-label">AERO BALANCE: 44.2%</span>
            <div className="chassis-wireframe-nose" />
          </div>
          <SingleWheelTopView position="FR" label="Front Right" data={wheels.fr} />
        </div>

        {/* Center Drivetrain / Engine Block Graphic per image 2 */}
        <div className="tyre-center-row">
          <div className="engine-thermals-block" data-testid="engine-thermals-block">
            <div className="engine-block-header">
              <span className="engine-block-title">ENGINE TEMPS</span>
            </div>
            <div className="engine-block-body">
              <div className="engine-sub-item">
                <span className="engine-sub-label">Oil Temp:</span>
                <span className={`engine-sub-val ${engineOilTempC > 115 ? 'temp-warn' : 'temp-ok'}`}>
                  {engineOilTempC.toFixed(1)}°C
                </span>
              </div>
              <div className="engine-sub-item">
                <span className="engine-sub-label">Water Temp:</span>
                <span className={`engine-sub-val ${engineWaterTempC > 105 ? 'temp-warn' : 'temp-ok'}`}>
                  {engineWaterTempC.toFixed(1)}°C
                </span>
              </div>
              <div className="engine-sub-item">
                <span className="engine-sub-label">Clutch RPM:</span>
                <span className="engine-sub-val num-cyan">{clutchRpm.toFixed(0)}</span>
              </div>
            </div>
          </div>
        </div>

        {/* Rear Axle */}
        <div className="tyre-axle-row">
          <SingleWheelTopView position="RL" label="Rear Left" data={wheels.rl} />
          <div className="chassis-rear-diff">
            <span className="chassis-diff-label">DIFF LOCK: 68%</span>
            <div className="chassis-wireframe-rear" />
          </div>
          <SingleWheelTopView position="RR" label="Rear Right" data={wheels.rr} />
        </div>
      </div>

      {showQuickSummary && (
        <div className="tyre-chassis-summary-strip">
          <TyreDegQuickSummary wheels={wheels} lockupEvents={lockupEvents} compact />
        </div>
      )}
    </div>
  );
}
