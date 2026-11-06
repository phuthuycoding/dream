import { defineConfig } from '@playwright/test';

// The daemon under test is the real ./target/debug/dream binary — it is
// spawned per test by e2e/helpers.ts on an ephemeral port with a throwaway
// --data-dir, so no webServer block is needed here.
export default defineConfig({
  testDir: './e2e',
  outputDir: 'e2e/test-results',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  retries: 0,
  reporter: [['list']],
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
});
