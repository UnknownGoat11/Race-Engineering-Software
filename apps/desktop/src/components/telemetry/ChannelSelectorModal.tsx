/**
 * MoTeC i2 Pro Style Channel Selector Modal.
 *
 * Allows race engineers to dynamically configure telemetry channels plotted on the
 * synchronized telemetry graph:
 * - Master channel catalog grouped by category (Inputs, Powertrain, Tyres, Brakes, Suspension, Aero, Energy, etc.)
 * - Instant real-time search filter matching channel export code, friendly label, or unit
 * - Active series tray with slot-based color mapping (slots 1..8)
 * - Move Up / Move Down to reorder slots and palette assignment
 * - Max 8 series hardware guard (WebGL shader slot capacity)
 * - Quick presets (Inputs, Powertrain, Tyres, Brakes) and Clear All
 */
import { useId, useMemo, useState, type JSX } from 'react';
import {
  ALL_CHANNEL_GROUPS,
  CHANNEL_GROUP_TITLES,
  ChannelGroup,
  CHANNELS,
  UNIT_SYMBOLS,
  ch,
  type ChannelDef,
} from '../../generated/channels';
import { channelDef, isNumericChannel } from '../../telemetry/registry';
import { ProvenanceBadge } from './ProvenanceBadge';

export interface ChannelSelectorModalProps {
  readonly isOpen: boolean;
  readonly activeChannelIds: readonly number[];
  readonly onClose: () => void;
  readonly onApply: (selectedChannelIds: readonly number[]) => void;
}

const MAX_PLOT_CHANNELS = 8;

const GROUP_BY_HIGH_BYTE: Readonly<Record<number, ChannelGroup>> = {
  0x00: ChannelGroup.VehicleInputs,
  0x01: ChannelGroup.Powertrain,
  0x02: ChannelGroup.Fuel,
  0x03: ChannelGroup.Tyres,
  0x04: ChannelGroup.Brakes,
  0x05: ChannelGroup.Dynamics,
  0x06: ChannelGroup.HybridEnergy,
  0x07: ChannelGroup.Timing,
  0x08: ChannelGroup.RaceScoring,
  0x09: ChannelGroup.Weather,
  0x0a: ChannelGroup.TrackPosition,
  0x0b: ChannelGroup.FlagsPit,
  0x0c: ChannelGroup.Strategy,
  0x0d: ChannelGroup.Diagnostics,
};

function channelGroupOf(def: ChannelDef): ChannelGroup {
  const highByte = def.id >> 8;
  return GROUP_BY_HIGH_BYTE[highByte] ?? ChannelGroup.VehicleInputs;
}

// Preset definitions for quick selection
const QUICK_PRESETS: Readonly<Record<string, { label: string; channelIds: readonly number[] }>> = {
  inputs: {
    label: 'Inputs',
    channelIds: [ch.SPEED, ch.THROTTLE, ch.BRAKE, ch.STEERING],
  },
  powertrain: {
    label: 'Powertrain',
    channelIds: [ch.RPM, ch.SPEED, ch.FUEL],
  },
  tyres: {
    label: 'Tyres',
    channelIds: [
      ch.TYRE_TEMP_MIDDLE_FL,
      ch.TYRE_TEMP_MIDDLE_FR,
      ch.TYRE_TEMP_MIDDLE_RL,
      ch.TYRE_TEMP_MIDDLE_RR,
    ],
  },
  brakes: {
    label: 'Brakes',
    channelIds: [
      ch.BRAKE_TEMP_FL,
      ch.BRAKE_TEMP_FR,
      ch.BRAKE_TEMP_RL,
      ch.BRAKE_TEMP_RR,
    ],
  },
};

export function ChannelSelectorModal({
  isOpen,
  activeChannelIds,
  onClose,
  onApply,
}: ChannelSelectorModalProps): JSX.Element | null {
  const titleId = useId();
  const [selectedIds, setSelectedIds] = useState<readonly number[]>(() => [...activeChannelIds]);
  const [searchQuery, setSearchQuery] = useState('');
  const [expandedGroups, setExpandedGroups] = useState<ReadonlySet<ChannelGroup>>(
    () => new Set([ChannelGroup.VehicleInputs]),
  );
  const [errorNotice, setErrorNotice] = useState<string | null>(null);

  // Filter numeric channels only
  const numericChannels = useMemo(() => CHANNELS.filter(isNumericChannel), []);

  // Filtered by search query
  const filteredChannels = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return numericChannels;
    return numericChannels.filter((c) => {
      const matchName = c.name.toLowerCase().includes(q);
      const matchExport = c.exportName.toLowerCase().includes(q);
      const matchLabel = c.label.toLowerCase().includes(q);
      const unit = UNIT_SYMBOLS[c.unit];
      const matchUnit = unit.toLowerCase().includes(q);
      return matchName || matchExport || matchLabel || matchUnit;
    });
  }, [numericChannels, searchQuery]);

  // Group channels by category
  const groupedChannels = useMemo(() => {
    const map = new Map<ChannelGroup, ChannelDef[]>();
    for (const group of ALL_CHANNEL_GROUPS) {
      map.set(group, []);
    }
    for (const c of filteredChannels) {
      const g = channelGroupOf(c);
      map.get(g)?.push(c);
    }
    return map;
  }, [filteredChannels]);

  if (!isOpen) return null;

  const isSelected = (id: number): boolean => selectedIds.includes(id);

  const getSlot = (id: number): number | null => {
    const idx = selectedIds.indexOf(id);
    return idx >= 0 ? idx + 1 : null;
  };

  const handleToggleChannel = (id: number) => {
    setErrorNotice(null);
    if (isSelected(id)) {
      setSelectedIds(selectedIds.filter((x) => x !== id));
    } else {
      if (selectedIds.length >= MAX_PLOT_CHANNELS) {
        setErrorNotice(`Maximum ${MAX_PLOT_CHANNELS} channels allowed on the telemetry graph (WebGL series capacity).`);
        return;
      }
      setSelectedIds([...selectedIds, id]);
    }
  };

  const handleRemoveSelected = (id: number) => {
    setErrorNotice(null);
    setSelectedIds(selectedIds.filter((x) => x !== id));
  };

  const handleMoveUp = (index: number) => {
    if (index <= 0) return;
    const next = [...selectedIds];
    const prev = next[index - 1];
    const curr = next[index];
    if (prev !== undefined && curr !== undefined) {
      next[index - 1] = curr;
      next[index] = prev;
      setSelectedIds(next);
    }
  };

  const handleMoveDown = (index: number) => {
    if (index >= selectedIds.length - 1) return;
    const next = [...selectedIds];
    const nxt = next[index + 1];
    const curr = next[index];
    if (nxt !== undefined && curr !== undefined) {
      next[index + 1] = curr;
      next[index] = nxt;
      setSelectedIds(next);
    }
  };

  const handleApplyPreset = (presetKey: string) => {
    const preset = QUICK_PRESETS[presetKey];
    if (preset) {
      setErrorNotice(null);
      setSelectedIds([...preset.channelIds]);
    }
  };

  const handleClearAll = () => {
    setErrorNotice(null);
    setSelectedIds([]);
  };

  const handleToggleGroup = (group: ChannelGroup) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group)) {
        next.delete(group);
      } else {
        next.add(group);
      }
      return next;
    });
  };

  const handleApply = () => {
    if (selectedIds.length === 0) {
      setErrorNotice('Please select at least 1 channel to plot.');
      return;
    }
    onApply(selectedIds);
    onClose();
  };

  return (
    <div
      className="modal-backdrop"
      data-testid="channel-selector-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
    >
      <div
        className="card modal-content channel-selector-modal"
        style={{
          maxWidth: '880px',
          width: '95vw',
          height: 'min(640px, 90vh)',
          maxHeight: '90vh',
          display: 'flex',
          flexDirection: 'column',
          margin: '0 auto',
        }}
      >
        {/* Header */}
        <div className="card-header" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <div>
            <span id={titleId} className="card-header-title">
              MoTeC i2 Pro Channel Selector
            </span>
            <span style={{ fontSize: '11px', color: 'var(--color-text-dim)', marginLeft: '8px' }}>
              Select & order up to {MAX_PLOT_CHANNELS} channels to plot
            </span>
          </div>
          <button
            type="button"
            className="btn btn-xs"
            onClick={() => {
              onClose();
            }}
            aria-label="Close dialog"
            style={{ fontWeight: 700, padding: '2px 8px' }}
          >
            ✕
          </button>
        </div>

        {/* Toolbar & Filter */}
        <div
          style={{
            padding: '10px 14px',
            backgroundColor: 'var(--color-surface)',
            borderBottom: '1px solid var(--color-border)',
            display: 'flex',
            flexWrap: 'wrap',
            gap: '8px',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          {/* Search box */}
          <div style={{ flex: '1 1 240px', display: 'flex', alignItems: 'center', position: 'relative' }}>
            <input
              type="text"
              className="input"
              style={{ width: '100%', padding: '6px 10px', fontSize: '12px' }}
              placeholder="Search channels (e.g. speed, steer, temp, fl, rad)..."
              value={searchQuery}
              onChange={(e) => {
                setSearchQuery(e.target.value);
              }}
              data-testid="channel-search-input"
            />
            {searchQuery && (
              <button
                type="button"
                onClick={() => {
                  setSearchQuery('');
                }}
                style={{
                  position: 'absolute',
                  right: '8px',
                  background: 'transparent',
                  border: 'none',
                  color: 'var(--color-text-dim)',
                  cursor: 'pointer',
                  fontSize: '12px',
                }}
                aria-label="Clear search"
              >
                ✕
              </button>
            )}
          </div>

          {/* Preset Buttons */}
          <div style={{ display: 'flex', gap: '4px', alignItems: 'center' }}>
            <span style={{ fontSize: '11px', color: 'var(--color-text-dim)', textTransform: 'uppercase' }}>
              Presets:
            </span>
            {Object.entries(QUICK_PRESETS).map(([key, p]) => (
              <button
                key={key}
                type="button"
                className="btn btn-xs"
                onClick={() => {
                  handleApplyPreset(key);
                }}
                data-testid={`preset-${key}-btn`}
              >
                {p.label}
              </button>
            ))}
            <button
              type="button"
              className="btn btn-xs"
              style={{ color: 'var(--color-error)' }}
              onClick={() => {
                handleClearAll();
              }}
              data-testid="clear-all-channels-btn"
            >
              Clear
            </button>
          </div>
        </div>

        {/* Error / Notice Alert */}
        {errorNotice && (
          <div
            className="status-badge status-badge-warn"
            style={{ margin: '8px 14px 0 14px', padding: '6px 10px', display: 'block', textAlign: 'left' }}
            role="alert"
          >
            {errorNotice}
          </div>
        )}

        {/* Modal Main Body (2 Columns: Catalog Tree + Active Series Tray) */}
        <div style={{ flex: 1, minHeight: 0, display: 'flex', overflow: 'hidden' }}>
          {/* Left: Channel Catalog */}
          <div
            style={{
              flex: 1,
              minWidth: 0,
              overflowY: 'auto',
              padding: '10px 14px',
              borderRight: '1px solid var(--color-border)',
            }}
            data-testid="channel-catalog-list"
          >
            {ALL_CHANNEL_GROUPS.map((group) => {
              const channelsInGroup = groupedChannels.get(group) ?? [];
              if (channelsInGroup.length === 0) return null;
              const isExpanded = searchQuery.length > 0 || expandedGroups.has(group);
              const groupSelectedCount = channelsInGroup.filter((c) => isSelected(c.id)).length;

              return (
                <div key={group} style={{ marginBottom: '8px' }}>
                  {/* Category Header */}
                  <div
                    onClick={() => {
                      handleToggleGroup(group);
                    }}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      padding: '6px 8px',
                      backgroundColor: 'var(--color-surface-raised)',
                      border: '1px solid var(--color-border)',
                      borderRadius: '3px',
                      cursor: 'pointer',
                      userSelect: 'none',
                      fontSize: '11px',
                      fontWeight: 700,
                      letterSpacing: '0.5px',
                    }}
                    role="button"
                    tabIndex={0}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        handleToggleGroup(group);
                      }
                    }}
                    aria-expanded={isExpanded}
                  >
                    <span style={{ color: 'var(--color-text-dim)' }}>
                      {isExpanded ? '▼' : '▶'} {CHANNEL_GROUP_TITLES[group].toUpperCase()}
                    </span>
                    <span style={{ display: 'flex', gap: '6px', alignItems: 'center' }}>
                      {groupSelectedCount > 0 && (
                        <span
                          className="status-badge status-badge-live"
                          style={{ padding: '1px 5px', fontSize: '9px' }}
                        >
                          {groupSelectedCount} active
                        </span>
                      )}
                      <span style={{ color: 'var(--color-text-dim)', fontSize: '10px' }}>
                        ({channelsInGroup.length})
                      </span>
                    </span>
                  </div>

                  {/* Channel Items Table */}
                  {isExpanded && (
                    <div style={{ paddingLeft: '4px', paddingTop: '4px' }}>
                      {channelsInGroup.map((def) => {
                        const selected = isSelected(def.id);
                        const slot = getSlot(def.id);
                        const unitSymbol = UNIT_SYMBOLS[def.unit];

                        return (
                          <div
                            key={def.id}
                            onClick={() => {
                              handleToggleChannel(def.id);
                            }}
                            style={{
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'space-between',
                              padding: '5px 8px',
                              marginBottom: '2px',
                              borderRadius: '3px',
                              cursor: 'pointer',
                              backgroundColor: selected
                                ? 'var(--color-selected-bg)'
                                : 'transparent',
                              border: selected
                                ? '1px solid var(--color-accent)'
                                : '1px solid transparent',
                              fontSize: '11px',
                            }}
                            role="checkbox"
                            aria-checked={selected}
                            tabIndex={0}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter' || e.key === ' ') {
                                e.preventDefault();
                                handleToggleChannel(def.id);
                              }
                            }}
                            data-testid={`channel-row-${def.id}`}
                          >
                            <div style={{ display: 'flex', alignItems: 'center', gap: '8px', minWidth: 0 }}>
                              <input
                                type="checkbox"
                                checked={selected}
                                onChange={() => {
                                  handleToggleChannel(def.id);
                                }}
                                tabIndex={-1}
                                aria-hidden="true"
                              />
                              <span
                                style={{
                                  fontFamily: 'var(--font-mono)',
                                  fontWeight: 600,
                                  color: selected ? 'var(--color-accent-strong)' : 'var(--color-text)',
                                }}
                              >
                                {def.exportName}
                              </span>
                              <span
                                style={{
                                  color: 'var(--color-text-muted)',
                                  whiteSpace: 'nowrap',
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                }}
                              >
                                {def.label}
                              </span>
                            </div>

                            <div style={{ display: 'flex', alignItems: 'center', gap: '6px', flexShrink: 0 }}>
                              {unitSymbol && (
                                <span
                                  style={{
                                    fontFamily: 'var(--font-mono)',
                                    color: 'var(--color-text-dim)',
                                    fontSize: '10px',
                                  }}
                                >
                                  {unitSymbol}
                                </span>
                              )}
                              <ProvenanceBadge provenance={def.provenance} />
                              {slot !== null && (
                                <span
                                  className="telemetry-graph__swatch"
                                  data-slot={slot}
                                  title={`Plot Series #${slot}`}
                                  style={{ width: '14px', height: '14px', borderRadius: '2px' }}
                                />
                              )}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
          </div>

          {/* Right: Selected Plot Series Tray */}
          <div
            style={{
              width: '280px',
              backgroundColor: 'var(--color-surface-raised)',
              display: 'flex',
              flexDirection: 'column',
              flexShrink: 0,
            }}
            data-testid="selected-series-tray"
          >
            <div
              style={{
                padding: '8px 12px',
                borderBottom: '1px solid var(--color-border)',
                display: 'flex',
                justifyContent: 'space-between',
                alignItems: 'center',
              }}
            >
              <span style={{ fontSize: '11px', fontWeight: 700, textTransform: 'uppercase', color: 'var(--color-text-dim)' }}>
                PLOTTED SERIES
              </span>
              <span
                className={`status-badge ${selectedIds.length === MAX_PLOT_CHANNELS ? 'status-badge-warn' : 'status-badge-live'}`}
                style={{ fontSize: '10px', padding: '1px 6px' }}
                data-testid="selected-channel-count"
              >
                {selectedIds.length} / {MAX_PLOT_CHANNELS}
              </span>
            </div>

            <div style={{ flex: 1, overflowY: 'auto', padding: '8px' }}>
              {selectedIds.length === 0 ? (
                <div style={{ padding: '24px 12px', textAlign: 'center', color: 'var(--color-text-dim)', fontSize: '11px' }}>
                  No channels selected.<br />Select channels from the catalog on the left or click a preset above.
                </div>
              ) : (
                selectedIds.map((id, index) => {
                  const def = channelDef(id);
                  const slot = index + 1;
                  const unitSymbol = def ? UNIT_SYMBOLS[def.unit] : '';

                  return (
                    <div
                      key={id}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                        padding: '6px 8px',
                        marginBottom: '4px',
                        backgroundColor: 'var(--color-surface)',
                        border: '1px solid var(--color-border)',
                        borderRadius: '3px',
                        fontSize: '11px',
                      }}
                      data-testid={`selected-item-${id}`}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: '6px', minWidth: 0 }}>
                        <span
                          className="telemetry-graph__swatch"
                          data-slot={slot}
                          title={`Color slot ${slot}`}
                          style={{ width: '12px', height: '12px', borderRadius: '2px', flexShrink: 0 }}
                        />
                        <span
                          style={{
                            fontFamily: 'var(--font-mono)',
                            fontWeight: 600,
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {def?.exportName ?? `Ch #${id}`}
                        </span>
                        {unitSymbol && (
                          <span style={{ color: 'var(--color-text-dim)', fontSize: '10px' }}>
                            ({unitSymbol})
                          </span>
                        )}
                      </div>

                      <div style={{ display: 'flex', alignItems: 'center', gap: '2px', flexShrink: 0 }}>
                        <button
                          type="button"
                          className="btn btn-xs"
                          onClick={() => {
                            handleMoveUp(index);
                          }}
                          disabled={index === 0}
                          title="Move up (change color slot)"
                          aria-label={`Move ${def?.exportName ?? id} up`}
                          style={{ padding: '0 4px', height: '18px' }}
                        >
                          ▲
                        </button>
                        <button
                          type="button"
                          className="btn btn-xs"
                          onClick={() => {
                            handleMoveDown(index);
                          }}
                          disabled={index === selectedIds.length - 1}
                          title="Move down (change color slot)"
                          aria-label={`Move ${def?.exportName ?? id} down`}
                          style={{ padding: '0 4px', height: '18px' }}
                        >
                          ▼
                        </button>
                        <button
                          type="button"
                          className="btn btn-xs"
                          onClick={() => {
                            handleRemoveSelected(id);
                          }}
                          title="Remove from graph"
                          aria-label={`Remove ${def?.exportName ?? id}`}
                          style={{ padding: '0 5px', height: '18px', color: 'var(--color-error)' }}
                        >
                          ✕
                        </button>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          </div>
        </div>

        {/* Footer */}
        <div
          style={{
            padding: '10px 14px',
            backgroundColor: 'var(--color-surface)',
            borderTop: '1px solid var(--color-border)',
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
          }}
        >
          <span style={{ fontSize: '11px', color: 'var(--color-text-dim)' }}>
            Tip: Click swatches or channel labels on the graph legend to toggle traces on/off.
          </span>
          <div style={{ display: 'flex', gap: '8px' }}>
            <button
              type="button"
              className="btn btn-sm"
              onClick={() => {
                onClose();
              }}
              data-testid="channel-selector-cancel-btn"
            >
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-sm btn-primary"
              onClick={() => {
                handleApply();
              }}
              disabled={selectedIds.length === 0}
              data-testid="channel-selector-apply-btn"
            >
              Apply Channels ({selectedIds.length})
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
