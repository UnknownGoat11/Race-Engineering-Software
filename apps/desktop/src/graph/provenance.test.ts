import { describe, expect, it } from 'vitest';
import { CHANNELS, Source, ch } from '../generated/channels';
import { channelDef } from '../telemetry/registry';
import { GRAPH_SOURCE_LABELS, checkDataKind, graphSourceText, isLmuData, seriesSourceLabel } from './provenance';

describe('seriesSourceLabel (spec §317)', () => {
  it('labels live LMU channels, calculated channels and recordings', () => {
    // Engine speed is read from LMU shared memory; vehicle speed is calculated from the velocity vector.
    expect(seriesSourceLabel('live', channelDef(ch.RPM), 'lmu')).toBe('Live LMU Telemetry');
    const calculated = CHANNELS.find((d) => d.source === Source.Calculated);
    expect(calculated).toBeDefined();
    expect(seriesSourceLabel('live', calculated, 'lmu')).toBe('Calculated');
    expect(seriesSourceLabel('recording', channelDef(ch.SPEED), 'lmu')).toBe('Session Recording');
    expect(seriesSourceLabel('recording', calculated, 'lmu')).toBe('Session Recording');
  });

  it('never presents generated test data as LMU telemetry or a recording', () => {
    const calculated = CHANNELS.find((d) => d.source === Source.Calculated);
    expect(seriesSourceLabel('live', channelDef(ch.RPM), 'synthetic')).toBe('Synthetic Test Data');
    expect(seriesSourceLabel('live', calculated, 'synthetic')).toBe('Synthetic Test Data');
    expect(seriesSourceLabel('recording', channelDef(ch.RPM), 'synthetic')).toBe('Synthetic Test Data');
  });

  it('fails safe: a source that does not state exactly "lmu" is never labelled as LMU data (silent-failure H2)', () => {
    for (const kind of [undefined, null, '', 'LMU', 'replay', true]) {
      for (const origin of ['live', 'recording'] as const) {
        const label = seriesSourceLabel(origin, channelDef(ch.RPM), kind);
        expect(label).toBe(GRAPH_SOURCE_LABELS.unverified);
        expect(label).not.toMatch(/LMU Telemetry|Session Recording/);
      }
    }
    expect(isLmuData({ dataKind: 'lmu' })).toBe(true);
    expect(isLmuData({ dataKind: 'synthetic' })).toBe(false);
    expect(isLmuData({})).toBe(false);
  });

  it('rejects a missing or unknown data kind when a source is built', () => {
    expect(checkDataKind('lmu')).toBe('lmu');
    expect(checkDataKind('synthetic')).toBe('synthetic');
    expect(() => checkDataKind(undefined)).toThrow(TypeError);
    expect(() => checkDataKind('replay')).toThrow(TypeError);
  });

  it('labels the application diagnostics and unknown channels honestly', () => {
    const app = CHANNELS.find((d) => d.source === Source.Application);
    if (app !== undefined) expect(seriesSourceLabel('live', app, 'lmu')).toBe(GRAPH_SOURCE_LABELS.application);
    expect(seriesSourceLabel('live', null, 'lmu')).toBe(GRAPH_SOURCE_LABELS.live);
  });
});

describe('graphSourceText', () => {
  it('joins the distinct labels in order', () => {
    expect(graphSourceText(['Live LMU Telemetry', 'Calculated', 'Live LMU Telemetry'])).toBe(
      'Source: Live LMU Telemetry · Calculated',
    );
    expect(graphSourceText([])).toBe('');
  });
});
