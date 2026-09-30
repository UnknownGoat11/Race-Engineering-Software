// Vitest setup: jest-dom matchers and DOM cleanup between tests (globals are disabled, so RTL's automatic
// cleanup does not register itself).
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

afterEach(() => {
  cleanup();
});
