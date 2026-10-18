// Preview harness self-check: the SYNTHETIC preview stream must be valid wire data for the real TelemetryStore
// (so the preview renders exactly what the app would), and the synthetic car / field must be plausible.
import { describe, expect, it } from 'vitest';
import { ch } from '../../generated/channels';
import { TelemetryStore } from '../../telemetry/store';
import { CarSimulator, MAX_SURFACE_SPIKE_C, ROW_DT_S, buildLapProfile, type CarRow, type CarVariant } from './car';
import { SyntheticField } from './field';
import { SyntheticTelemetryStream, type StreamOptions } from './telemetry';

function stream(variant: CarVariant, extra: Partial<StreamOptions> = {}): SyntheticTelemetryStream {
  return new SyntheticTelemetryStream({
    variant,
    startSessionTimeS: 2340,
    startLap: 19,
    startFuelL: 52.3,
    raceLaps: 118,
    bestLapS: 115,
    position: 7,
    classPosition: 7,
    ...extra,
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

// --- T28: generator sanity (REDESIGN W0.1) ---------------------------------------------------------------

function laps(variant: CarVariant, n: number, options = {}): CarRow[] {
  const sim = new CarSimulator(variant, 0, 0, 80, options);
  const rows: CarRow[] = [];
  const count = Math.round((n * sim.profile.lapTimeS) / ROW_DT_S);
  for (let i = 0; i < count; i++) rows.push(sim.step());
  return rows;
}

function storeFor(s: SyntheticTelemetryStream, backfillS: number, liveBatches = 1): TelemetryStore {
  const store = new TelemetryStore({ scheduleFrame: () => () => undefined });
  for (const batch of s.backfill(backfillS)) expect(store.applyBatch(batch).ok).toBe(true);
  // Live batches of 25 rows confirm every channel (scoring-class channels arrive every 20th row).
  for (let i = 0; i < liveBatches; i++) expect(store.applyBatch(s.live(25)).ok).toBe(true);
  return store;
}

describe('preview synthetic generator sanity (T28)', () => {
  it.each(['gt3', 'hypercar'] as const)('%s: the middle tyre temperature always lies between the edges', (variant) => {
    for (const r of laps(variant, 2)) {
      for (const w of r.wheels) {
        expect(w.surface).toBeLessThanOrEqual(Math.max(w.inner, w.outer));
        expect(w.surface).toBeGreaterThanOrEqual(Math.min(w.inner, w.outer));
      }
    }
  });

  it.each(['gt3', 'hypercar'] as const)('%s: surface = carcass + a bounded local spike; carcass is slow', (variant) => {
    const rows = laps(variant, 2);
    for (let w = 0; w < 4; w++) {
      const carcass = rows.map((r) => r.wheels[w]?.carcass ?? Number.NaN);
      const surface = rows.map((r) => r.wheels[w]?.surface ?? Number.NaN);
      // Carcass moves a few degrees per lap at most; the surface never swings like 60-118 °C (D8).
      expect(Math.max(...carcass) - Math.min(...carcass)).toBeLessThan(6);
      expect(Math.max(...surface) - Math.min(...surface)).toBeLessThanOrEqual(MAX_SURFACE_SPIKE_C + 6);
      rows.forEach((r) => {
        const wheel = r.wheels[w];
        if (wheel === undefined) throw new Error('wheel missing');
        expect(wheel.surface - wheel.carcass).toBeGreaterThanOrEqual(0);
        expect(wheel.surface - wheel.carcass).toBeLessThanOrEqual(MAX_SURFACE_SPIKE_C + 1e-9);
      });
      // Plausible running window for slicks.
      expect(Math.min(...surface)).toBeGreaterThan(70);
      expect(Math.max(...surface)).toBeLessThan(125);
    }
  });

  it('the four corners do not swing in phase (left and right differ through corners)', () => {
    const rows = laps('gt3', 1);
    const differing = rows.filter((r) => Math.abs((r.wheels[0]?.surface ?? 0) - (r.wheels[1]?.surface ?? 0)) > 2);
    expect(differing.length / rows.length).toBeGreaterThan(0.1);
  });

  it.each(['gt3', 'hypercar'] as const)('%s: brake temps are plausible and the stint peak never decreases', (variant) => {
    const rows = laps(variant, 2);
    let lastPeak = 0;
    for (const r of rows) {
      const fl = r.wheels[0];
      if (fl === undefined) throw new Error('wheel missing');
      expect(fl.brakePeak).toBeGreaterThanOrEqual(lastPeak);
      expect(fl.brakePeak).toBeGreaterThanOrEqual(fl.brake);
      lastPeak = fl.brakePeak;
    }
    const fl = rows.map((r) => r.wheels[0]?.brake ?? 0);
    expect(Math.min(...fl)).toBeGreaterThan(200);
    expect(Math.max(...fl)).toBeLessThan(1000);
  });

  it('the brake alert threshold (FL peak above 650 °C) agrees with the peak channel (D7)', () => {
    const store = storeFor(stream('gt3'), 60, 5);
    const peak = store.getSample(ch.BRAKE_TEMP_PEAK_FL).value;
    expect(typeof peak).toBe('number');
    expect(peak as number).toBeGreaterThan(650);
    // Heavy braking zones really pass the threshold in the live trace, not only in the peak.
    const fl = laps('gt3', 1).map((r) => r.wheels[0]?.brake ?? 0);
    expect(Math.max(...fl)).toBeGreaterThan(650);
  });

  it('the current lap time advances and differs between cars at the same moment (D14)', () => {
    const lapTime = (variant: CarVariant, offset: number): number => {
      const store = storeFor(stream(variant, { lapOffsetS: offset }), 10);
      const v = store.getSample(ch.LAP_TIME).value;
      if (typeof v !== 'number') throw new Error('LAP_TIME missing');
      return v;
    };
    const s = stream('gt3');
    const store = storeFor(s, 5);
    const t0 = store.getSample(ch.LAP_TIME).value as number;
    expect(store.applyBatch(s.live(100)).ok).toBe(true);
    const t1 = store.getSample(ch.LAP_TIME).value as number;
    expect(t1 - t0).toBeCloseTo(1, 1);
    expect(lapTime('gt3', 31)).not.toBeCloseTo(lapTime('hypercar', 55), 0);
  });

  it('race start: no completed lap means no best/last lap and no fuel-per-lap estimate (unknown is not 0)', () => {
    const store = storeFor(stream('gt3', { startLap: 0, bestLapS: Number.NaN }), 20, 2);
    for (const id of [ch.BEST_LAP_TIME, ch.LAST_LAP_TIME, ch.FUEL_PER_LAP, ch.FUEL_TO_FINISH, ch.DELTA_PERSONAL_BEST]) {
      expect(store.getSample(id).value).toBeNull();
    }
    expect(typeof store.getSample(ch.LAP_TIME).value).toBe('number');
  });

  it('qualifying: no fuel-to-finish', () => {
    const store = storeFor(stream('gt3', { session: 'qualifying', raceLaps: Number.NaN }), 5);
    expect(store.getSample(ch.FUEL_TO_FINISH).value).toBeNull();
    expect(store.getSample(ch.FUEL_TO_ADD).value).toBeNull();
    expect(typeof store.getSample(ch.FUEL_LAPS_REMAINING).value).toBe('number');
  });

  it('GT3 with virtual energy: VE present, hybrid channels absent', () => {
    const store = storeFor(stream('gt3', { virtualEnergy: true }), 5);
    expect(typeof store.getSample(ch.VIRTUAL_ENERGY).value).toBe('number');
    expect(store.getSample(ch.BATTERY_SOC).value).toBeNull();
    expect(store.getSample(ch.REGEN_POWER).value).toBeNull();
  });

  it('FCY: flags carry full-course yellow; green otherwise', () => {
    const fcy = storeFor(stream('gt3', { fcy: true }), 2);
    expect(fcy.getSample(ch.FULL_COURSE_YELLOW).value).toBe(true);
    expect(fcy.getSample(ch.FLAG_SUMMARY).value).toBe(2);
    const green = storeFor(stream('gt3'), 2);
    expect(green.getSample(ch.FULL_COURSE_YELLOW).value).toBe(false);
    expect(green.getSample(ch.FLAG_SUMMARY).value).toBe(0);
  });

  it('position channels follow the synthetic layout', () => {
    const store = storeFor(stream('gt3'), 2);
    for (const id of [ch.LAP_DISTANCE, ch.POS_X, ch.POS_Z]) expect(typeof store.getSample(id).value).toBe('number');
  });

  it('the stream is deterministic (same options, same bytes)', () => {
    const a = stream('hypercar');
    const b = stream('hypercar');
    expect(a.backfill(3)).toEqual(b.backfill(3));
    expect(a.live(50)).toEqual(b.live(50));
  });

  it('qualifying field: ordered by best lap with best-lap gaps', () => {
    const field = new SyntheticField({
      kind: 'gt3',
      playerVehicle: 'Porsche 911 GT3 R LMGT3',
      playerClass: 'GT3',
      playerDriver: 'Sam Driver',
      playerPlace: 5,
      playerProfile: buildLapProfile('gt3'),
      playerLapsDone: 3,
      sessionType: 'Qualifying 1',
      raceLengthS: 1200,
      referenceSessionTimeS: 600,
      qualifying: true,
    });
    const snap = field.snapshot(30);
    expect(snap.vehicles.find((v) => v.is_player)?.place).toBe(5);
    const bests = snap.vehicles.map((v) => v.best_lap_s ?? Number.NaN);
    for (let i = 1; i < bests.length; i++) expect(bests[i]).toBeGreaterThan(bests[i - 1] ?? Infinity);
    snap.vehicles.forEach((v, i) => {
      expect(v.time_behind_leader_s).toBeCloseTo((bests[i] ?? 0) - (bests[0] ?? 0), 6);
      expect(v.laps_behind_leader).toBe(0);
    });
  });

  it('mixed field with a GT3 player: prototypes are faster, the player keeps its place', () => {
    const field = new SyntheticField({
      kind: 'mixed',
      playerVehicle: 'Porsche 911 GT3 R LMGT3',
      playerClass: 'GT3',
      playerDriver: 'Sam Driver',
      playerPlace: 18,
      playerProfile: buildLapProfile('gt3'),
      playerLapsDone: 100,
      sessionType: 'Race 1',
      raceLengthS: 14400,
      referenceSessionTimeS: 12960,
    });
    for (const t of [0, 30, 60]) {
      const snap = field.snapshot(t);
      const player = snap.vehicles.find((v) => v.is_player);
      expect(player?.place).toBe(18);
      expect(player?.class_place).toBe(5);
    }
    const snap = field.snapshot(60);
    const best = (cls: string): number =>
      Math.min(...snap.vehicles.filter((v) => v.class_name === cls).map((v) => v.best_lap_s ?? Infinity));
    expect(best('Hypercar')).toBeLessThan(best('LMP2'));
    expect(best('LMP2')).toBeLessThan(best('GT3'));
  });

  it('race-start field: nose to tail, nobody lapped or in the pits, no negative laps', () => {
    const field = new SyntheticField({
      kind: 'gt3',
      playerVehicle: 'Porsche 911 GT3 R LMGT3',
      playerClass: 'GT3',
      playerDriver: 'Sam Driver',
      playerPlace: 7,
      playerProfile: buildLapProfile('gt3'),
      playerLapsDone: 0,
      sessionType: 'Race 1',
      raceLengthS: 14400,
      referenceSessionTimeS: 180,
      phase: 'start',
    });
    const snap = field.snapshot(25);
    expect(snap.vehicles.find((v) => v.is_player)?.place).toBe(7);
    for (const v of snap.vehicles) {
      expect(v.laps ?? 0).toBeGreaterThanOrEqual(0);
      expect(v.in_pits).toBe(false);
      expect(v.laps_behind_leader).toBe(0);
      expect(v.num_pitstops).toBe(0);
    }
  });
});
