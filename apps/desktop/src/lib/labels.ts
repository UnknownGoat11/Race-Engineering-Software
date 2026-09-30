import type { BuildProfile, Environment, Mode } from './ipc';

/** Upper-case display labels for enumerations shown in the UI. */

export function modeLabel(mode: Mode): string {
  switch (mode) {
    case 'driver':
      return 'DRIVER';
    case 'engineer':
      return 'ENGINEER';
  }
}

export function environmentLabel(environment: Environment): string {
  switch (environment) {
    case 'development':
      return 'DEVELOPMENT';
    case 'staging':
      return 'STAGING';
    case 'production':
      return 'PRODUCTION';
  }
}

export function buildProfileLabel(profile: BuildProfile): string {
  return profile === 'debug' ? 'DEBUG' : 'RELEASE';
}
