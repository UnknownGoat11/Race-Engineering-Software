import { describe, expect, it } from 'vitest';
import { CHANNELS, Source, ch } from '../generated/channels';
import { channelDef } from '../telemetry/registry';
import { GRAPH_SOURCE_LABELS, graphSourceText, seriesSourceLabel } from './provenance';

describe('seriesSourceLabel (spec §317)', () => {
  it('labels live LMU channels, calculated channels and recordings', () => {
    // Engine speed is read from LMU shared memory; vehicle speed is calculated from the velocity vector.
    expect(seriesSourceLabel('live', channelDef(ch.RPM))).toBe('Live LMU Telemetry');
    const calculated = CHANNELS.find((d) => d.source === Source.Calculated);
    expect(calculated).toBeDefined();
    expect(seriesSourceLabel('live', calculated)).toBe('Calculated');
    expect(seriesSourceLabel('recording', channelDef(ch.SPEED))).toBe('Session Recording');
    expect(seriesSourceLabel('recording', calculated)).toBe('Session Recording');
  });

  it('never presents generated test data as LMU telemetry or a recording', () => {
    const calculated = CHANNELS.find((d) => d.source === Source.Calculated);
    expect(seriesSourceLabel('live', channelDef(ch.RPM), true)).toBe('Synthetic Test Data');
    expect(seriesSourceLabel('live', calculated, true)).toBe('Synthetic Test Data');
    expect(seriesSourceLabel('recording', channelDef(ch.RPM), true)).toBe('Synthetic Test Data');
  });

  it('labels the application diagnostics and unknown channels honestly', () => {
    const app = CHANNELS.find((d) => d.source === Source.Application);
    if (app !== undefined) expect(seriesSourceLabel('live', app)).toBe(GRAPH_SOURCE_LABELS.application);
    expect(seriesSourceLabel('live', null)).toBe(GRAPH_SOURCE_LABELS.live);
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
