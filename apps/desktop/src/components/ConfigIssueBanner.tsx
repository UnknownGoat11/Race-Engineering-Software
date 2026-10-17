import type { JSX } from 'react';
import type { ConfigIssue } from '../lib/ipc';

const TITLES: Record<ConfigIssue['kind'], string> = {
  invalid: 'Configuration file invalid — safe defaults in use',
  unsupported_version: 'Configuration file from an unsupported version — safe defaults in use',
  unreadable: 'Configuration file could not be read — safe defaults in use',
};

interface ConfigIssueBannerProps {
  readonly issue: ConfigIssue;
}

/** Explains why the stored configuration was rejected and where the rejected file was preserved. */
export function ConfigIssueBanner({ issue }: ConfigIssueBannerProps): JSX.Element {
  return (
    <div className="alert" role="alert">
      <span className="alert-title">{TITLES[issue.kind]}</span>
      <span>{issue.message}</span>
      {issue.errors.length > 0 && (
        <ul>
          {issue.errors.map((error) => (
            <li key={`${error.field}:${error.message}`}>
              <code>{error.field}</code>: {error.message}
            </li>
          ))}
        </ul>
      )}
      {issue.preserved_as !== null && (
        <span>
          The rejected file was preserved as <code>{issue.preserved_as}</code>.
        </span>
      )}
    </div>
  );
}
