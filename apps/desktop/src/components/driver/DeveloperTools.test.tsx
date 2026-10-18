import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DeveloperTools } from './DeveloperTools';

describe('DeveloperTools', () => {
  it('is hidden when environment is production', () => {
    const { container } = render(
      <DeveloperTools
        environment="production"
        isSynthetic={false}
        isStreaming={false}
        isRecordingActive={false}
        selectedScenario="demo"
        onSelectScenario={vi.fn()}
        onStartDemo={vi.fn()}
        onStopDemo={vi.fn()}
      />,
    );

    expect(container.firstChild).toBeNull();
    expect(screen.queryByTestId('driver-developer-tools')).not.toBeInTheDocument();
  });

  it('renders in development environment', () => {
    render(
      <DeveloperTools
        environment="development"
        isSynthetic={false}
        isStreaming={false}
        isRecordingActive={false}
        selectedScenario="demo"
        onSelectScenario={vi.fn()}
        onStartDemo={vi.fn()}
        onStopDemo={vi.fn()}
      />,
    );

    expect(screen.getByTestId('driver-developer-tools')).toBeInTheDocument();
    expect(screen.getByLabelText(/Demo scenario/i)).toBeInTheDocument();
    expect(screen.getByTestId('start-demo-btn')).toBeInTheDocument();
  });

  it('disables Start Demo when streaming or recording', () => {
    const { rerender } = render(
      <DeveloperTools
        environment="development"
        isSynthetic={false}
        isStreaming={true}
        isRecordingActive={false}
        selectedScenario="demo"
        onSelectScenario={vi.fn()}
        onStartDemo={vi.fn()}
        onStopDemo={vi.fn()}
      />,
    );

    expect(screen.getByTestId('start-demo-btn')).toBeDisabled();

    rerender(
      <DeveloperTools
        environment="development"
        isSynthetic={false}
        isStreaming={false}
        isRecordingActive={true}
        selectedScenario="demo"
        onSelectScenario={vi.fn()}
        onStartDemo={vi.fn()}
        onStopDemo={vi.fn()}
      />,
    );

    expect(screen.getByTestId('start-demo-btn')).toBeDisabled();
  });

  it('disables Stop Demo when not synthetic', () => {
    render(
      <DeveloperTools
        environment="development"
        isSynthetic={false}
        isStreaming={false}
        isRecordingActive={false}
        selectedScenario="demo"
        onSelectScenario={vi.fn()}
        onStartDemo={vi.fn()}
        onStopDemo={vi.fn()}
      />,
    );

    expect(screen.getByTestId('stop-demo-btn')).toBeDisabled();
  });

  it('triggers onStartDemo, onStopDemo, and onSelectScenario', () => {
    const onStartDemo = vi.fn();
    const onStopDemo = vi.fn();
    const onSelectScenario = vi.fn();

    render(
      <DeveloperTools
        environment="development"
        isSynthetic={true}
        isStreaming={false}
        isRecordingActive={false}
        selectedScenario="demo"
        onSelectScenario={onSelectScenario}
        onStartDemo={onStartDemo}
        onStopDemo={onStopDemo}
      />,
    );

    fireEvent.click(screen.getByTestId('start-demo-btn'));
    expect(onStartDemo).toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('stop-demo-btn'));
    expect(onStopDemo).toHaveBeenCalled();

    fireEvent.change(screen.getByTestId('demo-scenario-select'), {
      target: { value: 'rain' },
    });
    expect(onSelectScenario).toHaveBeenCalledWith('rain');
  });
});
