import { useState, type JSX } from 'react';
import type { AppInfo, ConfigSnapshot, Mode } from '../lib/ipc';
import { buildProfileLabel, environmentLabel, modeLabel } from '../lib/labels';
import { NO_VALUE } from '../lib/units';

interface SettingsPageProps {
  readonly info: AppInfo;
  readonly snapshot: ConfigSnapshot;
  readonly mode: Mode;
  readonly onChangeMode: (mode: Mode) => Promise<void>;
}

const MODES: readonly Mode[] = ['driver', 'engineer'];

type SaveState = { readonly kind: 'idle' } | { readonly kind: 'saving' } | { readonly kind: 'error'; readonly message: string };

function textOrDash(value: string | null): string {
  return value === null || value === '' ? NO_VALUE : value;
}

export function SettingsPage({ info, snapshot, mode, onChangeMode }: SettingsPageProps): JSX.Element {
  const [selected, setSelected] = useState<Mode>(mode);
  const [save, setSave] = useState<SaveState>({ kind: 'idle' });
  const { config } = snapshot;
  const extraCaHonoured = info.environment === 'development';

  const apply = (): void => {
    setSave({ kind: 'saving' });
    onChangeMode(selected).then(
      () => {
        setSave({ kind: 'idle' });
      },
      (reason: unknown) => {
        setSave({ kind: 'error', message: reason instanceof Error ? reason.message : String(reason) });
      },
    );
  };

  return (
    <section className="page">
      <h2 className="page-title">Settings</h2>

      <div className="panel">
        <div className="panel-header">
          <span id="settings-mode-label">Operating mode</span>
          <span>Current: {modeLabel(mode)}</span>
        </div>
        <div className="panel-body">
          <div className="button-row" role="group" aria-labelledby="settings-mode-label">
            {MODES.map((option) => (
              <button
                key={option}
                type="button"
                className="btn"
                aria-pressed={selected === option}
                disabled={save.kind === 'saving'}
                onClick={() => {
                  setSelected(option);
                }}
              >
                {modeLabel(option)} MODE
              </button>
            ))}
            <button
              type="button"
              className="btn btn-primary"
              disabled={selected === mode || save.kind === 'saving'}
              onClick={apply}
            >
              Apply mode change
            </button>
            {save.kind === 'saving' && <span className="form-status">Saving…</span>}
          </div>
          {save.kind === 'error' && (
            <div className="alert alert-error" role="alert">
              <span className="alert-title">Could not save the operating mode</span>
              <span>{save.message}</span>
            </div>
          )}
        </div>
      </div>

      <div className="panel">
        <div className="panel-header">
          <span>Application</span>
        </div>
        <div className="panel-body">
          <dl className="kv">
            <dt>Version</dt>
            <dd className="num">{info.version}</dd>
            <dt>Build profile</dt>
            <dd>{buildProfileLabel(info.build_profile)}</dd>
            <dt>Environment</dt>
            <dd>{environmentLabel(info.environment)}</dd>
            <dt>Identifier</dt>
            <dd>
              <code>{info.identifier}</code>
            </dd>
            <dt>Configuration file</dt>
            <dd>
              <code>{snapshot.path}</code>
            </dd>
            <dt>Log directory</dt>
            <dd>
              <code>{textOrDash(info.log_dir)}</code>
            </dd>
          </dl>
        </div>
      </div>

      <div className="panel">
        <div className="panel-header">
          <span>Configuration</span>
          <span className="num">schema v{config.schema_version}</span>
        </div>
        <div className="panel-body">
          <dl className="kv">
            <dt>Relay host</dt>
            <dd>{textOrDash(config.network.relay_host)}</dd>
            <dt>Relay port</dt>
            <dd className="num">{config.network.relay_port}</dd>
            <dt>Connect timeout</dt>
            <dd className="num">{config.network.connect_timeout_ms} ms</dd>
            <dt>Extra CA file</dt>
            <dd>
              {textOrDash(config.network.extra_ca_file)}
              {config.network.extra_ca_file !== null && !extraCaHonoured && ' (ignored outside development builds)'}
            </dd>
            <dt>Recording</dt>
            <dd>{config.telemetry.recording_enabled ? 'Enabled' : 'Disabled'}</dd>
            <dt>Recording directory</dt>
            <dd>{config.telemetry.recording_dir ?? 'Default (per-user app data)'}</dd>
            <dt>Fuel averaging window (laps)</dt>
            <dd className="num">{config.fuel.window_laps}</dd>
            <dt>Fuel reserve (laps)</dt>
            <dd className="num">{config.fuel.reserve_laps}</dd>
            <dt>UI scale</dt>
            <dd className="num">{config.ui.scale}</dd>
            <dt>UI max refresh</dt>
            <dd className="num">{config.ui.max_refresh_hz} Hz</dd>
          </dl>
          <p className="panel-note">
            These values are read from the configuration file. Only the operating mode is editable in this build.
          </p>
        </div>
      </div>
    </section>
  );
}
