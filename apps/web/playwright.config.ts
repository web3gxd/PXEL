import { defineConfig, devices } from '@playwright/test';

const runnerProcess = (
  globalThis as unknown as {
    process: {
      env: Record<string, string | undefined>;
      pid: number;
      getBuiltinModule(name: 'url'): { fileURLToPath(url: URL): string };
    };
  }
).process;
const runnerEnvironment = runnerProcess.env;
// Private, suite-specific cookie state, removed by global teardown.
const authState =
  runnerEnvironment.PXEL_PLAYWRIGHT_AUTH_PATH ??
  runnerProcess
    .getBuiltinModule('url')
    .fileURLToPath(
      new URL(`../../.local/playwright-auth-${runnerProcess.pid}.json`, import.meta.url),
    );
// Playwright evaluates this config again in worker processes. Share the runner's
// path so every browser context reads the session created by global setup.
runnerEnvironment.PXEL_PLAYWRIGHT_AUTH_PATH = authState;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  // The development server limits session/WS requests to 64 per second globally.
  // More browser workers can exhaust that shared bucket during StrictMode reconnects.
  workers: 4,
  forbidOnly: true,
  retries: 0,
  reporter: [['list'], ['html', { open: 'never' }]],
  globalSetup: './e2e/global-setup.ts',
  globalTeardown: './e2e/global-teardown.ts',
  use: {
    baseURL: runnerEnvironment.PXEL_WEB_URL ?? 'http://localhost:5173',
    trace: 'retain-on-failure',
    storageState: authState,
  },
  projects: [
    {
      name: 'chromium-dpr1',
      use: { ...devices['Desktop Chrome'], deviceScaleFactor: 1 },
    },
    {
      name: 'chromium-dpr2',
      use: { ...devices['Desktop Chrome'], deviceScaleFactor: 2 },
    },
  ],
});
