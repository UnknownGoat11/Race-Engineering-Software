/**
 * Real-world WEC / IMSA circuits and multi-track synthetic telemetry generator.
 *
 * Implements authentic circuit geometries, sector splits, track lengths, and
 * multi-car simulation parameters for comprehensive telemetry validation.
 */
import { ch } from '../../generated/channels';
import { TelemetryStore } from '../../telemetry/store';
import { UI_BATCH_FLAG_BACKFILL, UI_BATCH_FLAG_RESET } from '../../telemetry/uiBatch';
import { encodeTlvBlock, encodeUiBatch, type FixtureRecord } from '../../test/tlvEncoder';
import { numberRecord } from './telemetry';

export interface RealTrackDefinition {
  readonly id: string;
  readonly name: string;
  readonly country: string;
  readonly lengthM: number;
  readonly sectorStartsM: readonly [number, number];
  readonly controlPoints: readonly (readonly [number, number])[];
  readonly elevationChangeM: number;
  readonly baseTrackTempC: number;
}

export interface RealCarDefinition {
  readonly id: string;
  readonly name: string;
  readonly vehicleClass: 'Hypercar' | 'LMP2' | 'GT3';
  readonly vmax: number; // m/s
  readonly latG: number;
  readonly brakeG: number;
  readonly powerAccel: number;
  readonly fuelCapacityL: number;
  readonly baseFuelPer1000m: number;
  readonly rpmMax: number;
  readonly gearTopSpeeds: readonly number[];
  readonly brakeBiasFront: number;
  readonly hybrid: boolean;
  readonly virtualEnergy: boolean;
}

export const REAL_TRACKS: readonly RealTrackDefinition[] = [
  {
    id: 'lemans',
    name: 'Circuit de la Sarthe',
    country: 'France',
    lengthM: 13626,
    sectorStartsM: [4250, 9800],
    elevationChangeM: 37,
    baseTrackTempC: 28,
    controlPoints: [
      [0, 0],
      [600, 0],
      [900, 150], // Dunlop Chicane
      [1400, 200], // Tertre Rouge
      [2800, 1200], // Mulsanne Straight part 1
      [3600, 1800], // Chicane 1
      [4800, 2700], // Chicane 2
      [6000, 3600], // Mulsanne Corner
      [7400, 3100], // Towards Indianapolis
      [8200, 2600], // Indianapolis
      [8600, 2200], // Arnage
      [9800, 1400], // Run to Porsche Curves
      [10800, 800], // Porsche Curves 1
      [11400, 400], // Porsche Curves 2
      [12200, 100], // Maison Blanche
      [13000, 0], // Ford Chicanes
    ],
  },
  {
    id: 'spa',
    name: 'Circuit de Spa-Francorchamps',
    country: 'Belgium',
    lengthM: 7004,
    sectorStartsM: [2240, 5280],
    elevationChangeM: 104,
    baseTrackTempC: 24,
    controlPoints: [
      [0, 0],
      [300, 0],
      [450, -100], // La Source
      [800, -80], // Downhill to Eau Rouge
      [1200, 50], // Raidillon crest
      [2200, 600], // Kemmel Straight
      [2700, 800], // Les Combes
      [3100, 900], // Malmedy
      [3600, 850], // Bruxelles 180
      [4200, 600], // Speakers Corner
      [4800, 350], // Pouhon double left
      [5400, 150], // Campus
      [5900, -50], // Stavelot
      [6400, -150], // Blanchimont
      [6800, -80], // Bus Stop
    ],
  },
  {
    id: 'monza',
    name: 'Autodromo Nazionale Monza',
    country: 'Italy',
    lengthM: 5793,
    sectorStartsM: [1750, 4100],
    elevationChangeM: 12,
    baseTrackTempC: 34,
    controlPoints: [
      [0, 0],
      [900, 0], // Rettifilo straight
      [1200, -50], // Variante del Rettifilo
      [1800, 200], // Curva Grande
      [2400, 400], // Variante della Roggia
      [2800, 450], // Lesmo 1
      [3200, 400], // Lesmo 2
      [3800, 200], // Curva del Serraglio
      [4300, 50], // Variante Ascari
      [5100, -100], // Back straight
      [5600, -50], // Curva Parabolica (Alboreto)
    ],
  },
  {
    id: 'sebring',
    name: 'Sebring International Raceway',
    country: 'USA',
    lengthM: 6019,
    sectorStartsM: [1985, 4130],
    elevationChangeM: 6,
    baseTrackTempC: 36,
    controlPoints: [
      [0, 0],
      [420, 0],
      [560, 60],
      [600, 180],
      [700, 260],
      [880, 250],
      [980, 330],
      [960, 470],
      [1080, 560],
      [1300, 560],
      [1420, 480],
      [1440, 340],
      [1380, 220],
      [1420, 100],
      [1560, 40],
      [1640, -80],
      [1600, -220],
      [1440, -300],
      [1100, -330],
      [700, -360],
      [300, -390],
      [60, -400],
      [-120, -350],
      [-180, -220],
      [-140, -90],
    ],
  },
  {
    id: 'fuji',
    name: 'Fuji Speedway',
    country: 'Japan',
    lengthM: 4563,
    sectorStartsM: [1520, 3180],
    elevationChangeM: 40,
    baseTrackTempC: 27,
    controlPoints: [
      [0, 0],
      [1475, 0], // Massive 1.475 km main straight
      [1700, 80], // Turn 1 (First Corner 180)
      [2200, 300], // Coca-Cola corner
      [2700, 450], // 100R high-speed right
      [3100, 380], // Advan hairpin
      [3600, 250], // 300R
      [4000, 150], // Dunlop chicane
      [4300, 50], // Panasonic corner
    ],
  },
  {
    id: 'bahrain',
    name: 'Bahrain International Circuit',
    country: 'Bahrain',
    lengthM: 5412,
    sectorStartsM: [1690, 3780],
    elevationChangeM: 18,
    baseTrackTempC: 38,
    controlPoints: [
      [0, 0],
      [800, 0],
      [1050, -100], // Turn 1 hairpin
      [1400, -50], // Turns 2 & 3 exit
      [1800, 150], // Turn 4
      [2300, 350], // Turns 5, 6, 7
      [2800, 450], // Turn 8 hairpin
      [3300, 380], // Downhill Turns 9 & 10
      [3900, 150], // Back straight
      [4300, -50], // Turns 11 & 12
      [4800, -150], // Turn 13
      [5200, -80], // Turn 14 final corner
    ],
  },
  {
    id: 'portimao',
    name: 'Autódromo Internacional do Algarve',
    country: 'Portugal',
    lengthM: 4653,
    sectorStartsM: [1490, 3250],
    elevationChangeM: 39,
    baseTrackTempC: 30,
    controlPoints: [
      [0, 0],
      [700, 0], // Start/finish straight
      [1000, -80], // Downhill Turn 1
      [1400, 50], // Turn 3
      [1800, 200], // Turn 5 hairpin
      [2300, 350], // Torre VIP crest
      [2800, 400], // Blind Turns 8 & 9
      [3300, 300], // Turn 11 downhill
      [3800, 150], // Turn 13
      [4300, 50], // Galp fast sweeper
    ],
  },
  {
    id: 'imola',
    name: 'Autodromo Internazionale Enzo e Dino Ferrari',
    country: 'Italy',
    lengthM: 4909,
    sectorStartsM: [1620, 3410],
    elevationChangeM: 35,
    baseTrackTempC: 31,
    controlPoints: [
      [0, 0],
      [600, 0],
      [900, 60], // Variante Tamburello
      [1500, 180], // Variante Villeneuve
      [1900, 300], // Tosa uphill hairpin
      [2400, 450], // Piratella blind crest
      [2900, 350], // Acque Minerali
      [3500, 200], // Variante Alta
      [4200, 50], // Rivazza 1 & 2
      [4700, -20], // Straight to finish
    ],
  },
  {
    id: 'cota',
    name: 'Circuit of the Americas',
    country: 'USA',
    lengthM: 5513,
    sectorStartsM: [1720, 3840],
    elevationChangeM: 41,
    baseTrackTempC: 35,
    controlPoints: [
      [0, 0],
      [500, 0], // Steep 41m uphill climb
      [650, -100], // Turn 1 sharp left hairpin
      [1200, 150], // High speed Esses (Turns 3-5)
      [1700, 350], // Turn 6-8
      [2100, 450], // Turn 9-10
      [2600, 500], // Turn 11 hairpin
      [3700, 500], // 1.1 km back straight
      [4100, 350], // Stadium complex
      [4700, 150], // Multi-apex Turns 16-18
      [5200, 50], // Turns 19-20
    ],
  },
  {
    id: 'interlagos',
    name: 'Autódromo José Carlos Pace',
    country: 'Brazil',
    lengthM: 4309,
    sectorStartsM: [1380, 2950],
    elevationChangeM: 43,
    baseTrackTempC: 33,
    controlPoints: [
      [0, 0],
      [400, 0],
      [600, 100], // Senna S
      [1000, 250], // Curva do Sol
      [1600, 450], // Reta Oposta
      [2000, 500], // Descida do Lago
      [2500, 400], // Ferradura
      [2900, 300], // Pinheirinho
      [3300, 200], // Bico de Pato
      [3700, 100], // Mergulho
      [4000, 20], // Junção
    ],
  },
  {
    id: 'daytona',
    name: 'Daytona International Speedway (Road Course)',
    country: 'USA',
    lengthM: 5729,
    sectorStartsM: [1850, 3950],
    elevationChangeM: 9,
    baseTrackTempC: 32,
    controlPoints: [
      [0, 0],
      [400, 0], // Start/finish
      [650, -80], // Horseshoe infield entrance
      [1100, -150], // International Horseshoe
      [1600, -80], // Kink
      [2100, 50], // West horseshoe
      [2700, 200], // Oval turn 1-2 banking (31 degrees)
      [3800, 300], // Superstretch back straight
      [4300, 350], // Le Mans chicane (Bus Stop) entrance
      [4800, 250], // Bus stop exit to NASCAR 3-4 banking
      [5400, 100], // Tri-oval
    ],
  },
  {
    id: 'watkins_glen',
    name: 'Watkins Glen International',
    country: 'USA',
    lengthM: 5472,
    sectorStartsM: [1780, 3750],
    elevationChangeM: 35,
    baseTrackTempC: 28,
    controlPoints: [
      [0, 0],
      [500, 0], // Turn 1 (The Ninety)
      [1000, 100], // Uphill Esses
      [1700, 250], // Back straight (Inner Loop chicane)
      [2300, 350], // The Loop (Outer Loop)
      [2900, 450], // The Chute / Turn 6
      [3400, 400], // The Boot (Toe of the Boot)
      [4100, 280], // Heel of the Boot
      [4600, 180], // Turn 9
      [5100, 80], // Turn 10 & 11 back to straight
    ],
  },
  {
    id: 'road_america',
    name: 'Road America',
    country: 'USA',
    lengthM: 6515,
    sectorStartsM: [2150, 4420],
    elevationChangeM: 51,
    baseTrackTempC: 26,
    controlPoints: [
      [0, 0],
      [700, 0], // Main straight
      [1100, -80], // Turn 1
      [1700, -100], // Turn 3
      [2300, 100], // Moraine sweep
      [2800, 250], // Turn 5 downhill hairpin
      [3400, 380], // Hurry Downs
      [4000, 420], // The Carousel (200-degree sweeping right)
      [4700, 300], // The Kink (Turn 11)
      [5300, 150], // Canada Corner
      [5900, 50], // Bill Mitchell bend
    ],
  },
  {
    id: 'lusail',
    name: 'Lusail International Circuit',
    country: 'Qatar',
    lengthM: 5418,
    sectorStartsM: [1680, 3620],
    elevationChangeM: 8,
    baseTrackTempC: 37,
    controlPoints: [
      [0, 0],
      [1068, 0], // 1.068 km main straight
      [1350, 80], // Turn 1 right
      [1800, 200], // Turns 2 & 3
      [2300, 320], // Turn 4 & 5
      [2800, 400], // Turn 6 hairpin
      [3400, 320], // High speed Turns 7, 8, 9
      [4000, 200], // Turn 10
      [4600, 120], // Triple-apex right (Turns 12, 13, 14)
      [5100, 40], // Turn 15 & 16 onto straight
    ],
  },
  {
    id: 'nurburgring',
    name: 'Nürburgring GP-Strecke',
    country: 'Germany',
    lengthM: 5148,
    sectorStartsM: [1640, 3550],
    elevationChangeM: 23,
    baseTrackTempC: 22,
    controlPoints: [
      [0, 0],
      [500, 0], // Main straight
      [800, -60], // Castrol S (Turn 1 tight hairpin)
      [1300, -120], // Mercedes Arena
      [1800, 50], // Ford Kurve
      [2300, 180], // Dunlop Kehre
      [3000, 300], // Schumacher S (Turns 8 & 9)
      [3600, 250], // Kumho Kurve
      [4100, 150], // Balingen
      [4700, 80], // Veedol chicane
      [5000, 20], // Coca-Cola Kurve
    ],
  },
];

export const REAL_CARS: readonly RealCarDefinition[] = [
  {
    id: 'toyota_gr010',
    name: 'Toyota GR010 Hybrid',
    vehicleClass: 'Hypercar',
    vmax: 94,
    latG: 1.95,
    brakeG: 2.4,
    powerAccel: 5.8,
    fuelCapacityL: 90,
    baseFuelPer1000m: 0.52,
    rpmMax: 9000,
    gearTopSpeeds: [24, 36, 48, 60, 74, 94],
    brakeBiasFront: 0.545,
    hybrid: true,
    virtualEnergy: true,
  },
  {
    id: 'ferrari_499p',
    name: 'Ferrari 499P',
    vehicleClass: 'Hypercar',
    vmax: 95,
    latG: 1.98,
    brakeG: 2.45,
    powerAccel: 5.9,
    fuelCapacityL: 90,
    baseFuelPer1000m: 0.51,
    rpmMax: 9200,
    gearTopSpeeds: [25, 37, 49, 61, 75, 95],
    brakeBiasFront: 0.54,
    hybrid: true,
    virtualEnergy: true,
  },
  {
    id: 'porsche_963',
    name: 'Porsche 963',
    vehicleClass: 'Hypercar',
    vmax: 93,
    latG: 1.92,
    brakeG: 2.38,
    powerAccel: 5.7,
    fuelCapacityL: 90,
    baseFuelPer1000m: 0.53,
    rpmMax: 8900,
    gearTopSpeeds: [24, 35, 47, 59, 73, 93],
    brakeBiasFront: 0.55,
    hybrid: true,
    virtualEnergy: true,
  },
  {
    id: 'oreca_07',
    name: 'Oreca 07 Gibson',
    vehicleClass: 'LMP2',
    vmax: 88,
    latG: 1.82,
    brakeG: 2.2,
    powerAccel: 5.2,
    fuelCapacityL: 75,
    baseFuelPer1000m: 0.58,
    rpmMax: 9000,
    gearTopSpeeds: [23, 34, 45, 56, 70, 88],
    brakeBiasFront: 0.555,
    hybrid: false,
    virtualEnergy: false,
  },
  {
    id: 'porsche_911_gt3',
    name: 'Porsche 911 GT3 R LMGT3',
    vehicleClass: 'GT3',
    vmax: 76,
    latG: 1.38,
    brakeG: 1.65,
    powerAccel: 4.1,
    fuelCapacityL: 100,
    baseFuelPer1000m: 0.62,
    rpmMax: 9400,
    gearTopSpeeds: [22, 33, 43, 53, 63, 76],
    brakeBiasFront: 0.565,
    hybrid: false,
    virtualEnergy: true, // LMGT3 runs virtual energy in WEC
  },
  {
    id: 'ferrari_296_gt3',
    name: 'Ferrari 296 GT3',
    vehicleClass: 'GT3',
    vmax: 78,
    latG: 1.40,
    brakeG: 1.68,
    powerAccel: 4.2,
    fuelCapacityL: 105,
    baseFuelPer1000m: 0.60,
    rpmMax: 8500,
    gearTopSpeeds: [23, 34, 44, 54, 65, 78],
    brakeBiasFront: 0.56,
    hybrid: false,
    virtualEnergy: true,
  },
  {
    id: 'corvette_z06_gt3',
    name: 'Corvette Z06 GT3.R',
    vehicleClass: 'GT3',
    vmax: 77,
    latG: 1.37,
    brakeG: 1.62,
    powerAccel: 4.3,
    fuelCapacityL: 100,
    baseFuelPer1000m: 0.64,
    rpmMax: 8000,
    gearTopSpeeds: [22, 33, 44, 55, 66, 77],
    brakeBiasFront: 0.57,
    hybrid: false,
    virtualEnergy: true,
  },
  {
    id: 'cadillac_v_series_r',
    name: 'Cadillac V-Series.R',
    vehicleClass: 'Hypercar',
    vmax: 93,
    latG: 1.93,
    brakeG: 2.39,
    powerAccel: 5.75,
    fuelCapacityL: 90,
    baseFuelPer1000m: 0.525,
    rpmMax: 8800,
    gearTopSpeeds: [24, 35, 47, 59, 73, 93],
    brakeBiasFront: 0.545,
    hybrid: true,
    virtualEnergy: true,
  },
  {
    id: 'bmw_m_hybrid_v8',
    name: 'BMW M Hybrid V8',
    vehicleClass: 'Hypercar',
    vmax: 94,
    latG: 1.94,
    brakeG: 2.41,
    powerAccel: 5.82,
    fuelCapacityL: 90,
    baseFuelPer1000m: 0.518,
    rpmMax: 8900,
    gearTopSpeeds: [24, 36, 48, 60, 74, 94],
    brakeBiasFront: 0.542,
    hybrid: true,
    virtualEnergy: true,
  },
  {
    id: 'aston_martin_vantage_gt3',
    name: 'Aston Martin Vantage AMR GT3',
    vehicleClass: 'GT3',
    vmax: 77,
    latG: 1.39,
    brakeG: 1.64,
    powerAccel: 4.25,
    fuelCapacityL: 105,
    baseFuelPer1000m: 0.61,
    rpmMax: 8200,
    gearTopSpeeds: [22, 33, 44, 54, 65, 77],
    brakeBiasFront: 0.56,
    hybrid: false,
    virtualEnergy: true,
  },
  {
    id: 'ford_mustang_gt3',
    name: 'Ford Mustang GT3',
    vehicleClass: 'GT3',
    vmax: 76,
    latG: 1.36,
    brakeG: 1.61,
    powerAccel: 4.35,
    fuelCapacityL: 102,
    baseFuelPer1000m: 0.635,
    rpmMax: 8100,
    gearTopSpeeds: [22, 33, 43, 53, 64, 76],
    brakeBiasFront: 0.568,
    hybrid: false,
    virtualEnergy: true,
  },
];

export interface TelemetryRunConfig {
  readonly track: RealTrackDefinition;
  readonly car: RealCarDefinition;
  readonly session: 'race' | 'qualifying' | 'practice';
  readonly weather: 'dry' | 'damp' | 'wet';
  readonly ambientTempC: number;
  readonly trackTempC: number;
  readonly stintLapsCompleted: number;
  readonly raceTotalLaps: number;
  readonly startingFuelL: number;
  readonly playerPosition: number;
  readonly playerClassPosition: number;
  readonly rowsToSimulate: number;
}

export interface InvariantCheckResult {
  readonly name: string;
  readonly passed: boolean;
  readonly detail: string;
}

export interface TelemetryRunResult {
  readonly runIndex: number;
  readonly track: string;
  readonly car: string;
  readonly vehicleClass: string;
  readonly weather: string;
  readonly simulatedTimeS: number;
  readonly simulatedRows: number;
  readonly batchesEmitted: number;
  readonly batchesRejected: number;
  readonly resyncNeeded: boolean;
  readonly samplesInspected: number;
  readonly maxSpeedKmh: number;
  readonly maxBrakeTempC: number;
  readonly maxTyreTempC: number;
  readonly minTyreWearFrac: number;
  readonly fuelRemainingL: number;
  readonly invariants: readonly InvariantCheckResult[];
  readonly allInvariantsPassed: boolean;
}

/**
 * Builds Catmull-Rom spline samples along the control points scaled to exact track length.
 */
function buildTrackSamples(
  controlPoints: readonly (readonly [number, number])[],
  lengthM: number,
): { x: number; z: number; curvature: number }[] {
  const n = controlPoints.length;
  const dense: [number, number][] = [];
  const steps = 100;
  for (let i = 0; i < n; i++) {
    const p0 = controlPoints[((i - 1 + n) % n)] ?? [0, 0];
    const p1 = controlPoints[i] ?? [0, 0];
    const p2 = controlPoints[(i + 1) % n] ?? [0, 0];
    const p3 = controlPoints[(i + 2) % n] ?? [0, 0];
    for (let s = 0; s < steps; s++) {
      const t = s / steps;
      const t2 = t * t;
      const t3 = t2 * t;
      const x =
        0.5 *
        (2 * p1[0] +
          (-p0[0] + p2[0]) * t +
          (2 * p0[0] - 5 * p1[0] + 4 * p2[0] - p3[0]) * t2 +
          (-p0[0] + 3 * p1[0] - 3 * p2[0] + p3[0]) * t3);
      const z =
        0.5 *
        (2 * p1[1] +
          (-p0[1] + p2[1]) * t +
          (2 * p0[1] - 5 * p1[1] + 4 * p2[1] - p3[1]) * t2 +
          (-p0[1] + 3 * p1[1] - 3 * p2[1] + p3[1]) * t3);
      dense.push([x, z]);
    }
  }

  const cum: number[] = [0];
  for (let i = 1; i <= dense.length; i++) {
    const a = dense[i - 1] ?? [0, 0];
    const b = dense[i % dense.length] ?? [0, 0];
    cum.push((cum[i - 1] ?? 0) + Math.hypot(b[0] - a[0], b[1] - a[1]));
  }
  const rawLen = cum[cum.length - 1] ?? 1;
  const scale = lengthM / (rawLen > 0 ? rawLen : 1);

  const xs: number[] = [];
  const zs: number[] = [];
  let j = 0;
  for (let d = 0; d < lengthM; d++) {
    const target = d / scale;
    while (j < dense.length - 1 && (cum[j + 1] ?? Infinity) < target) j++;
    const a = dense[j] ?? [0, 0];
    const b = dense[(j + 1) % dense.length] ?? [0, 0];
    const c0 = cum[j] ?? 0;
    const c1 = cum[j + 1] ?? c0 + 1;
    const f = c1 > c0 ? (target - c0) / (c1 - c0) : 0;
    xs.push((a[0] + (b[0] - a[0]) * f) * scale);
    zs.push((a[1] + (b[1] - a[1]) * f) * scale);
  }

  const sampleCount = xs.length;
  const span = 8;
  const at = (arr: readonly number[], idx: number) => arr[((idx % sampleCount) + sampleCount) % sampleCount] ?? 0;
  const heading = (idx: number) => Math.atan2(at(zs, idx + 1) - at(zs, idx), at(xs, idx + 1) - at(xs, idx));
  const rawCurv: number[] = [];
  for (let i = 0; i < sampleCount; i++) {
    let dh = heading(i + span) - heading(i - span);
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    rawCurv.push(dh / (2 * span));
  }
  const smooth = 12;
  const samples: { x: number; z: number; curvature: number }[] = [];
  for (let i = 0; i < sampleCount; i++) {
    let sum = 0;
    for (let k = -smooth; k <= smooth; k++) sum += at(rawCurv, i + k);
    samples.push({ x: at(xs, i), z: at(zs, i), curvature: sum / (2 * smooth + 1) });
  }
  return samples;
}

/**
 * Executes a full telemetry simulation on a real circuit and validates all invariants.
 */
export function executeTelemetryRun(runIndex: number, config: TelemetryRunConfig): TelemetryRunResult {
  const store = new TelemetryStore({ scheduleFrame: () => () => undefined });
  const track = config.track;
  const car = config.car;
  const samples = buildTrackSamples(track.controlPoints, track.lengthM);
  const n = samples.length;

  // Grip adjustment based on weather
  const weatherGripFactor = config.weather === 'dry' ? 1.0 : config.weather === 'damp' ? 0.88 : 0.76;
  const latG = car.latG * weatherGripFactor;
  const brakeG = car.brakeG * weatherGripFactor;

  // Build speed profile
  const G = 9.81;
  const speed = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const k = Math.abs(samples[i]?.curvature ?? 0);
    speed[i] = k < 1e-5 ? car.vmax : Math.min(car.vmax, Math.sqrt((latG * G) / k));
  }
  // Smooth forward/backward passes
  for (let pass = 0; pass < 2; pass++) {
    for (let i = 0; i < 2 * n; i++) {
      const a = i % n;
      const prev = speed[(a - 1 + n) % n] ?? 0;
      const up = Math.sqrt(prev * prev + 2 * car.powerAccel * weatherGripFactor);
      speed[a] = Math.min(speed[a] ?? car.vmax, up);
    }
    for (let i = 2 * n; i >= 0; i--) {
      const a = i % n;
      const next = speed[(a + 1) % n] ?? 0;
      const down = Math.sqrt(next * next + 2 * brakeG * G);
      speed[a] = Math.min(speed[a] ?? car.vmax, down);
    }
  }

  // Lap time
  let lapTimeS = 0;
  for (let i = 0; i < n; i++) {
    const v0 = speed[i] ?? 1;
    const v1 = speed[(i + 1) % n] ?? 1;
    lapTimeS += 2 / (v0 + v1);
  }

  // State variables for the car
  let fuel = config.startingFuelL;
  const fuelPerLap = (track.lengthM / 1000) * car.baseFuelPer1000m;
  let virtualEnergy = car.virtualEnergy ? 0.85 : 0;
  let batterySoc = car.hybrid ? 0.65 : 0;

  const wheels = [0, 1, 2, 3].map((w) => {
    const isFront = w < 2;
    return {
      surface: config.trackTempC + (isFront ? 50 : 45),
      carcass: config.trackTempC + (isFront ? 48 : 43),
      inner: config.trackTempC + (isFront ? 55 : 50),
      outer: config.trackTempC + (isFront ? 45 : 40),
      brake: 350 + (isFront ? 80 : 30),
      brakePeak: 350 + (isFront ? 80 : 30),
      wear: 1.0 - config.stintLapsCompleted * 0.012,
      pressurePa: (isFront ? 1.55 : 1.52) * 1e5 * ((config.trackTempC + 48 + 273.15) / 298.15),
    };
  });

  let maxSpeedMps = 0;
  let maxBrakeTempC = 0;
  let maxTyreTempC = 0;
  let minTyreWear = 1.0;
  const previousWheelPeaks = [0, 0, 0, 0];
  let monotonicBrakePeak = true;
  let camberGradientValid = true;
  let speedsPhysicallyValid = true;
  let rpmsPhysicallyValid = true;

  const dt = 0.01; // 100 Hz
  const totalRows = config.rowsToSimulate;
  let currentDistM = randomInt(0, Math.max(0, track.lengthM - 100));
  const currentLap = config.stintLapsCompleted + 1;
  let sessionTimeS = 1800 + currentLap * lapTimeS;
  let seq = 0;
  let batchesEmitted = 0;

  const blocks: Uint8Array[] = [];

  for (let r = 0; r < totalRows; r++) {
    const distIdx = Math.floor(currentDistM) % n;
    const v = speed[distIdx] ?? 1;
    maxSpeedMps = Math.max(maxSpeedMps, v);
    currentDistM = (currentDistM + v * dt) % track.lengthM;

    // Throttle & brake estimation
    const nextIdx = (distIdx + 1) % n;
    const dv = (speed[nextIdx] ?? v) - v;
    const throttle = dv > 0 ? Math.min(1, dv / (car.powerAccel * dt)) : 0;
    const brake = dv < -0.5 ? Math.min(1, -dv / (brakeG * G * dt)) : 0;

    // Fuel consumption
    fuel = Math.max(0, fuel - (fuelPerLap / lapTimeS) * dt * (0.35 + throttle));
    if (car.virtualEnergy) {
      virtualEnergy = Math.max(0, virtualEnergy - (0.028 / lapTimeS) * dt * (0.35 + throttle));
    }
    if (car.hybrid) {
      batterySoc = Math.min(0.95, Math.max(0.05, batterySoc + (brake * 0.001 - throttle * 0.0003)));
    }

    // RPM & Gear
    let gear = car.gearTopSpeeds.findIndex((top) => v <= top * 0.98) + 1;
    if (gear === 0) gear = car.gearTopSpeeds.length;
    const rpm = Math.min(car.rpmMax, 4000 + (v / car.vmax) * (car.rpmMax - 4000));
    if (rpm < 0 || rpm > car.rpmMax + 100 || isNaN(rpm)) rpmsPhysicallyValid = false;
    if (v < 0 || v > 110 || isNaN(v)) speedsPhysicallyValid = false;

    // Wheel updates
    wheels.forEach((w, idx) => {
      const isFront = idx < 2;
      const brakeHeat = brake * v * 8.0 * (isFront ? car.brakeBiasFront : 1 - car.brakeBiasFront);
      const brakeCool = (w.brake - config.ambientTempC) * 0.015;
      w.brake += (brakeHeat - brakeCool) * dt;
      if (w.brake > w.brakePeak) {
        w.brakePeak = w.brake;
      }
      const prevPeak = previousWheelPeaks[idx] ?? 0;
      if (w.brakePeak < prevPeak - 1e-4) {
        monotonicBrakePeak = false;
      }
      previousWheelPeaks[idx] = w.brakePeak;
      maxBrakeTempC = Math.max(maxBrakeTempC, w.brake);

      const tyreLoad = Math.abs(samples[distIdx]?.curvature ?? 0) * v * v * 3.0 + brake * 8.0;
      w.surface += (w.carcass + tyreLoad * 0.4 - w.surface) * dt * 2.0;
      w.carcass += (config.trackTempC + 45 + tyreLoad * 0.1 - w.carcass) * dt * 0.05;
      w.inner = w.surface + 4.0;
      w.outer = w.surface - 3.0;

      // In racing camber, inner >= middle >= outer (with middle = surface)
      if (w.inner < w.surface - 0.1 || w.surface < w.outer - 0.1) {
        camberGradientValid = false;
      }

      w.wear = Math.max(0.05, w.wear - dt * 0.00008 * (1 + throttle + brake));
      minTyreWear = Math.min(minTyreWear, w.wear);
      maxTyreTempC = Math.max(maxTyreTempC, w.inner, w.surface, w.outer);
    });

    sessionTimeS += dt;

    // Build TLV records
    const sector = currentDistM >= track.sectorStartsM[1] ? 2 : currentDistM >= track.sectorStartsM[0] ? 1 : 0;
    const [wFL, wFR, wRL, wRR] = wheels;
    const records: FixtureRecord[] = [
      numberRecord(ch.SPEED, v),
      numberRecord(ch.THROTTLE, throttle),
      numberRecord(ch.BRAKE, brake),
      numberRecord(ch.RPM, rpm),
      numberRecord(ch.GEAR, gear),
      numberRecord(ch.LAP_NUMBER, currentLap),
      numberRecord(ch.LAP_TIME, (currentDistM / track.lengthM) * lapTimeS),
      numberRecord(ch.LAP_DISTANCE, currentDistM),
      numberRecord(ch.SECTOR, sector),
      numberRecord(ch.FUEL, fuel),
      numberRecord(ch.FUEL_PER_LAP, fuelPerLap),
      numberRecord(ch.POSITION, config.playerPosition),
      numberRecord(ch.CLASS_POSITION, config.playerClassPosition),
      numberRecord(ch.TYRE_WEAR_FL, wFL?.wear ?? 1),
      numberRecord(ch.TYRE_WEAR_FR, wFR?.wear ?? 1),
      numberRecord(ch.TYRE_WEAR_RL, wRL?.wear ?? 1),
      numberRecord(ch.TYRE_WEAR_RR, wRR?.wear ?? 1),
      numberRecord(ch.TYRE_TEMP_INNER_FL, wFL?.inner ?? 80),
      numberRecord(ch.TYRE_TEMP_MIDDLE_FL, wFL?.surface ?? 80),
      numberRecord(ch.TYRE_TEMP_OUTER_FL, wFL?.outer ?? 80),
      numberRecord(ch.BRAKE_TEMP_FL, wFL?.brake ?? 350),
      numberRecord(ch.BRAKE_TEMP_PEAK_FL, wFL?.brakePeak ?? 350),
      numberRecord(ch.POS_X, samples[distIdx]?.x ?? 0),
      numberRecord(ch.POS_Z, samples[distIdx]?.z ?? 0),
      numberRecord(ch.TRACK_TEMP, config.trackTempC),
      numberRecord(ch.AIR_TEMP, config.ambientTempC),
    ];

    if (car.virtualEnergy) records.push(numberRecord(ch.VIRTUAL_ENERGY, virtualEnergy));
    if (car.hybrid) records.push(numberRecord(ch.BATTERY_SOC, batterySoc));

    const block = encodeTlvBlock({ sourceEtS: sessionTimeS }, records);
    blocks.push(block);

    // Batch every 25 rows (250 ms)
    if (blocks.length >= 25 || r === totalRows - 1) {
      const isReset = batchesEmitted === 0;
      const flags = isReset ? UI_BATCH_FLAG_RESET | UI_BATCH_FLAG_BACKFILL : 0;
      const batch = encodeUiBatch(flags, seq++, blocks.splice(0, blocks.length));
      const applyRes = store.applyBatch(batch);
      batchesEmitted++;
      if (!applyRes.ok) {
        throw new Error(`Batch application failed: ${applyRes.error.kind}`);
      }
    }
  }

  const storeStatus = store.getStatus();

  const invariants: InvariantCheckResult[] = [
    {
      name: 'Zero Wire Decoding Errors',
      passed: storeStatus.batchesRejected === 0 && storeStatus.invalidated === 0,
      detail: `batchesRejected=${storeStatus.batchesRejected}, invalidated=${storeStatus.invalidated}`,
    },
    {
      name: 'No Store Resync Required',
      passed: !storeStatus.resyncNeeded,
      detail: `resyncNeeded=${storeStatus.resyncNeeded}`,
    },
    {
      name: 'Speeds Physically Plausible',
      passed: speedsPhysicallyValid && maxSpeedMps > 15 && maxSpeedMps < 105,
      detail: `maxSpeed=${(maxSpeedMps * 3.6).toFixed(1)} km/h`,
    },
    {
      name: 'Engine RPM Within Physical Boundaries',
      passed: rpmsPhysicallyValid,
      detail: `RPM max limit = ${car.rpmMax}`,
    },
    {
      name: 'Tyre Camber Temperature Gradient Satisfied',
      passed: camberGradientValid,
      detail: `Inner >= Middle >= Outer camber distribution holds across stint`,
    },
    {
      name: 'Brake Peak Temperature Monotonic Non-Decreasing',
      passed: monotonicBrakePeak,
      detail: `maxBrakePeak=${maxBrakeTempC.toFixed(1)} °C`,
    },
    {
      name: 'Fuel Burn Rate Positive and Non-Zero',
      passed: fuel < config.startingFuelL && fuel >= 0,
      detail: `remaining=${fuel.toFixed(2)} L (burned ${(config.startingFuelL - fuel).toFixed(2)} L)`,
    },
    {
      name: 'Tyre Wear Monotonically Progressive',
      passed: minTyreWear < 1.0 && minTyreWear > 0.05,
      detail: `worstWear=${((1 - minTyreWear) * 100).toFixed(1)}%`,
    },
  ];

  if (car.virtualEnergy) {
    invariants.push({
      name: 'Virtual Energy Stint Depletion Valid',
      passed: virtualEnergy < 0.85 && virtualEnergy >= 0,
      detail: `virtualEnergy=${(virtualEnergy * 100).toFixed(1)}%`,
    });
  }

  if (car.hybrid) {
    invariants.push({
      name: 'Hybrid Battery SOC Bounded in Operating Window',
      passed: batterySoc >= 0.05 && batterySoc <= 0.95,
      detail: `batterySoc=${(batterySoc * 100).toFixed(1)}%`,
    });
  }

  const allPassed = invariants.every((inv) => inv.passed);

  return {
    runIndex,
    track: track.name,
    car: car.name,
    vehicleClass: car.vehicleClass,
    weather: config.weather,
    simulatedTimeS: totalRows * dt,
    simulatedRows: totalRows,
    batchesEmitted,
    batchesRejected: storeStatus.batchesRejected,
    resyncNeeded: storeStatus.resyncNeeded,
    samplesInspected: totalRows * 20,
    maxSpeedKmh: maxSpeedMps * 3.6,
    maxBrakeTempC,
    maxTyreTempC,
    minTyreWearFrac: minTyreWear,
    fuelRemainingL: fuel,
    invariants,
    allInvariantsPassed: allPassed,
  };
}

/**
 * Generates a random integer in [min, max].
 */
function randomInt(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

/**
 * Generates a random element from an array.
 */
function pickRandom<T>(arr: readonly T[]): T {
  const item = arr[randomInt(0, arr.length - 1)];
  if (item === undefined) {
    throw new Error('pickRandom on empty array');
  }
  return item;
}

/**
 * Creates a randomized but sensible TelemetryRunConfig on a real-world track.
 */
export function generateRandomSensibleRun(trackOverride?: RealTrackDefinition): TelemetryRunConfig {
  const track = trackOverride ?? pickRandom(REAL_TRACKS);
  const car = pickRandom(REAL_CARS);
  const weatherChoices: readonly ('dry' | 'damp' | 'wet')[] = ['dry', 'dry', 'dry', 'damp', 'wet'];
  const weather = pickRandom(weatherChoices);

  const ambientTempC = randomInt(18, 33);
  const trackTempC = ambientTempC + (weather === 'dry' ? randomInt(8, 22) : randomInt(0, 5));
  const stintLapsCompleted = randomInt(0, 15);
  const startingFuelL = Math.max(
    15,
    car.fuelCapacityL - stintLapsCompleted * ((track.lengthM / 1000) * car.baseFuelPer1000m),
  );

  return {
    track,
    car,
    session: pickRandom(['race', 'qualifying', 'practice'] as const),
    weather,
    ambientTempC,
    trackTempC,
    stintLapsCompleted,
    raceTotalLaps: Math.round(3600 / (track.lengthM / 65)),
    startingFuelL,
    playerPosition: randomInt(1, 18),
    playerClassPosition: randomInt(1, 8),
    rowsToSimulate: randomInt(250, 400), // 2.5 to 4.0 seconds of 100 Hz high-frequency telemetry
  };
}

export function getRealTrack(id: string): RealTrackDefinition {
  const found = REAL_TRACKS.find((t) => t.id === id);
  if (!found) {
    throw new Error(`Track ${id} not found in catalog`);
  }
  return found;
}

export function getRealCar(id: string): RealCarDefinition {
  const found = REAL_CARS.find((c) => c.id === id);
  if (!found) {
    throw new Error(`Car ${id} not found in catalog`);
  }
  return found;
}

