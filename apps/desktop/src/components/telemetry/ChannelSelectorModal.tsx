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
 * - Accessible Modal dialog (native <dialog>, showModal, focus trap, Escape, A01, A05)
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
import { Modal } from '../ui/Modal';
import { ProvenanceBadge } from './ProvenanceBadge';
import '../../graph/graph.css';
import '../../styles/modal.css';

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
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      ariaLabelledBy={titleId}
      size="xl"
      testId="channel-selector-modal"
      className="channel-selector-modal-wrapper"
    >
      <div className="card modal-content channel-selector-modal">
        {/* Header */}
        <div className="card-header channel-selector-header">
          <div>
            <span id={titleId} className="card-header-title">
              Choose channels
            </span>
            <span className="channel-selector-subtitle">
              Select & order up to {MAX_PLOT_CHANNELS} channels to plot
            </span>
          </div>
          <button
            type="button"
            className="btn btn-xs channel-selector-close-btn"
            onClick={onClose}
            aria-label="Close dialog"
          >
            ✕
          </button>
        </div>

        {/* Toolbar & Filter */}
        <div className="channel-selector-toolbar">
          {/* Search box */}
          <div className="channel-selector-search-box">
            <input
              type="text"
              className="input channel-selector-search-input"
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
                className="channel-selector-clear-search-btn"
                onClick={() => {
                  setSearchQuery('');
                }}
                aria-label="Clear search"
              >
                ✕
              </button>
            )}
          </div>

          {/* Preset Buttons */}
          <div className="channel-selector-presets-row">
            <span className="channel-selector-presets-label">
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
              className="btn btn-xs channel-selector-clear-btn"
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
            className="status-badge status-badge-warn channel-selector-notice"
            role="alert"
          >
            {errorNotice}
          </div>
        )}

        {/* Modal Main Body (2 Columns: Catalog Tree + Active Series Tray) */}
        <div className="channel-selector-body">
          {/* Left: Channel Catalog */}
          <div className="channel-selector-catalog" data-testid="channel-catalog-list">
            {ALL_CHANNEL_GROUPS.map((group) => {
              const channelsInGroup = groupedChannels.get(group) ?? [];
              if (channelsInGroup.length === 0) return null;
              const isExpanded = searchQuery.length > 0 || expandedGroups.has(group);
              const groupSelectedCount = channelsInGroup.filter((c) => isSelected(c.id)).length;

              return (
                <div key={group} className="channel-selector-group">
                  {/* Category Header */}
                  <div
                    className="channel-selector-group-header"
                    onClick={() => {
                      handleToggleGroup(group);
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
                    <span className="channel-selector-group-title">
                      {isExpanded ? '▼' : '▶'} {CHANNEL_GROUP_TITLES[group].toUpperCase()}
                    </span>
                    <span className="channel-selector-group-meta">
                      {groupSelectedCount > 0 && (
                        <span className="status-badge status-badge-live channel-selector-active-badge">
                          {groupSelectedCount} active
                        </span>
                      )}
                      <span className="channel-selector-count">
                        ({channelsInGroup.length})
                      </span>
                    </span>
                  </div>

                  {/* Channel Items Table */}
                  {isExpanded && (
                    <div className="channel-selector-items">
                      {channelsInGroup.map((def) => {
                        const selected = isSelected(def.id);
                        const slot = getSlot(def.id);
                        const unitSymbol = UNIT_SYMBOLS[def.unit];

                        return (
                          <div
                            key={def.id}
                            className={`channel-selector-row${selected ? ' channel-selector-row--selected' : ''}`}
                            onClick={() => {
                              handleToggleChannel(def.id);
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
                            <div className="channel-selector-row-left">
                              <input
                                type="checkbox"
                                checked={selected}
                                onChange={() => {
                                  handleToggleChannel(def.id);
                                }}
                                tabIndex={-1}
                                aria-hidden="true"
                              />
                              <span className="channel-selector-row-export">
                                {def.exportName}
                              </span>
                              <span className="channel-selector-row-label">
                                {def.label}
                              </span>
                            </div>

                            <div className="channel-selector-row-right">
                              {unitSymbol && (
                                <span className="channel-selector-row-unit">
                                  {unitSymbol}
                                </span>
                              )}
                              <ProvenanceBadge provenance={def.provenance} />
                              {slot !== null && (
                                <span
                                  className="telemetry-graph__swatch channel-selector-swatch-slot"
                                  data-slot={slot}
                                  title={`Plot Series #${slot}`}
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
          <div className="channel-selector-tray" data-testid="selected-series-tray">
            <div className="channel-selector-tray-header">
              <span className="channel-selector-tray-title">
                PLOTTED SERIES
              </span>
              <span
                className={`status-badge ${selectedIds.length === MAX_PLOT_CHANNELS ? 'status-badge-warn' : 'status-badge-live'} channel-selector-tray-count`}
                data-testid="selected-channel-count"
              >
                {selectedIds.length} / {MAX_PLOT_CHANNELS}
              </span>
            </div>

            <div className="channel-selector-tray-list">
              {selectedIds.length === 0 ? (
                <div className="channel-selector-empty">
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
                      className="channel-selector-selected-item"
                      data-testid={`selected-item-${id}`}
                    >
                      <div className="channel-selector-selected-left">
                        <span
                          className="telemetry-graph__swatch channel-selector-selected-swatch"
                          data-slot={slot}
                          title={`Color slot ${slot}`}
                        />
                        <span className="channel-selector-selected-name">
                          {def?.exportName ?? `Ch #${id}`}
                        </span>
                        {unitSymbol && (
                          <span className="channel-selector-selected-unit">
                            ({unitSymbol})
                          </span>
                        )}
                      </div>

                      <div className="channel-selector-selected-controls">
                        <button
                          type="button"
                          className="btn btn-xs channel-selector-slot-btn"
                          onClick={() => {
                            handleMoveUp(index);
                          }}
                          disabled={index === 0}
                          title="Move up (change color slot)"
                          aria-label={`Move ${def?.exportName ?? id} up`}
                        >
                          ▲
                        </button>
                        <button
                          type="button"
                          className="btn btn-xs channel-selector-slot-btn"
                          onClick={() => {
                            handleMoveDown(index);
                          }}
                          disabled={index === selectedIds.length - 1}
                          title="Move down (change color slot)"
                          aria-label={`Move ${def?.exportName ?? id} down`}
                        >
                          ▼
                        </button>
                        <button
                          type="button"
                          className="btn btn-xs channel-selector-remove-btn"
                          onClick={() => {
                            handleRemoveSelected(id);
                          }}
                          title="Remove from graph"
                          aria-label={`Remove ${def?.exportName ?? id}`}
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
        <div className="channel-selector-footer">
          <span className="channel-selector-tip">
            Tip: Click swatches or channel labels on the graph legend to toggle traces on/off.
          </span>
          <div className="channel-selector-actions">
            <button
              type="button"
              className="btn btn-sm"
              onClick={onClose}
              data-testid="channel-selector-cancel-btn"
            >
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-sm btn-primary"
              onClick={handleApply}
              disabled={selectedIds.length === 0}
              data-testid="channel-selector-apply-btn"
            >
              Apply Channels ({selectedIds.length})
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
