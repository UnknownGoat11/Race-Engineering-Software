import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { EngineerView } from '../../lib/statusModel';
import { PitwallBar } from './PitwallBar';

describe('PitwallBar', () => {
  const liveView: EngineerView = { kind: 'live', ageS: 0.1, degraded: false };
  const syntheticView: EngineerView = { kind: 'synthetic', ageS: 0.1 };

  it('renders LIVE stream badge when origin is lmu and view is live', () => {
    render(
      <PitwallBar
        view={liveView}
        origin="lmu"
        driverName="Jane Doe"
        carName="Ferrari 499P"
        vehicleClass="Hypercar"
        trackName="Circuit de la Sarthe"
        rttMs={33}
        streamAgeS={0.1}
        isConnected={true}
        onRefreshAllData={vi.fn()}
        onLeaveSession={vi.fn()}
      />,
    );

    const badge = screen.getByTestId('stream-health-badge');
    expect(badge).toHaveTextContent('LIVE STREAM');
    expect(screen.getByTestId('engineer-driver-name')).toHaveTextContent('Jane Doe');
    expect(screen.getByTestId('engineer-car')).toHaveTextContent('Ferrari 499P (HY)');
    expect(screen.getByTestId('engineer-track')).toHaveTextContent('Circuit de la Sarthe');
    expect(screen.getByTestId('engineer-rtt')).toHaveTextContent('33 ms');
    expect(screen.getByTestId('engineer-stream-age')).toHaveTextContent('0.1 s');
  });

  it('renders SYNTHETIC badge and never LIVE when origin is synthetic (T04)', () => {
    render(
      <PitwallBar
        view={syntheticView}
        origin="synthetic"
        driverName="Jane Doe"
        isConnected={true}
        onRefreshAllData={vi.fn()}
        onLeaveSession={vi.fn()}
      />,
    );

    const badge = screen.getByTestId('stream-health-badge');
    expect(badge).toHaveTextContent('SYNTHETIC');
    expect(badge.textContent).not.toContain('LIVE');
  });

  it('hides driver identity when mayShowDriverIdentity is false (T09, OR-3)', () => {
    const awaitingView: EngineerView = { kind: 'awaitingConfirmation' };
    render(
      <PitwallBar
        view={awaitingView}
        origin="lmu"
        driverName="Secret Driver"
        carName="Secret Car"
        isConnected={true}
        onRefreshAllData={vi.fn()}
        onLeaveSession={vi.fn()}
      />,
    );

    expect(screen.queryByTestId('engineer-driver-name')).not.toBeInTheDocument();
    expect(screen.queryByTestId('engineer-car')).not.toBeInTheDocument();
  });

  it('calls onRefreshAllData and onOpenSettings when clicked', () => {
    const onRefresh = vi.fn();
    const onSettings = vi.fn();
    render(
      <PitwallBar
        view={liveView}
        origin="lmu"
        isConnected={true}
        onRefreshAllData={onRefresh}
        onLeaveSession={vi.fn()}
        onOpenSettings={onSettings}
      />,
    );

    fireEvent.click(screen.getByTestId('btn-request-snapshot'));
    expect(onRefresh).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByTestId('btn-open-settings'));
    expect(onSettings).toHaveBeenCalledOnce();
  });

  it('ended: one Close button replaces Resync and Leave (U18)', () => {
    const onClose = vi.fn();
    render(
      <PitwallBar
        view={{ kind: 'ended', reason: null }}
        isConnected={true}
        ended={true}
        onClose={onClose}
        onRefreshAllData={vi.fn()}
        onLeaveSession={vi.fn()}
      />,
    );
    expect(screen.queryByTestId('btn-request-snapshot')).not.toBeInTheDocument();
    expect(screen.queryByTestId('btn-engineer-disconnect')).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('btn-engineer-close'));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Resync carries its tooltip; FCY / SC shows one pill', () => {
    render(
      <PitwallBar
        view={liveView}
        origin="lmu"
        caution="fcy"
        isConnected={true}
        onRefreshAllData={vi.fn()}
        onLeaveSession={vi.fn()}
      />,
    );
    expect(screen.getByTestId('btn-request-snapshot')).toHaveTextContent('Resync');
    expect(screen.getByTestId('btn-request-snapshot')).toHaveAttribute('title', "Ask the driver's app for a full state snapshot");
    expect(screen.getAllByTestId('pitwall-caution')).toHaveLength(1);
  });
});
