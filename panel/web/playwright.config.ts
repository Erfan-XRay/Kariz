import { defineConfig } from "@playwright/test";

// The tests run against a real panel and a real agent (e2e/global-setup.ts starts them), in a
// browser. PW_CHANNEL=msedge uses the Edge that is installed (no download); CI uses Chromium.
export default defineConfig({
  testDir: "./e2e",
  // One panel, one order: the lockout test is last, because it locks this machine's address.
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],
  globalSetup: "./e2e/global-setup.ts",
  globalTeardown: "./e2e/global-teardown.ts",
  use: {
    ignoreHTTPSErrors: true,
    channel: process.env.PW_CHANNEL || undefined,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    viewport: { width: 1280, height: 800 },
  },
  outputDir: "./e2e-results",
});
