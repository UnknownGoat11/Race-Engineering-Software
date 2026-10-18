/**
 * Telemetry graph worksheets and stacked panes (REDESIGN.md §4 W5.1, closing G02, G03, G04, T14).
 *
 * Each worksheet organizes telemetry into stacked panes with dedicated vertical proportions,
 * allowing synchronized comparison across domains (Driver, Tyres, Brakes, Energy, Powertrain).
 */
import { ch } from '../generated/channels';
import type { GraphSeries } from './types';

/** Standard worksheet identifiers. */
export type StandardWorksheetId = 'driver' | 'tyres' | 'brakes' | 'energy' | 'powertrain';

/** All worksheet identifiers including user-customized. */
export type WorksheetId = StandardWorksheetId | 'custom';

/** One pane in a stacked worksheet. */
export interface WorksheetPane {
  /** Stable identifier for the pane within the worksheet. */
  readonly id: string;
  /** Human-readable title of the pane. */
  readonly title: string;
  /** Vertical proportion of the total plot height (percentage, e.g. 35 for 35%). */
  readonly proportion: number;
  /** Series plotted in this pane. */
  readonly series: readonly GraphSeries[];
}

/** A collection of stacked panes forming a cohesive telemetry view. */
export interface Worksheet {
  readonly id: WorksheetId;
  readonly label: string;
  readonly panes: readonly WorksheetPane[];
}

/** Standard worksheets per REDESIGN.md §4 W5.1. */
export const WORKSHEETS: Readonly<Record<StandardWorksheetId, Worksheet>> = {
  driver: {
    id: 'driver',
    label: 'Driver',
    panes: [
      {
        id: 'speed',
        title: 'Speed',
        proportion: 35,
        series: [{ channelId: ch.SPEED, color: 1, enabled: true }],
      },
      {
        id: 'delta',
        title: 'Delta',
        proportion: 15,
        series: [{ channelId: ch.DELTA_BEST, color: 2, enabled: true }],
      },
      {
        id: 'inputs',
        title: 'Throttle & Brake',
        proportion: 20,
        series: [
          { channelId: ch.THROTTLE, color: 3, enabled: true },
          { channelId: ch.BRAKE, color: 4, enabled: true },
        ],
      },
      {
        id: 'engine',
        title: 'Gear & RPM',
        proportion: 15,
        series: [
          { channelId: ch.GEAR, color: 5, enabled: true },
          { channelId: ch.RPM, color: 6, enabled: true },
        ],
      },
      {
        id: 'steering',
        title: 'Steering',
        proportion: 15,
        series: [{ channelId: ch.STEERING_ANGLE, color: 7, enabled: true }],
      },
    ],
  },
  tyres: {
    id: 'tyres',
    label: 'Tyres',
    panes: [
      {
        id: 'tyre-temps',
        title: 'Carcass & Optimal Window',
        proportion: 60,
        series: [
          { channelId: ch.TYRE_CARCASS_TEMP_FL, color: 1, enabled: true },
          { channelId: ch.TYRE_CARCASS_TEMP_FR, color: 2, enabled: true },
          { channelId: ch.TYRE_CARCASS_TEMP_RL, color: 3, enabled: true },
          { channelId: ch.TYRE_CARCASS_TEMP_RR, color: 4, enabled: true },
          { channelId: ch.TYRE_OPTIMAL_TEMP_FL, color: 5, enabled: true },
          { channelId: ch.TYRE_OPTIMAL_TEMP_FR, color: 6, enabled: true },
          { channelId: ch.TYRE_OPTIMAL_TEMP_RL, color: 7, enabled: true },
          { channelId: ch.TYRE_OPTIMAL_TEMP_RR, color: 8, enabled: true },
        ],
      },
      {
        id: 'tyre-pressures',
        title: 'Pressures',
        proportion: 40,
        series: [
          { channelId: ch.TYRE_PRESSURE_FL, color: 1, enabled: true },
          { channelId: ch.TYRE_PRESSURE_FR, color: 2, enabled: true },
          { channelId: ch.TYRE_PRESSURE_RL, color: 3, enabled: true },
          { channelId: ch.TYRE_PRESSURE_RR, color: 4, enabled: true },
        ],
      },
    ],
  },
  brakes: {
    id: 'brakes',
    label: 'Brakes',
    panes: [
      {
        id: 'brake-temps',
        title: 'Disc Temperatures',
        proportion: 55,
        series: [
          { channelId: ch.BRAKE_TEMP_FL, color: 1, enabled: true },
          { channelId: ch.BRAKE_TEMP_FR, color: 2, enabled: true },
          { channelId: ch.BRAKE_TEMP_RL, color: 3, enabled: true },
          { channelId: ch.BRAKE_TEMP_RR, color: 4, enabled: true },
        ],
      },
      {
        id: 'brake-control',
        title: 'Pedal, Bias & ABS',
        proportion: 45,
        series: [
          { channelId: ch.BRAKE, color: 1, enabled: true },
          { channelId: ch.BRAKE_BIAS_FRONT, color: 2, enabled: true },
          { channelId: ch.ABS_ACTIVE, color: 3, enabled: true },
        ],
      },
    ],
  },
  energy: {
    id: 'energy',
    label: 'Energy',
    panes: [
      {
        id: 'energy-state',
        title: 'Virtual Energy & SoC',
        proportion: 50,
        series: [
          { channelId: ch.VIRTUAL_ENERGY, color: 1, enabled: true },
          { channelId: ch.BATTERY_SOC, color: 2, enabled: true },
        ],
      },
      {
        id: 'energy-deployment',
        title: 'Throttle & Motor Torque',
        proportion: 50,
        series: [
          { channelId: ch.THROTTLE, color: 3, enabled: true },
          { channelId: ch.HYBRID_MOTOR_TORQUE, color: 4, enabled: true },
        ],
      },
    ],
  },
  powertrain: {
    id: 'powertrain',
    label: 'Powertrain',
    panes: [
      {
        id: 'fluid-temps',
        title: 'Water & Oil Temperatures',
        proportion: 40,
        series: [
          { channelId: ch.WATER_TEMP, color: 1, enabled: true },
          { channelId: ch.OIL_TEMP, color: 2, enabled: true },
        ],
      },
      {
        id: 'engine-speed',
        title: 'RPM & Speed',
        proportion: 35,
        series: [
          { channelId: ch.RPM, color: 3, enabled: true },
          { channelId: ch.SPEED, color: 4, enabled: true },
        ],
      },
      {
        id: 'gear-throttle',
        title: 'Gear & Throttle',
        proportion: 25,
        series: [
          { channelId: ch.GEAR, color: 5, enabled: true },
          { channelId: ch.THROTTLE, color: 6, enabled: true },
        ],
      },
    ],
  },
};

/** Default active worksheet ID. */
export const DEFAULT_WORKSHEET_ID: WorksheetId = 'driver';

/** Retrieves a worksheet definition, synthesizing custom worksheets when requested. */
export function getWorksheet(id: WorksheetId, customSeries?: readonly GraphSeries[]): Worksheet {
  if (id !== 'custom') {
    return WORKSHEETS[id];
  }
  const series = customSeries && customSeries.length > 0 ? customSeries : [{ channelId: ch.SPEED, color: 1, enabled: true }];
  return {
    id: 'custom',
    label: 'Custom',
    panes: [
      {
        id: 'custom-pane',
        title: 'Custom Channels',
        proportion: 100,
        series,
      },
    ],
  };
}

/** Gathers all distinct channel IDs across all panes in a worksheet. */
export function worksheetAllChannels(worksheet: Worksheet): readonly number[] {
  const set = new Set<number>();
  for (const pane of worksheet.panes) {
    for (const s of pane.series) {
      set.add(s.channelId);
    }
  }
  return Array.from(set);
}

/** Gathers all series across all panes in a worksheet. */
export function worksheetAllSeries(worksheet: Worksheet): readonly GraphSeries[] {
  const list: GraphSeries[] = [];
  const seen = new Set<number>();
  for (const pane of worksheet.panes) {
    for (const s of pane.series) {
      if (!seen.has(s.channelId)) {
        seen.add(s.channelId);
        list.push(s);
      }
    }
  }
  return list;
}
