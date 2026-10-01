import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: './test/browser',
  fullyParallel: true,
  workers: 2,
  reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:3417', trace: 'retain-on-failure' },
  webServer: {
    command: 'bun src/server.mjs',
    url: 'http://127.0.0.1:3417/healthz',
    reuseExistingServer: false,
    env: {
      PORT: '3417',
      HOST: '127.0.0.1',
      PUBLIC_ORIGIN: 'http://127.0.0.1:3417',
      W3BS_DATA_DIR: '.local/browser-test',
    },
  },
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'] } },
    { name: 'mobile', use: { ...devices['iPhone 13'], defaultBrowserType: 'chromium' } },
  ],
});
