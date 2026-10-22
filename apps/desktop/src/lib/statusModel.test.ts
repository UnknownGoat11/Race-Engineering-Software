import { describe, expect, it } from 'vitest';
import type {
  CoreStatus,
  DropsDto,
  EngineerSlotDto,
  EngineerStatusDto,
  LmuSourceStatus,
  NetworkStatusDto,
  RatesDto,
  RecordingStatusDto,
} from './ipc';
import {
  EMPTY_HISTORY,
  RECONNECT_CRITICAL_MS,
  codeTile,
  deriveDriverTiles,
  deriveEngineerView,
  engineerViewTruth,
  engineersTile,
  isLiveLooking,
  isLiveTruth,
  lmuSourceTruth,
  lmuTile,
  mayShowDriverIdentity,
  nextSince,
  pipelineAttentionDetail,
  pipelineHealth,
  recordStatus,
  recordingTile,
  recordingTruth,
  schemaMismatch,
  staleTag,
  streamingSummary,
  streamingTruth,
  type EngineerView,
  type StatusHistory,
} from './statusModel';

// ---------------------------------------------------------------------------------------------------------
// Builders (made-up names only)
// ---------------------------------------------------------------------------------------------------------

const ZERO_DROPS: DropsDto = {
  reader_overrun: 0,
  pipeline_overrun: 0,
  recorder_overrun: 0,
  net_dropped_obsolete: 0,
  net_events_dropped: 0,
  ui_dropped: 0,
};

interface Overrides {
  origin?: CoreStatus['origin'];
  mode?: CoreStatus['mode'];
  lmu?: Partial<LmuSourceStatus>;
  recording?: Partial<RecordingStatusDto>;
  network?: Partial<NetworkStatusDto>;
  rates?: Partial<RatesDto>;
  drops?: Partial<DropsDto>;
  engineer?: Partial<EngineerStatusDto> | null;
}

function slot(id: number, link = 'healthy', name = 'EngOne'): EngineerSlotDto {
  return { connection_id: id, username: name.toLowerCase(), display_name: name, link, connected_since_unix_s: 1_000 };
}

/** A healthy live LMU driver: LIVE at 100 Hz, recording, streaming connected, no engineers. */
function driver(o: Overrides = {}): CoreStatus {
  return {
    mode: o.mode ?? 'driver',
    origin: o.origin ?? 'lmu',
    lmu: { state: 'LIVE', detail: null, game_version: 1, ...o.lmu },
    session: { session_id: 1, session_type: 'Race', track: 'TrackOne', car: 'CarOne', vehicle_class: 'GT3', started_at_unix_s: 1 },
    recording: {
      state: 'active',
      recording_id: 'r1',
      bytes_written: 123_633_664,
      frames_written: 1_000,
      frames_lost: 0,
      disk_free_bytes: 500e9,
      error: null,
      ...o.recording,
    },
    network: {
      state: 'CONNECTED',
      streaming: true,
      rtt_ms: 38,
      connect_code: 'K7Q2XM',
      code_expires_at_unix_s: 2_000,
      engineers_connected: 0,
      engineers_max: 2,
      degraded_reason: null,
      driver_away: false,
      last_error: null,
      security_issue: false,
      failure: null,
      relogin_required: false,
      engineers: [],
      ...o.network,
    },
    rates: {
      telemetry_read_hz: 100,
      scoring_read_hz: 5,
      frames_sent_hz: 50,
      bytes_sent_per_s: 10_000,
      frames_dropped_total: 0,
      ...o.rates,
    },
    drops: { ...ZERO_DROPS, ...o.drops },
    engineer:
      o.engineer === null || o.engineer === undefined
        ? null
        : {
            phase: 'monitoring',
            driver_name: 'DriverOne',
            ended_reason: null,
            stream_health: 'healthy',
            stream_age_s: 0.1,
            driver_away: false,
            incompatible_schema: false,
            ...o.engineer,
          },
  };
}

function engineer(e: Partial<EngineerStatusDto> = {}, o: Overrides = {}): CoreStatus {
  return driver({ ...o, mode: 'engineer', engineer: e });
}

/** History with the given samples at the given times (drops, ids). */
function historyOf(...entries: [number, Partial<DropsDto>, number[]?][]): StatusHistory {
  return {
    samples: entries.map(([atMs, drops, ids]) => ({ atMs, drops: { ...ZERO_DROPS, ...drops }, engineerIds: ids ?? [] })),
    reconnectingSinceMs: null,
  };
}

const NOW = 1_000_000;

// ---------------------------------------------------------------------------------------------------------
// Source truth
// ---------------------------------------------------------------------------------------------------------

describe('source truth: stale/synthetic/failed/ended never derive as live', () => {
  it('LMU source truth table', () => {
    const table: [Overrides, string][] = [
      [{}, 'live'],
      [{ origin: 'synthetic' }, 'synthetic'],
      [{ lmu: { state: 'STALE' } }, 'stale'],
      [{ lmu: { state: 'PAUSED' } }, 'stale'],
      [{ lmu: { state: 'ERROR', detail: 'boom' } }, 'failed'],
      [{ lmu: { state: 'NOT_RUNNING' } }, 'unavailable'],
      [{ lmu: { state: 'CONNECTING' } }, 'unavailable'],
      [{ lmu: { state: 'UNKNOWN' } }, 'unavailable'],
      [{ lmu: { state: 'NEW_THING' } }, 'unavailable'],
      [{ origin: 'synthetic', lmu: { state: 'STALE' } }, 'stale'],
    ];
    for (const [o, truth] of table) {
      const s = lmuSourceTruth(driver(o));
      expect(s.truth).toBe(truth);
      expect(isLiveTruth(s)).toBe(truth === 'live');
    }
    expect(lmuSourceTruth(null).truth).toBe('unavailable');
  });

  it('an unexpected origin value is never live', () => {
    const odd = { ...driver(), origin: 'replay' } as unknown as CoreStatus;
    expect(lmuSourceTruth(odd).truth).toBe('unavailable');
    expect(isLiveTruth(lmuSourceTruth(odd))).toBe(false);
  });

  it('recording truth table', () => {
    expect(recordingTruth(driver()).truth).toBe('live');
    expect(recordingTruth(driver({ recording: { state: 'failed', error: 'disk full' } }))).toMatchObject({
      truth: 'failed',
      reason: 'disk full',
    });
    expect(recordingTruth(driver({ recording: { state: 'suspended' } })).truth).toBe('failed');
    expect(recordingTruth(driver({ recording: { state: 'off' } })).truth).toBe('unavailable');
    expect(recordingTruth(driver({ origin: 'synthetic' })).truth).toBe('unavailable');
    expect(recordingTruth(null).truth).toBe('unavailable');
  });

  it('streaming truth table', () => {
    expect(streamingTruth(driver()).truth).toBe('live');
    expect(streamingTruth(driver({ network: { state: 'RECONNECTING' } })).truth).toBe('stale');
    expect(streamingTruth(driver({ network: { state: 'STALE' } })).truth).toBe('stale');
    expect(streamingTruth(driver({ network: { state: 'FAILED', failure: 'x' } }))).toMatchObject({
      truth: 'failed',
      reason: 'x',
    });
    expect(streamingTruth(driver({ network: { relogin_required: true } })).truth).toBe('failed');
    expect(streamingTruth(driver({ lmu: { state: 'STALE' } })).truth).toBe('stale');
    expect(streamingTruth(driver({ rates: { frames_sent_hz: 0 } })).truth).toBe('stale');
    expect(streamingTruth(driver({ network: { streaming: false } })).truth).toBe('unavailable');
    expect(streamingTruth(driver({ origin: 'synthetic' })).truth).toBe('unavailable');
    expect(streamingTruth(null).truth).toBe('unavailable');
  });
});

// ---------------------------------------------------------------------------------------------------------
// Engineer view
// ---------------------------------------------------------------------------------------------------------

describe('deriveEngineerView', () => {
  const lmu = 'lmu' as const;

  it('no status → unavailable; phases before monitoring', () => {
    expect(deriveEngineerView({ status: null, origin: lmu }).kind).toBe('unavailable');
    expect(deriveEngineerView({ status: engineer({ phase: 'idle' }), origin: lmu }).kind).toBe('notConnected');
    expect(deriveEngineerView({ status: engineer({ phase: 'not_started' }), origin: lmu }).kind).toBe('notConnected');
    expect(deriveEngineerView({ status: driver(), origin: lmu }).kind).toBe('notConnected');
    expect(deriveEngineerView({ status: engineer({ phase: 'joining' }), origin: lmu }).kind).toBe('joining');
    expect(deriveEngineerView({ status: engineer({ phase: 'awaiting_confirmation' }), origin: lmu }).kind).toBe(
      'awaitingConfirmation',
    );
  });

  it('not signed in while idle', () => {
    const login = { authenticated: false, role: 'engineer' as const, account: null };
    expect(deriveEngineerView({ status: engineer({ phase: 'idle' }), origin: lmu, login }).kind).toBe('notSignedIn');
    // A login problem does not hide a running stream.
    expect(deriveEngineerView({ status: engineer(), origin: lmu, login }).kind).toBe('live');
  });

  it('ended carries the reason', () => {
    const v = deriveEngineerView({
      status: engineer({ phase: 'ended', ended_reason: 'The driver ended the session.' }),
      origin: lmu,
    });
    expect(v).toEqual({ kind: 'ended', reason: 'The driver ended the session.' });
    expect(engineerViewTruth(v)).toBe('ended');
  });

  it('live only for healthy LMU data', () => {
    const v = deriveEngineerView({ status: engineer(), origin: lmu, storeFreshness: 'live' });
    expect(v).toEqual({ kind: 'live', ageS: 0.1, degraded: false });
    expect(isLiveLooking(v)).toBe(true);
    expect(deriveEngineerView({ status: engineer(), origin: lmu, storeFreshness: 'degraded' })).toMatchObject({
      kind: 'live',
      degraded: true,
    });
  });

  it('synthetic is never live-looking (store origin wins over CoreStatus.origin)', () => {
    // Engineer CoreStatus.origin is always lmu (app-core core.rs); the store says synthetic.
    const v = deriveEngineerView({ status: engineer({}, { origin: 'lmu' }), origin: 'synthetic' });
    expect(v.kind).toBe('synthetic');
    expect(isLiveLooking(v)).toBe(false);
    expect(engineerViewTruth(v)).toBe('synthetic');
  });

  it('unknown origin is never live-looking', () => {
    const v = deriveEngineerView({ status: engineer(), origin: null });
    expect(isLiveLooking(v)).toBe(false);
    expect(v.kind).toBe('stale');
  });

  it('stale from the DTO or from store freshness, with degraded_reason', () => {
    const v = deriveEngineerView({
      status: engineer({ stream_health: 'stale', stream_age_s: 6.2 }, { network: { degraded_reason: 'no frames for 3 s' } }),
      origin: lmu,
    });
    expect(v).toEqual({ kind: 'stale', ageS: 6.2, reason: 'no frames for 3 s', synthetic: false });
    expect(isLiveLooking(v)).toBe(false);
    expect(deriveEngineerView({ status: engineer(), origin: lmu, storeFreshness: 'stale' }).kind).toBe('stale');
    expect(
      deriveEngineerView({ status: engineer({ stream_health: 'stale' }), origin: 'synthetic' }),
    ).toMatchObject({ kind: 'stale', synthetic: true });
  });

  it('waiting before the first data', () => {
    expect(deriveEngineerView({ status: engineer({ stream_health: 'no_data' }), origin: lmu }).kind).toBe('waiting');
    expect(deriveEngineerView({ status: engineer(), origin: lmu, storeFreshness: 'noData' }).kind).toBe('waiting');
  });

  it('away with since, beats stale', () => {
    const v = deriveEngineerView({
      status: engineer({ driver_away: true, stream_health: 'stale' }),
      origin: lmu,
      awaySinceMs: 42,
    });
    expect(v).toMatchObject({ kind: 'away', sinceMs: 42 });
    expect(engineerViewTruth(v)).toBe('stale');
    expect(isLiveLooking(v)).toBe(false);
  });

  it('incompatible beats everything while monitoring, with versions from the store', () => {
    const v = deriveEngineerView({ status: engineer({ incompatible_schema: true, driver_away: true }), origin: lmu });
    expect(v).toEqual({ kind: 'incompatible', versions: null });
    const store = { state: 'rejected' as const, origin: lmu, lastRejection: { kind: 'schemaMismatch' as const, found: 2, expected: 1, blockIndex: 0 } };
    expect(deriveEngineerView({ status: engineer(), origin: lmu, store })).toEqual({
      kind: 'incompatible',
      versions: { found: 2, expected: 1 },
    });
    expect(isLiveLooking(v)).toBe(false);
  });

  it('identity only after confirmation', () => {
    const hidden: EngineerView['kind'][] = ['unavailable', 'notSignedIn', 'notConnected', 'joining', 'awaitingConfirmation', 'ended'];
    for (const kind of hidden) {
      expect(mayShowDriverIdentity({ kind } as EngineerView)).toBe(false);
    }
    expect(mayShowDriverIdentity({ kind: 'live', ageS: 0, degraded: false })).toBe(true);
  });

  it('isLiveLooking is true for exactly one kind', () => {
    const all: EngineerView[] = [
      { kind: 'unavailable' },
      { kind: 'notSignedIn' },
      { kind: 'notConnected' },
      { kind: 'joining' },
      { kind: 'awaitingConfirmation' },
      { kind: 'waiting' },
      { kind: 'live', ageS: 0, degraded: false },
      { kind: 'synthetic', ageS: 0 },
      { kind: 'stale', ageS: 1, reason: null, synthetic: false },
      { kind: 'away', sinceMs: 0, ageS: 1, synthetic: false },
      { kind: 'incompatible', versions: null },
      { kind: 'ended', reason: null },
    ];
    expect(all.filter(isLiveLooking).map((v) => v.kind)).toEqual(['live']);
    expect(all.filter((v) => engineerViewTruth(v) === 'live').map((v) => v.kind)).toEqual(['live']);
  });
});

describe('schemaMismatch', () => {
  it('only for a rejected store with a schema mismatch', () => {
    expect(schemaMismatch(null)).toBeNull();
    expect(schemaMismatch({ state: 'receiving', origin: 'lmu', lastRejection: null })).toBeNull();
    expect(schemaMismatch({ state: 'rejected', origin: 'lmu', lastRejection: { kind: 'envelope' } })).toBeNull();
    expect(
      schemaMismatch({ state: 'receiving', origin: 'lmu', lastRejection: { kind: 'schemaMismatch', found: 2, expected: 1 } }),
    ).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------------
// History and pipeline health
// ---------------------------------------------------------------------------------------------------------

describe('recordStatus / nextSince', () => {
  it('keeps one baseline sample before the window and trims older ones', () => {
    let h = EMPTY_HISTORY;
    for (let t = 0; t <= 120_000; t += 10_000) h = recordStatus(h, driver(), t);
    expect(h.samples[0]?.atMs).toBe(60_000);
    expect(h.samples.at(-1)?.atMs).toBe(120_000);
  });
  it('ignores a null status', () => {
    expect(recordStatus(EMPTY_HISTORY, null, 5)).toBe(EMPTY_HISTORY);
  });
  it('tracks reconnecting since', () => {
    let h = recordStatus(EMPTY_HISTORY, driver({ network: { state: 'RECONNECTING' } }), 100);
    h = recordStatus(h, driver({ network: { state: 'RECONNECTING' } }), 900);
    expect(h.reconnectingSinceMs).toBe(100);
    h = recordStatus(h, driver(), 1_000);
    expect(h.reconnectingSinceMs).toBeNull();
  });
  it('nextSince', () => {
    expect(nextSince(null, true, 5)).toBe(5);
    expect(nextSince(3, true, 5)).toBe(3);
    expect(nextSince(3, false, 5)).toBeNull();
  });
});

describe('pipelineHealth: any drop → not healthy; nothing running → not healthy', () => {
  it('no status → unavailable, never 0', () => {
    const p = pipelineHealth(null, EMPTY_HISTORY, NOW);
    expect(p.state).toBe('unavailable');
    expect(p.dropsInWindow).toBeNull();
    expect(p.totals).toBeNull();
    expect(p.word).not.toBe('OK');
  });
  it('idle core (lmu UNKNOWN, zeroed counters) → not running', () => {
    const p = pipelineHealth(driver({ lmu: { state: 'UNKNOWN' } }), EMPTY_HISTORY, NOW);
    expect(p.state).toBe('not_running');
    expect(p.level).toBe('off');
    expect(p.totals).toBeNull();
  });
  it('zero drops → healthy', () => {
    expect(pipelineHealth(driver(), EMPTY_HISTORY, NOW).state).toBe('healthy');
  });
  it.each([
    ['reader_overrun'],
    ['pipeline_overrun'],
    ['recorder_overrun'],
    ['net_dropped_obsolete'],
    ['net_events_dropped'],
    ['ui_dropped'],
  ] as const)('a %s drop is never healthy', (key) => {
    const s = driver({ drops: { [key]: 1 } });
    expect(pipelineHealth(s, EMPTY_HISTORY, NOW).state).not.toBe('healthy');
    const h = historyOf([NOW - 30_000, {}]);
    expect(pipelineHealth(s, h, NOW)).toMatchObject({ state: 'dropping', level: 'warning', dropsInWindow: 1 });
  });
  it('1 208 recorder drops within the window → sustained (critical)', () => {
    const h = historyOf([NOW - 61_000, {}], [NOW - 30_000, { recorder_overrun: 100 }]);
    const p = pipelineHealth(driver({ drops: { recorder_overrun: 1_208 } }), h, NOW);
    expect(p).toMatchObject({ state: 'dropping_sustained', level: 'critical', dropsInWindow: 1_208 });
  });
  it('old drops with a full quiet window → healthy, totals kept', () => {
    const h = historyOf([NOW - 70_000, { net_dropped_obsolete: 412 }], [NOW - 10_000, { net_dropped_obsolete: 412 }]);
    const p = pipelineHealth(driver({ drops: { net_dropped_obsolete: 412 } }), h, NOW);
    expect(p.state).toBe('healthy');
    expect(p.totals?.net_dropped_obsolete).toBe(412);
  });
  it('old drops but the history is shorter than the window → still warning', () => {
    const h = historyOf([NOW - 5_000, { net_dropped_obsolete: 412 }]);
    expect(pipelineHealth(driver({ drops: { net_dropped_obsolete: 412 } }), h, NOW).state).toBe('dropping');
  });
  it('a counter reset counts the new value as new drops', () => {
    const h = historyOf([NOW - 70_000, { ui_dropped: 50 }]);
    expect(pipelineHealth(driver({ drops: { ui_dropped: 3 } }), h, NOW)).toMatchObject({ dropsInWindow: 3 });
  });
});

// ---------------------------------------------------------------------------------------------------------
// Driver tiles (REDESIGN §3.1 table; ux audit §2 / domain audit §3 scenarios)
// ---------------------------------------------------------------------------------------------------------

const ALL_TEXT = (t: { word: string; detail: string | null; reason: string | null }): string =>
  `${t.word} ${t.detail ?? ''} ${t.reason ?? ''}`;

describe('no status → "—", never 0', () => {
  it('every tile', () => {
    const tiles = deriveDriverTiles(null, EMPTY_HISTORY, { nowMs: NOW });
    for (const t of [tiles.lmu, tiles.recording, tiles.streaming, tiles.engineers, tiles.code]) {
      expect(t.word).toBe('—');
      expect(t.level).toBe('off');
      expect(ALL_TEXT(t)).not.toMatch(/\b0\b/);
    }
    expect(tiles.code.code).toBeNull();
    expect(tiles.pipeline.state).toBe('unavailable');
    expect(tiles.attention).toEqual([]);
  });
});

describe('LMU tile', () => {
  it('live at nominal rate', () => {
    expect(lmuTile(driver())).toMatchObject({ level: 'ok', word: 'LIVE', detail: '100 Hz' });
  });
  it('low rate is a warning; rate is measured, not hard-coded', () => {
    expect(lmuTile(driver({ rates: { telemetry_read_hz: 61 } }))).toMatchObject({ level: 'warning', detail: '61 Hz' });
    expect(lmuTile(driver({ rates: { telemetry_read_hz: null } })).detail).toBe('—');
    expect(lmuTile(driver({ rates: { telemetry_read_hz: 45 } }), 50).level).toBe('ok');
  });
  it('synthetic reads DEMO (magenta), never LIVE', () => {
    const t = lmuTile(driver({ origin: 'synthetic' }));
    expect(t).toMatchObject({ level: 'synthetic', word: 'DEMO' });
    expect(ALL_TEXT(t)).not.toMatch(/LIVE/);
  });
  it.each([
    ['STALE', 'critical'],
    ['ERROR', 'critical'],
    ['PAUSED', 'warning'],
    ['CONNECTING', 'warning'],
    ['NOT_RUNNING', 'off'],
    ['UNKNOWN', 'off'],
  ] as const)('%s → %s, no raw enum', (state, level) => {
    const t = lmuTile(driver({ lmu: { state } }));
    expect(t.level).toBe(level);
    expect(t.word).not.toMatch(/_/);
    expect(t.word).not.toBe('LIVE');
  });
});

describe('Recording tile', () => {
  it('active, plenty of disk', () => {
    const t = recordingTile(driver());
    expect(t).toMatchObject({ level: 'ok', word: 'REC' });
    expect(t.detail).toContain('117.9 MiB');
  });
  it('disk bands', () => {
    expect(recordingTile(driver({ recording: { disk_free_bytes: 19e9 } })).level).toBe('warning');
    expect(recordingTile(driver({ recording: { disk_free_bytes: 4e9 } })).level).toBe('critical');
  });
  it('recorder drop in the last 60 s is critical; unknown-time loss is a warning', () => {
    const h = historyOf([NOW - 61_000, {}]);
    expect(recordingTile(driver({ drops: { recorder_overrun: 5 } }), h, NOW).level).toBe('critical');
    expect(recordingTile(driver({ drops: { recorder_overrun: 5 } }), EMPTY_HISTORY, NOW).level).toBe('warning');
    const quiet = historyOf([NOW - 61_000, { recorder_overrun: 5 }]);
    expect(recordingTile(driver({ drops: { recorder_overrun: 5 } }), quiet, NOW).level).toBe('ok');
  });
  it('failed and suspended are critical with an action', () => {
    expect(recordingTile(driver({ recording: { state: 'failed', error: 'Disk full' } }))).toMatchObject({
      level: 'critical',
      word: 'FAILED',
      reason: 'Disk full',
      action: 'open_recordings',
    });
    expect(recordingTile(driver({ recording: { state: 'suspended' } }))).toMatchObject({ level: 'critical', word: 'SUSPENDED' });
  });
  it('off / disabled / demo are neutral', () => {
    expect(recordingTile(driver({ recording: { state: 'off' } }))).toMatchObject({ level: 'off', word: 'OFF' });
    expect(recordingTile(driver({ recording: { state: 'disabled' } }))).toMatchObject({ level: 'off', word: 'OFF IN SETTINGS' });
    expect(recordingTile(driver({ origin: 'synthetic' })).level).toBe('off');
  });
});

describe('Streaming tile', () => {
  it('connected, low RTT → ON', () => {
    expect(streamingSummary(driver())).toMatchObject({ level: 'ok', word: 'ON', detail: 'RTT 38 ms' });
  });
  it('reconnecting never reads ON or a rate', () => {
    const t = streamingSummary(driver({ network: { state: 'RECONNECTING', degraded_reason: 'attempt 3' } }), EMPTY_HISTORY, NOW);
    expect(t.word).toBe('RECONNECTING');
    expect(t.level).toBe('warning');
    expect(ALL_TEXT(t)).not.toMatch(/\bON\b|Hz/);
    expect(t.reason).toContain('attempt 3');
    expect(t.reason).toContain('Recording continues');
  });
  it('reconnecting ≥ 30 s is critical', () => {
    const h: StatusHistory = { samples: [], reconnectingSinceMs: NOW - RECONNECT_CRITICAL_MS };
    expect(streamingSummary(driver({ network: { state: 'RECONNECTING' } }), h, NOW).level).toBe('critical');
  });
  it('stale LMU while streaming → IDLE (no source), amber', () => {
    const t = streamingSummary(driver({ lmu: { state: 'STALE' } }));
    expect(t).toMatchObject({ level: 'warning', word: 'IDLE' });
    expect(t.reason).toMatch(/No LMU data/);
  });
  it('0 frames/s while connected → IDLE', () => {
    expect(streamingSummary(driver({ rates: { frames_sent_hz: 0 } })).word).toBe('IDLE');
  });
  it('RTT bands', () => {
    expect(streamingSummary(driver({ network: { rtt_ms: 200 } })).level).toBe('warning');
    expect(streamingSummary(driver({ network: { rtt_ms: 500 } })).level).toBe('critical');
    expect(streamingSummary(driver({ network: { rtt_ms: null } }))).toMatchObject({ level: 'ok', detail: '—' });
  });
  it('relogin → critical with Sign in again action and the backend reason', () => {
    const t = streamingSummary(
      driver({ network: { state: 'FAILED', relogin_required: true, failure: 'Login expired. Log in again to stream.' } }),
    );
    expect(t).toMatchObject({ level: 'critical', word: 'FAILED', action: 'sign_in', reason: 'Login expired. Log in again to stream.' });
  });
  it('failed shows the reason', () => {
    expect(streamingSummary(driver({ network: { state: 'FAILED', failure: null, last_error: 'TLS error' } })).reason).toBe('TLS error');
  });
  it('off / not set up / demo / connecting', () => {
    expect(streamingSummary(driver({ network: { state: 'DISCONNECTED', streaming: false } }))).toMatchObject({ level: 'off', word: 'OFF' });
    expect(streamingSummary(driver({ network: { state: 'NOT_CONFIGURED', streaming: false } }))).toMatchObject({ level: 'off', word: 'NOT SET UP' });
    // Demo: "—" and no reason line; the demo band says it once (C04).
    expect(streamingSummary(driver({ origin: 'synthetic', network: { state: 'NOT_CONFIGURED', streaming: false } }))).toMatchObject({
      level: 'off',
      word: '—',
      reason: null,
    });
    expect(streamingSummary(driver({ network: { state: 'CONNECTING' } }))).toMatchObject({ level: 'warning', word: 'CONNECTING' });
    expect(streamingSummary(driver({ network: { state: 'AUTHENTICATING' } }))).toMatchObject({ level: 'warning', word: 'SIGNING IN' });
  });
});

describe('Engineers tile', () => {
  it('0 / 2 while streaming is neutral "waiting" (C18)', () => {
    expect(engineersTile(driver())).toMatchObject({ level: 'off', word: '0 / 2', reason: 'Waiting for engineer' });
  });
  it('all healthy → ok, with friendly link labels', () => {
    const t = engineersTile(driver({ network: { engineers_connected: 1, engineers: [slot(7)] } }));
    expect(t).toMatchObject({ level: 'ok', word: '1 / 2' });
    expect(t.detail).toBe('EngOne · Connected');
    expect(t.detail).not.toMatch(/healthy/);
  });
  it('weak link → warning', () => {
    const t = engineersTile(driver({ network: { engineers_connected: 1, engineers: [slot(7, 'reconnecting')] } }));
    expect(t.level).toBe('warning');
  });
  it('an engineer dropped in the last 60 s → warning', () => {
    const h = historyOf([NOW - 30_000, {}, [7, 8]]);
    const t = engineersTile(driver({ network: { engineers_connected: 1, engineers: [slot(8)] } }), h, NOW);
    expect(t.level).toBe('warning');
  });
  it('invalid max is not invented', () => {
    expect(engineersTile(driver({ network: { engineers_max: 0 } })).word).toBe('0 / —');
  });
});

describe('Code tile', () => {
  it('code with countdown', () => {
    const t = codeTile(driver(), 2_000 - 522);
    expect(t).toMatchObject({ level: 'ok', word: 'K7Q2XM', code: 'K7Q2XM', expiresInS: 522, sessionFull: false });
    expect(t.reason).toContain('8:42');
  });
  it('expiring soon → warning; expired → no code', () => {
    expect(codeTile(driver(), 1_970).level).toBe('warning');
    expect(codeTile(driver(), 2_001)).toMatchObject({ code: null, word: 'EXPIRED' });
  });
  it('no code → empty slots, never "NO CODE" as a value', () => {
    const t = codeTile(driver({ network: { connect_code: null, streaming: false, state: 'DISCONNECTED' } }), 0);
    expect(t).toMatchObject({ code: null, word: '—', reason: 'Start streaming to get a code' });
  });
  it('session full (2/2)', () => {
    const t = codeTile(driver({ network: { engineers_connected: 2, engineers: [slot(1), slot(2)] } }), 0);
    expect(t).toMatchObject({ sessionFull: true, code: null, word: 'SESSION FULL' });
    expect(t.reason).toContain('2/2');
  });
  it('demo has no code', () => {
    expect(codeTile(driver({ origin: 'synthetic' }), 0).code).toBeNull();
  });
});

describe('deriveDriverTiles (scenario rows)', () => {
  it('healthy live streaming has no attention rows', () => {
    const tiles = deriveDriverTiles(driver(), historyOf([NOW - 61_000, {}]), { nowMs: NOW, nowUnixS: 1_000 });
    expect(tiles.attention).toEqual([]);
    expect(tiles.pipeline.state).toBe('healthy');
  });
  it('recording failed (disk full): critical row first, pipeline not healthy', () => {
    const tiles = deriveDriverTiles(
      driver({ recording: { state: 'failed', error: 'Disk full' }, drops: { recorder_overrun: 1_208 }, network: { rtt_ms: 200 } }),
      historyOf([NOW - 61_000, {}]),
      { nowMs: NOW, nowUnixS: 1_000 },
    );
    expect(tiles.attention[0]?.level).toBe('critical');
    expect(tiles.attention.map((a) => a.id)).toEqual(expect.arrayContaining(['recording', 'pipeline', 'streaming']));
    expect(tiles.pipeline.state).not.toBe('healthy');
    const levels = tiles.attention.map((a) => a.level);
    expect(levels.indexOf('warning')).toBeGreaterThan(levels.lastIndexOf('critical'));
  });
  it('demo: DEMO tile, nothing live-looking, no streaming/recording claims', () => {
    const tiles = deriveDriverTiles(
      driver({ origin: 'synthetic', recording: { state: 'disabled' }, network: { state: 'NOT_CONFIGURED', streaming: false } }),
      EMPTY_HISTORY,
      { nowMs: NOW },
    );
    expect(tiles.lmu.word).toBe('DEMO');
    for (const t of [tiles.recording, tiles.streaming, tiles.engineers, tiles.code]) {
      expect(t.level).not.toBe('ok');
      // C04: no repeated "Not available in demo." reason line in every tile; the value is "—".
      expect(t.reason).toBeNull();
      expect(t.word).toBe('—');
    }
  });
  it('a partial history never prints "0 dropped" in the pipeline warning (U12)', () => {
    // History shorter than the window, no new drop since it started: the 412 earlier drops have no known time.
    const tiles = deriveDriverTiles(
      driver({ drops: { net_dropped_obsolete: 412 } }),
      historyOf([NOW - 5_000, { net_dropped_obsolete: 412 }]),
      { nowMs: NOW, nowUnixS: 1_000 },
    );
    const row = tiles.attention.find((a) => a.id === 'pipeline');
    expect(row?.level).toBe('warning');
    expect(row?.detail).toBe('412 dropped since start (timing unknown)');
    expect(row?.detail).not.toMatch(/\b0 dropped/);
  });
  it('new drops inside the window keep the window wording', () => {
    const tiles = deriveDriverTiles(driver({ drops: { ui_dropped: 3 } }), historyOf([NOW - 61_000, {}]), {
      nowMs: NOW,
      nowUnixS: 1_000,
    });
    expect(tiles.attention.find((a) => a.id === 'pipeline')?.detail).toBe('3 dropped in the last 60 s');
  });
});

describe('pipelineAttentionDetail', () => {
  it('is null when nothing was dropped at all', () => {
    expect(pipelineAttentionDetail(pipelineHealth(driver(), EMPTY_HISTORY, NOW))).toBeNull();
  });
});

describe('staleTag', () => {
  it('formats the age', () => {
    expect(staleTag(6.2)).toBe('STALE 6.2 s');
    expect(staleTag(null)).toBe('STALE');
  });
});
