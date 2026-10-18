import { describe, expect, it } from 'vitest';
import { COMMAND_NAMES, DEMO_SCENARIOS } from './ipc';
import {
  COPY,
  ENGINEER_LINKS,
  ENGINEER_PHASES,
  LMU_STATES,
  NETWORK_STATES,
  RECORDING_STATES,
  STREAM_HEALTHS,
  UNKNOWN_STATE,
  buildProfileLabel,
  demoScenarioLabel,
  endedReasonKind,
  endedReasonLabel,
  engineerLinkLabel,
  engineerPhaseLabel,
  environmentLabel,
  incompatibleCopy,
  joinErrorCopy,
  lmuStateLabel,
  modeLabel,
  networkStateLabel,
  pipelineHealthLabel,
  recordingStateLabel,
  streamHealthLabel,
  vehicleClassCode,
  type StateCopy,
} from './labels';

/** No raw enum text: no underscores anywhere, and the word is upper case. */
function expectClean(c: StateCopy, raw?: string): void {
  for (const text of [c.word, c.label, c.helper ?? '']) {
    expect(text).not.toMatch(/_/);
  }
  expect(c.word).toBe(c.word.toUpperCase());
  expect(c.label.length).toBeGreaterThan(0);
  if (raw?.includes('_') === true) {
    expect(c.word).not.toBe(raw);
    expect(c.label).not.toBe(raw);
  }
}

describe('existing app labels', () => {
  it('are unchanged', () => {
    expect(modeLabel('driver')).toBe('DRIVER');
    expect(modeLabel('engineer')).toBe('ENGINEER');
    expect(environmentLabel('development')).toBe('DEVELOPMENT');
    expect(environmentLabel('staging')).toBe('STAGING');
    expect(environmentLabel('production')).toBe('PRODUCTION');
    expect(buildProfileLabel('debug')).toBe('DEBUG');
    expect(buildProfileLabel('release')).toBe('RELEASE');
  });
});

describe('lmuStateLabel', () => {
  it('maps every state for both origins without raw enum text', () => {
    for (const s of LMU_STATES) {
      for (const origin of ['lmu', 'synthetic', null] as const) {
        const c = lmuStateLabel(s, origin);
        expectClean(c, s);
        expect(c).not.toEqual(UNKNOWN_STATE);
      }
    }
  });
  it('reads DEMO, never LIVE, for synthetic live data', () => {
    expect(lmuStateLabel('LIVE', 'synthetic').word).toBe('DEMO');
    expect(lmuStateLabel('LIVE', 'synthetic').label).not.toMatch(/live/i);
    expect(lmuStateLabel('LIVE', 'lmu').word).toBe('LIVE');
  });
  it('does not claim LMU for an unknown origin', () => {
    expect(lmuStateLabel('LIVE', null).word).not.toBe('LIVE');
  });
  it('maps NOT_RUNNING to readable copy', () => {
    expect(lmuStateLabel('NOT_RUNNING', 'lmu')).toMatchObject({ word: 'NOT RUNNING', label: 'LMU not running' });
  });
  it('does not show unknown raw values', () => {
    expect(lmuStateLabel('SOME_NEW_STATE', 'lmu')).toEqual(UNKNOWN_STATE);
    expect(lmuStateLabel(null, 'lmu')).toEqual(UNKNOWN_STATE);
    expect(lmuStateLabel(undefined, 'lmu')).toEqual(UNKNOWN_STATE);
  });
});

describe('networkStateLabel', () => {
  it('maps every state', () => {
    for (const s of NETWORK_STATES) {
      const c = networkStateLabel(s);
      expectClean(c, s);
      expect(c).not.toEqual(UNKNOWN_STATE);
    }
  });
  it('never says relay', () => {
    for (const s of NETWORK_STATES) {
      const c = networkStateLabel(s);
      expect(`${c.word} ${c.label} ${c.helper ?? ''}`).not.toMatch(/relay/i);
    }
  });
  it('reconnecting reassures that recording continues', () => {
    expect(networkStateLabel('RECONNECTING').helper).toMatch(/Recording continues/);
  });
  it('demo: not available', () => {
    expect(networkStateLabel('NOT_CONFIGURED', { demo: true }).helper).toBe('Not available in demo.');
  });
  it('unknown value', () => {
    expect(networkStateLabel('WHATEVER')).toEqual(UNKNOWN_STATE);
  });
});

describe('recordingStateLabel', () => {
  it('maps every state', () => {
    for (const s of RECORDING_STATES) {
      const c = recordingStateLabel(s);
      expectClean(c, s);
      expect(c).not.toEqual(UNKNOWN_STATE);
    }
  });
  it('suspended is the disk-space protection', () => {
    expect(recordingStateLabel('suspended').helper).toMatch(/[Dd]isk space/);
  });
  it('unknown value', () => {
    expect(recordingStateLabel('paused_by_user')).toEqual(UNKNOWN_STATE);
  });
});

describe('engineer labels', () => {
  it('links', () => {
    for (const l of ENGINEER_LINKS) expectClean(engineerLinkLabel(l), l);
    expect(engineerLinkLabel('healthy').label).toBe('Connected');
    expect(engineerLinkLabel('degraded').label).toBe('Weak link');
    expect(engineerLinkLabel('nope')).toEqual(UNKNOWN_STATE);
  });
  it('phases', () => {
    for (const p of ENGINEER_PHASES) {
      expectClean(engineerPhaseLabel(p), p);
      expect(engineerPhaseLabel(p)).not.toEqual(UNKNOWN_STATE);
    }
    expect(engineerPhaseLabel('x')).toEqual(UNKNOWN_STATE);
  });
  it('stream health: LIVE only for LMU origin', () => {
    for (const h of STREAM_HEALTHS) {
      for (const o of ['lmu', 'synthetic', null] as const) expectClean(streamHealthLabel(h, o), h);
    }
    expect(streamHealthLabel('healthy', 'lmu').word).toBe('LIVE');
    expect(streamHealthLabel('healthy', 'synthetic').word).toBe('SYNTHETIC');
    expect(streamHealthLabel('healthy', null).word).not.toBe('LIVE');
    expect(streamHealthLabel('stale', 'lmu', '6.2 s').word).toBe('STALE 6.2 s');
    expect(streamHealthLabel('stale', 'lmu').word).toBe('STALE');
    expect(streamHealthLabel('no_data', 'lmu').word).toBe('NO DATA');
    expect(streamHealthLabel('odd', 'lmu')).toEqual(UNKNOWN_STATE);
  });
});

describe('ended reasons', () => {
  const MESSAGES: readonly [string, string][] = [
    ['Connection ended. Enter a new code from the driver.', 'ticket_expired'],
    ['The driver disconnected you.', 'kicked'],
    ['The driver ended the session.', 'driver_ended_session'],
    ["The driver's session expired.", 'session_expired'],
    ['Your account connected to this driver from elsewhere.', 'replaced'],
    ['Disconnected.', 'user_disconnected'],
    ['Your login is no longer valid. Log in, then enter a new code from the driver.', 'relogin_required'],
    [
      'This app version is not compatible with the relay. Update the app, then enter a new code from the driver.',
      'incompatible',
    ],
    ['Configuration error. Please check your network and TLS settings.', 'configuration'],
    ['The relay environment does not match this app version.', 'environment_mismatch'],
    ['This account is not authorized as an engineer.', 'not_an_engineer'],
  ];
  it('maps every EngineerEndReason::message() text (crates/transport/src/state.rs)', () => {
    for (const [message, kind] of MESSAGES) {
      expect(endedReasonKind(message)).toBe(kind);
      expect(endedReasonKind(kind)).toBe(kind);
      expectClean(endedReasonLabel(message));
    }
  });
  it('distinguishes driver-ended, kicked and network', () => {
    expect(endedReasonLabel('The driver ended the session.', 'Sam Driver').label).toBe('Session ended by Sam Driver');
    expect(endedReasonLabel('The driver disconnected you.').label).toBe('Removed by the driver');
    expect(endedReasonLabel('Connection ended. Enter a new code from the driver.').word).toBe('DISCONNECTED');
  });
  it('rejoin always needs a new code', () => {
    expect(endedReasonLabel('The driver ended the session.').helper).toMatch(/new code/);
  });
  it('unknown and missing', () => {
    expect(endedReasonKind(null)).toBe('unknown');
    expect(endedReasonKind('something else')).toBe('unknown');
    expect(endedReasonLabel(undefined).label).toBe('Session ended');
  });
});

describe('demoScenarioLabel', () => {
  it('maps every scenario to a friendly name', () => {
    for (const s of DEMO_SCENARIOS) {
      const l = demoScenarioLabel(s);
      expect(l).not.toMatch(/_/);
      expect(l).not.toBe(s);
      expect(l[0]).toBe(l[0]?.toUpperCase());
    }
    expect(demoScenarioLabel('safety_car')).toBe('Safety car');
  });
});

describe('joinErrorCopy', () => {
  it('maps the join error codes with clear-field hints', () => {
    expect(joinErrorCopy('code_expired').clearField).toBe(true);
    expect(joinErrorCopy('code_invalid').clearField).toBe(true);
    expect(joinErrorCopy('session_full').title).toMatch(/2 engineers/);
    expect(joinErrorCopy('rate_limited').clearField).toBe(false);
    expect(joinErrorCopy('role_mismatch').title).toMatch(/engineer account/);
  });
  it('is sentence case and never raw', () => {
    const codes = ['code_malformed', 'code_invalid', 'code_expired', 'session_full', 'rate_limited', 'xyz', null];
    for (const c of codes) {
      const e = joinErrorCopy(c);
      expect(e.title).not.toMatch(/_/);
      expect(e.title).not.toBe(e.title.toUpperCase());
      expect(e.helper.length).toBeGreaterThan(0);
    }
  });
  it('command names are not leaked', () => {
    for (const name of COMMAND_NAMES) expect(joinErrorCopy(name).title).not.toContain(name);
  });
});

describe('pipeline / schema / class', () => {
  it('pipeline states', () => {
    for (const s of ['healthy', 'dropping', 'dropping_sustained', 'not_running', 'unavailable'] as const) {
      expectClean(pipelineHealthLabel(s), s);
    }
    expect(pipelineHealthLabel('not_running').word).not.toBe('OK');
  });
  it('incompatible copy states versions when known', () => {
    expect(incompatibleCopy(2, 1).helper).toBe("The driver's app sends data format v2; this app reads v1.");
    expect(incompatibleCopy(null, null).label).toBe(COPY.updateRequired);
  });
  it('re-exports vehicleClassCode', () => {
    expect(vehicleClassCode('Hypercar')).toBe('HY');
  });
  it('terminology has no banned words', () => {
    const all = Object.values(COPY).join(' | ');
    expect(all).not.toMatch(/\bKick\b|\bSnapshot\b|\bYOU\b|MoTeC|relay/);
  });
});
