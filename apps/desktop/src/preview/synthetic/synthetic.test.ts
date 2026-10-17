// Preview harness self-check: the SYNTHETIC preview stream must be valid wire data for the real TelemetryStore
// (so the preview renders exactly what the app would), and the synthetic car / field must be plausible.
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { TelemetryStore } from '../../telemetry/store';
import { buildLapProfile } from './car';
import { SyntheticField } from './field';
import { SyntheticTelemetryStream } from './telemetry';

function stream(variant: 'gt3' | 'hypercar'): SyntheticTelemetryStream {
  return new SyntheticTelemetryStream({
    variant,
    startSessionTimeS: 2340,
    startLap: 19,
    startFuelL: 52.3,
    raceLaps: 118,
    bestLapS: 115,
    position: 7,
    classPosition: 7,
  });
}

describe('preview synthetic telemetry', () => {
  it.each(['gt3', 'hypercar'] as const)('%s: backfill and live batches are accepted without rejections', (variant) => {
    const store = new TelemetryStore({ scheduleFrame: () => () => undefined });
    const s = stream(variant);
    for (const batch of s.backfill(5, 200)) {
      const result = store.applyBatch(batch);
      expect(result.ok).toBe(true);
    }
    for (let i = 0; i < 20; i++) expect(store.applyBatch(s.live(5)).ok).toBe(true);
    const status = store.getStatus();
    expect(status.batchesRejected).toBe(0);
    expect(status.unknownSkipped).toBe(0);
    expect(status.invalidated).toBe(0);
    expect(status.resyncNeeded).toBe(false);
    expect(typeof store.getSample(ch.SPEED).value).toBe('number');
    expect(store.getSample(ch.VEHICLE_NAME).value).toBeTypeOf('string');
    const ve = store.getSample(ch.VIRTUAL_ENERGY).value;
    if (variant === 'hypercar') expect(ve).toBeTypeOf('number');
    else expect(ve).toBeNull();
  });

  it('lap times are plausible for the class', () => {
    const gt3 = buildLapProfile('gt3').lapTimeS;
    const hyper = buildLapProfile('hypercar').lapTimeS;
    expect(gt3).toBeGreaterThan(105);
    expect(gt3).toBeLessThan(135);
    expect(hyper).toBeLessThan(gt3);
  });

  it('the field has 25 cars, one player, consistent places', () => {
    const profile = buildLapProfile('gt3');
    const field = new SyntheticField({
      kind: 'gt3',
      playerVehicle: 'Porsche 911 GT3 R LMGT3',
      playerClass: 'GT3',
      playerDriver: 'Sam Driver',
      playerPlace: 7,
      playerProfile: profile,
      playerLapsDone: 19,
      sessionType: 'RACE',
      raceLengthS: 14400,
      referenceSessionTimeS: 2340,
    });
    const snap = field.snapshot(60);
    expect(snap.vehicles).toHaveLength(25);
    expect(snap.vehicles.filter((v) => v.is_player)).toHaveLength(1);
    expect(snap.vehicles.map((v) => v.place)).toEqual(Array.from({ length: 25 }, (_, i) => i + 1));
    // The running order agrees with the player's telemetry POSITION (7) throughout the lap.
    for (const t of [0, 20, 45, 60, 90]) {
      expect(field.snapshot(t).vehicles.find((v) => v.is_player)?.place).toBe(7);
    }
  });
});
