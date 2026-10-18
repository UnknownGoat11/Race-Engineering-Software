import { describe, expect, it } from 'vitest';
import { ch } from '../generated/channels';
import { channelDef } from '../telemetry/registry';
import {
  DEFAULT_WORKSHEET_ID,
  WORKSHEETS,
  getWorksheet,
  worksheetAllChannels,
  worksheetAllSeries,
  type StandardWorksheetId,
} from './worksheets';

describe('worksheets definitions (REDESIGN.md §4 W5.1)', () => {
  const standardIds: StandardWorksheetId[] = ['driver', 'tyres', 'brakes', 'energy', 'powertrain'];

  it('verifies that every channel in all worksheets exists in the registry', () => {
    for (const id of standardIds) {
      const ws = WORKSHEETS[id];
      for (const pane of ws.panes) {
        for (const s of pane.series) {
          const def = channelDef(s.channelId);
          expect(def, `Channel 0x${s.channelId.toString(16)} in worksheet ${id} pane ${pane.id} must exist in registry`).toBeDefined();
        }
      }
    }
  });

  it('verifies pane proportions sum to 100% for each standard worksheet', () => {
    for (const id of standardIds) {
      const ws = WORKSHEETS[id];
      const sum = ws.panes.reduce((acc, p) => acc + p.proportion, 0);
      expect(sum, `Proportions in worksheet ${id} must sum to 100`).toBe(100);
    }
  });

  function getPane(panes: readonly { id: string; proportion: number; series: readonly { channelId: number }[] }[], index: number) {
    const p = panes[index];
    if (!p) throw new Error(`Missing pane at index ${index}`);
    return p;
  }

  it('configures Driver worksheet with 5 panes and exact proportions (G02, T14)', () => {
    const ws = WORKSHEETS.driver;
    expect(ws.panes).toHaveLength(5);
    expect(getPane(ws.panes, 0)).toMatchObject({ id: 'speed', proportion: 35 });
    expect(getPane(ws.panes, 0).series.map((s) => s.channelId)).toEqual([ch.SPEED]);

    expect(getPane(ws.panes, 1)).toMatchObject({ id: 'delta', proportion: 15 });
    expect(getPane(ws.panes, 1).series.map((s) => s.channelId)).toEqual([ch.DELTA_BEST]);

    expect(getPane(ws.panes, 2)).toMatchObject({ id: 'inputs', proportion: 20 });
    expect(getPane(ws.panes, 2).series.map((s) => s.channelId)).toEqual([ch.THROTTLE, ch.BRAKE]);

    expect(getPane(ws.panes, 3)).toMatchObject({ id: 'engine', proportion: 15 });
    expect(getPane(ws.panes, 3).series.map((s) => s.channelId)).toEqual([ch.GEAR, ch.RPM]);

    // T14: Steering must use STEERING_ANGLE (0x001B), not raw STEERING
    expect(getPane(ws.panes, 4)).toMatchObject({ id: 'steering', proportion: 15 });
    expect(getPane(ws.panes, 4).series.map((s) => s.channelId)).toEqual([ch.STEERING_ANGLE]);
  });

  it('configures Tyres worksheet with carcass/optimal temps and pressures (G03)', () => {
    const ws = WORKSHEETS.tyres;
    expect(ws.panes).toHaveLength(2);
    expect(getPane(ws.panes, 0).id).toBe('tyre-temps');
    expect(getPane(ws.panes, 0).series.map((s) => s.channelId)).toEqual([
      ch.TYRE_CARCASS_TEMP_FL,
      ch.TYRE_CARCASS_TEMP_FR,
      ch.TYRE_CARCASS_TEMP_RL,
      ch.TYRE_CARCASS_TEMP_RR,
      ch.TYRE_OPTIMAL_TEMP_FL,
      ch.TYRE_OPTIMAL_TEMP_FR,
      ch.TYRE_OPTIMAL_TEMP_RL,
      ch.TYRE_OPTIMAL_TEMP_RR,
    ]);
    expect(getPane(ws.panes, 1).id).toBe('tyre-pressures');
    expect(getPane(ws.panes, 1).series.map((s) => s.channelId)).toEqual([
      ch.TYRE_PRESSURE_FL,
      ch.TYRE_PRESSURE_FR,
      ch.TYRE_PRESSURE_RL,
      ch.TYRE_PRESSURE_RR,
    ]);
  });

  it('configures Brakes worksheet with disc temps, pedal, bias, and ABS ticks', () => {
    const ws = WORKSHEETS.brakes;
    expect(ws.panes).toHaveLength(2);
    expect(getPane(ws.panes, 0).series.map((s) => s.channelId)).toEqual([
      ch.BRAKE_TEMP_FL,
      ch.BRAKE_TEMP_FR,
      ch.BRAKE_TEMP_RL,
      ch.BRAKE_TEMP_RR,
    ]);
    expect(getPane(ws.panes, 1).series.map((s) => s.channelId)).toEqual([
      ch.BRAKE,
      ch.BRAKE_BIAS_FRONT,
      ch.ABS_ACTIVE,
    ]);
  });

  it('configures Energy worksheet with VE, SoC, throttle, and motor torque', () => {
    const ws = WORKSHEETS.energy;
    expect(ws.panes).toHaveLength(2);
    expect(getPane(ws.panes, 0).series.map((s) => s.channelId)).toEqual([
      ch.VIRTUAL_ENERGY,
      ch.BATTERY_SOC,
    ]);
    expect(getPane(ws.panes, 1).series.map((s) => s.channelId)).toEqual([
      ch.THROTTLE,
      ch.HYBRID_MOTOR_TORQUE,
    ]);
  });

  it('configures Powertrain worksheet with water/oil temps instead of FUEL (G04)', () => {
    const ws = WORKSHEETS.powertrain;
    expect(ws.panes).toHaveLength(3);
    expect(getPane(ws.panes, 0).series.map((s) => s.channelId)).toEqual([
      ch.WATER_TEMP,
      ch.OIL_TEMP,
    ]);
    expect(getPane(ws.panes, 1).series.map((s) => s.channelId)).toEqual([
      ch.RPM,
      ch.SPEED,
    ]);
    expect(getPane(ws.panes, 2).series.map((s) => s.channelId)).toEqual([
      ch.GEAR,
      ch.THROTTLE,
    ]);
  });

  it('handles custom worksheets and default worksheet lookup', () => {
    expect(DEFAULT_WORKSHEET_ID).toBe('driver');
    const custom = getWorksheet('custom', [{ channelId: ch.SPEED, color: 2 }]);
    expect(custom.id).toBe('custom');
    expect(custom.panes).toHaveLength(1);
    expect(getPane(custom.panes, 0).proportion).toBe(100);
    expect(getPane(custom.panes, 0).series[0]?.channelId).toBe(ch.SPEED);
  });

  it('gathers all distinct channels and series across panes', () => {
    const driverWs = WORKSHEETS.driver;
    const channels = worksheetAllChannels(driverWs);
    expect(channels).toContain(ch.SPEED);
    expect(channels).toContain(ch.DELTA_BEST);
    expect(channels).toContain(ch.THROTTLE);
    expect(channels).toContain(ch.BRAKE);
    expect(channels).toContain(ch.GEAR);
    expect(channels).toContain(ch.RPM);
    expect(channels).toContain(ch.STEERING_ANGLE);

    const allSeries = worksheetAllSeries(driverWs);
    expect(allSeries.map((s) => s.channelId)).toEqual(channels);
  });
});
