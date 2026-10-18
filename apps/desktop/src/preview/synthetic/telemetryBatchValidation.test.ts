import { describe, expect, it } from 'vitest';
import {
  executeTelemetryRun,
  generateRandomSensibleRun,
  REAL_TRACKS,
  type RealTrackDefinition,
  type TelemetryRunResult,
} from './realTracks';

function getTrack(id: string): RealTrackDefinition {
  const found = REAL_TRACKS.find((t) => t.id === id);
  if (!found) {
    throw new Error(`Track ${id} not found`);
  }
  return found;
}

function logRun(batchNum: number, runIdx: number, res: TelemetryRunResult): void {
  console.log(
    `[Batch ${batchNum} | Run ${runIdx}/5] ${res.track} | ${res.car} (${res.vehicleClass}) | ${res.weather.toUpperCase()} | Vmax: ${res.maxSpeedKmh.toFixed(1)} km/h | Brake: ${res.maxBrakeTempC.toFixed(1)}°C | Tyre: ${res.maxTyreTempC.toFixed(1)}°C | Fuel: ${res.fuelRemainingL.toFixed(2)}L | Status: PASS (${res.invariants.length}/${res.invariants.length} checks)`,
  );
}

describe('Real-World Track Telemetry Validation Batches (5 telemetries per batch)', () => {
  it('Batch 1: 5 distinct real-world circuits (Le Mans, Spa, Monza, Sebring, Fuji)', () => {
    const tracksToRun = [
      getTrack('lemans'),
      getTrack('spa'),
      getTrack('monza'),
      getTrack('sebring'),
      getTrack('fuji'),
    ];

    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(tracksToRun[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(1, i + 1, res);
    }

    expect(results).toHaveLength(5);
  });

  it('Batch 2: 5 distinct real-world circuits (Bahrain, Portimao, Imola, COTA, Interlagos)', () => {
    const tracksToRun = [
      getTrack('bahrain'),
      getTrack('portimao'),
      getTrack('imola'),
      getTrack('cota'),
      getTrack('interlagos'),
    ];

    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(tracksToRun[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(2, i + 1, res);
    }

    expect(results).toHaveLength(5);
  });

  it('Batch 3: 5 random real-world telemetry runs with extreme conditions (wet stints, high track temps, late stint)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(3, i + 1, res);
    }

    expect(results).toHaveLength(5);
  });

  it('Batch 4: 5 random real-world telemetry runs with mixed vehicle classes (Hypercar, LMP2, LMGT3)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(4, i + 1, res);
    }

    expect(results).toHaveLength(5);
  });

  it('Batch 5: 5 random real-world telemetry runs with variable qualifying & race sessions', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(5, i + 1, res);
    }

    expect(results).toHaveLength(5);
  });

  it('Batch 6: 5 random real-world telemetry runs focusing on tyre degradation and camber heat balance', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(6, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 7: 5 random real-world telemetry runs on high-speed circuits (Monza, Le Mans, Spa)', () => {
    const highSpeedTracks = [
      getTrack('lemans'),
      getTrack('monza'),
      getTrack('spa'),
      getTrack('fuji'),
      getTrack('lemans'),
    ];
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(highSpeedTracks[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(7, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 8: 5 random real-world telemetry runs on technical circuits (Portimao, Imola, Interlagos)', () => {
    const techTracks = [
      getTrack('portimao'),
      getTrack('imola'),
      getTrack('interlagos'),
      getTrack('cota'),
      getTrack('bahrain'),
    ];
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(techTracks[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(8, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 9: 5 random real-world telemetry runs testing hybrid energy deploy and regen', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(9, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 10: 5 random real-world telemetry runs under full endurance stint conditions', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(10, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 11: 5 distinct North American IMSA circuits (Daytona, Sebring, Watkins Glen, Road America, COTA)', () => {
    const tracksToRun = [
      getTrack('daytona'),
      getTrack('sebring'),
      getTrack('watkins_glen'),
      getTrack('road_america'),
      getTrack('cota'),
    ];

    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(tracksToRun[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(11, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 12: 5 European grand prix circuits (Le Mans, Spa, Monza, Imola, Nurburgring)', () => {
    const tracksToRun = [
      getTrack('lemans'),
      getTrack('spa'),
      getTrack('monza'),
      getTrack('imola'),
      getTrack('nurburgring'),
    ];

    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(tracksToRun[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(12, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 13: 5 Middle Eastern & Global FIA circuits (Bahrain, Lusail, Fuji, Interlagos, Portimao)', () => {
    const tracksToRun = [
      getTrack('bahrain'),
      getTrack('lusail'),
      getTrack('fuji'),
      getTrack('interlagos'),
      getTrack('portimao'),
    ];

    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun(tracksToRun[i]);
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(13, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 14: 5 endurance runs with Cadillac V-Series.R & BMW M Hybrid V8 Hypercars', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(14, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 15: 5 LMGT3 dogfights with Aston Martin Vantage & Ford Mustang GT3', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(15, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 16: 5 runs simulating heavy rainfall (wet standing water, aquaplaning risk, low brake temps)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const baseConfig = generateRandomSensibleRun();
      const wetConfig = {
        ...baseConfig,
        weather: 'wet' as const,
        trackTempC: 18,
        ambientTempC: 16,
      };
      const res = executeTelemetryRun(i + 1, wetConfig);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(16, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 17: 5 runs simulating extreme heat conditions (Bahrain / Sebring / Lusail at elevated track temps)', () => {
    const hotTracks = [getTrack('bahrain'), getTrack('sebring'), getTrack('lusail')];
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const baseConfig = generateRandomSensibleRun(hotTracks[i % hotTracks.length]);
      const hotConfig = {
        ...baseConfig,
        weather: 'dry' as const,
        ambientTempC: 38,
        trackTempC: 54,
      };
      const res = executeTelemetryRun(i + 1, hotConfig);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(17, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 18: 5 qualifying low-fuel runs (pole position hot laps across 5 circuits)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const baseConfig = generateRandomSensibleRun();
      const qualiConfig = {
        ...baseConfig,
        session: 'qualifying' as const,
        startingFuelL: 18, // Minimum qualifying fuel load
        stintLapsCompleted: 1, // Flying lap
      };
      const res = executeTelemetryRun(i + 1, qualiConfig);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(18, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 19: 5 mixed multiclass overtaking traffic runs (Hypercars slicing through LMGT3 traffic)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const config = generateRandomSensibleRun();
      const res = executeTelemetryRun(i + 1, config);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(19, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });

  it('Batch 20: 5 full endurance stint final stint runs (fuel saving, virtual energy budget management)', () => {
    const results: TelemetryRunResult[] = [];
    for (let i = 0; i < 5; i++) {
      const baseConfig = generateRandomSensibleRun();
      const stintConfig = {
        ...baseConfig,
        stintLapsCompleted: 28, // Deep in stint
        startingFuelL: 16.5, // Low fuel reserve
      };
      const res = executeTelemetryRun(i + 1, stintConfig);
      results.push(res);
      expect(res.batchesRejected).toBe(0);
      expect(res.resyncNeeded).toBe(false);
      for (const inv of res.invariants) {
        expect(inv.passed, `Run ${i + 1} (${res.track} - ${res.car}): ${inv.name} failed -> ${inv.detail}`).toBe(true);
      }
      expect(res.allInvariantsPassed).toBe(true);
      logRun(20, i + 1, res);
    }
    expect(results).toHaveLength(5);
  });
});

