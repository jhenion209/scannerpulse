import { defineConfig, devices } from '@playwright/test';

// The tests run against a real `scannerpulse web` over a seeded database
// (e2e/seed.sh). E2E_BASE_URL names one already running; otherwise the
// binary named by E2E_BINARY (default: the workspace's debug build) is
// started here, with DATABASE_URL from the environment.
const external = process.env.E2E_BASE_URL;
const listen = '127.0.0.1:8099';

export default defineConfig({
  testDir: 'e2e',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  use: { baseURL: external ?? `http://${listen}`, ...devices['Desktop Chrome'] },
  webServer: external
    ? undefined
    : {
        command: process.env.E2E_BINARY ?? '../target/debug/scannerpulse web',
        url: `http://${listen}/health`,
        env: { SCANNERPULSE_WEB_LISTEN: listen },
        reuseExistingServer: false,
      },
});
