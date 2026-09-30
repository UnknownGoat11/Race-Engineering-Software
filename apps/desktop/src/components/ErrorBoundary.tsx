import { Component, Fragment, type ErrorInfo, type ReactNode } from 'react';

function describeError(error: unknown): string {
  if (error instanceof Error) {
    return error.stack ?? `${error.name}: ${error.message}`;
  }
  return String(error);
}

// ---------------------------------------------------------------------------------------------------------
// Top-level boundary: last line of defence. Anything reaching it means the shell itself failed.
// ---------------------------------------------------------------------------------------------------------

interface AppErrorBoundaryProps {
  readonly children: ReactNode;
}

interface AppErrorBoundaryState {
  readonly error: unknown;
  readonly hasError: boolean;
}

export class AppErrorBoundary extends Component<AppErrorBoundaryProps, AppErrorBoundaryState> {
  override state: AppErrorBoundaryState = { error: null, hasError: false };

  static getDerivedStateFromError(error: unknown): AppErrorBoundaryState {
    return { error, hasError: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('[AppErrorBoundary] unrecoverable UI error', error, info.componentStack);
  }

  private readonly reload = (): void => {
    window.location.reload();
  };

  override render(): ReactNode {
    if (!this.state.hasError) {
      return this.props.children;
    }
    return (
      <main className="fullscreen-state" role="alert">
        <h1>User interface error</h1>
        <p>
          The interface stopped because of an unexpected error. Background processes (acquisition, recording,
          streaming) run in the native backend and are not affected.
        </p>
        <pre>{describeError(this.state.error)}</pre>
        <button type="button" className="btn btn-primary" onClick={this.reload}>
          Reload interface
        </button>
      </main>
    );
  }
}

// ---------------------------------------------------------------------------------------------------------
// Panel boundary: isolates one panel so a failure never takes down the rest of the app.
// ---------------------------------------------------------------------------------------------------------

interface PanelErrorBoundaryProps {
  /** Panel name shown in the fallback, e.g. "Fuel". */
  readonly name: string;
  readonly children: ReactNode;
  /** Optional hook for diagnostics; called once per caught error. */
  readonly onError?: (error: unknown, info: ErrorInfo) => void;
}

interface PanelErrorBoundaryState {
  readonly hasError: boolean;
  /** Incremented on retry so children remount with fresh state. */
  readonly attempt: number;
}

export class PanelErrorBoundary extends Component<PanelErrorBoundaryProps, PanelErrorBoundaryState> {
  override state: PanelErrorBoundaryState = { hasError: false, attempt: 0 };

  static getDerivedStateFromError(): Partial<PanelErrorBoundaryState> {
    return { hasError: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error(`[PanelErrorBoundary] panel "${this.props.name}" failed`, error, info.componentStack);
    this.props.onError?.(error, info);
  }

  private readonly retry = (): void => {
    this.setState((previous) => ({ hasError: false, attempt: previous.attempt + 1 }));
  };

  override render(): ReactNode {
    if (this.state.hasError) {
      return (
        <section className="panel-unavailable" role="alert" aria-label={`${this.props.name} panel`}>
          <span className="panel-unavailable-title">Panel unavailable</span>
          <span>{this.props.name}</span>
          <button type="button" className="btn" onClick={this.retry}>
            Retry
          </button>
        </section>
      );
    }
    return <Fragment key={this.state.attempt}>{this.props.children}</Fragment>;
  }
}
