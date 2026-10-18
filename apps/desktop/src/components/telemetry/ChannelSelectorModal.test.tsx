import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ch } from '../../generated/channels';
import { ChannelSelectorModal } from './ChannelSelectorModal';

describe('ChannelSelectorModal (MoTeC i2 Pro style)', () => {
  it('does not render when isOpen is false', () => {
    const { container } = render(
      <ChannelSelectorModal
        isOpen={false}
        activeChannelIds={[ch.SPEED, ch.THROTTLE]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('renders modal with channel catalog and active selected channels when isOpen is true', () => {
    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED, ch.THROTTLE]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.getByText('Choose channels')).toBeInTheDocument();
    expect(screen.getByTestId('channel-search-input')).toBeInTheDocument();
    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('2 / 8');
    expect(screen.getByTestId(`selected-item-${ch.SPEED}`)).toBeInTheDocument();
    expect(screen.getByTestId(`selected-item-${ch.THROTTLE}`)).toBeInTheDocument();
  }, 15000);

  it('filters channels in real-time when searching by name or export code', () => {
    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    const searchInput = screen.getByTestId('channel-search-input');
    fireEvent.change(searchInput, { target: { value: 'BrakeTemp' } });

    // Should find brake temperature channels
    expect(screen.getByTestId(`channel-row-${ch.BRAKE_TEMP_FL}`)).toBeInTheDocument();
    expect(screen.getByTestId(`channel-row-${ch.BRAKE_TEMP_FR}`)).toBeInTheDocument();

    // Clear search
    fireEvent.click(screen.getByLabelText('Clear search'));
    expect(searchInput).toHaveValue('');
  });

  it('adds and removes channels when rows are clicked', () => {
    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('1 / 8');

    // Click steering channel to add it
    const steerRow = screen.getByTestId(`channel-row-${ch.STEERING}`);
    fireEvent.click(steerRow);

    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('2 / 8');
    expect(screen.getByTestId(`selected-item-${ch.STEERING}`)).toBeInTheDocument();

    // Click it again to deselect
    fireEvent.click(steerRow);
    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('1 / 8');
    expect(screen.queryByTestId(`selected-item-${ch.STEERING}`)).not.toBeInTheDocument();
  });

  it('enforces maximum 8 channels guard with warning notice', () => {
    const eightChannels = [
      ch.SPEED,
      ch.THROTTLE,
      ch.BRAKE,
      ch.STEERING,
      ch.RPM,
      ch.FUEL,
      ch.BRAKE_TEMP_FL,
      ch.BRAKE_TEMP_FR,
    ];

    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={eightChannels}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('8 / 8');

    // Attempt to add a 9th channel from VehicleInputs (e.g. ch.CLUTCH)
    const extraRow = screen.getByTestId(`channel-row-${ch.CLUTCH}`);
    fireEvent.click(extraRow);

    // Should show error notice and remain at 8 channels
    expect(screen.getByRole('alert')).toHaveTextContent(/Maximum 8 channels allowed/);
    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('8 / 8');
  });

  it('reorders channels with Move Up and Move Down buttons', () => {
    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED, ch.THROTTLE, ch.BRAKE]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    const throttleItem = screen.getByTestId(`selected-item-${ch.THROTTLE}`);
    const moveUpBtn = throttleItem.querySelector('button[title*="Move up"]');
    expect(moveUpBtn).not.toBeNull();
    if (moveUpBtn) fireEvent.click(moveUpBtn);

    // Throttle should now be first (index 0)
    const tray = screen.getByTestId('selected-series-tray');
    const items = tray.querySelectorAll('[data-testid^="selected-item-"]');
    expect(items[0]).toHaveAttribute('data-testid', `selected-item-${ch.THROTTLE}`);
    expect(items[1]).toHaveAttribute('data-testid', `selected-item-${ch.SPEED}`);
  });

  it('applies quick presets and clears all', () => {
    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED]}
        onClose={vi.fn()}
        onApply={vi.fn()}
      />,
    );

    // Click Tyres preset
    fireEvent.click(screen.getByTestId('preset-tyres-btn'));
    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('4 / 8');
    expect(screen.getByTestId(`selected-item-${ch.TYRE_TEMP_MIDDLE_FL}`)).toBeInTheDocument();

    // Click Clear
    fireEvent.click(screen.getByTestId('clear-all-channels-btn'));
    expect(screen.getByTestId('selected-channel-count')).toHaveTextContent('0 / 8');
  });

  it('calls onApply with selected channel IDs on Apply button click', () => {
    const onApply = vi.fn();
    const onClose = vi.fn();

    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED, ch.THROTTLE]}
        onClose={onClose}
        onApply={onApply}
      />,
    );

    fireEvent.click(screen.getByTestId('channel-selector-apply-btn'));

    expect(onApply).toHaveBeenCalledWith([ch.SPEED, ch.THROTTLE]);
    expect(onClose).toHaveBeenCalled();
  });

  it('calls onClose when Cancel or close button is clicked', () => {
    const onClose = vi.fn();

    render(
      <ChannelSelectorModal
        isOpen={true}
        activeChannelIds={[ch.SPEED]}
        onClose={onClose}
        onApply={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByTestId('channel-selector-cancel-btn'));
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByLabelText('Close dialog'));
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
