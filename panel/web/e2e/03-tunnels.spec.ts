import net from "node:net";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { goTo, signIn } from "./helpers";
import { freePort, startAgent } from "./stack";
import type { Agent } from "./stack";

// One panel, one real agent joined through the page, and a tunnel between them made with the
// wizard: the browser does what a person does. The tunnel's own status comes over a Unix
// socket, so the tunnel tests run on Linux (the CI); the page part runs anywhere.
test.describe.configure({ mode: "serial" });

let agent: Agent | undefined;
let echo: net.Server;
let echoPort = 0;

test.beforeAll(async () => {
  echo = net.createServer((s) => s.pipe(s));
  await new Promise<void>((r) => echo.listen(0, "127.0.0.1", r));
  echoPort = (echo.address() as net.AddressInfo).port;
});

test.afterAll(() => {
  echo.close();
  agent?.stop();
});

/** Sends something through a port and reads it back (or gives up). */
async function echoes(port: number, what: string): Promise<boolean> {
  for (let i = 0; i < 20; i++) {
    const ok = await new Promise<boolean>((resolve) => {
      const s = net.connect(port, "127.0.0.1");
      const timer = setTimeout(() => (s.destroy(), resolve(false)), 2000);
      s.once("error", () => (clearTimeout(timer), resolve(false)));
      s.once("data", (d) => (clearTimeout(timer), s.destroy(), resolve(d.toString() === what)));
      s.once("connect", () => s.write(what));
    });
    if (ok) return true;
    await new Promise((r) => setTimeout(r, 500));
  }
  return false;
}

async function refuses(port: number): Promise<boolean> {
  for (let i = 0; i < 10; i++) {
    const refused = await new Promise<boolean>((resolve) => {
      const s = net.connect(port, "127.0.0.1");
      s.once("error", () => resolve(true));
      s.once("connect", () => (s.destroy(), resolve(false)));
    });
    if (refused) return true;
    await new Promise((r) => setTimeout(r, 500));
  }
  return false;
}

async function next(page: Page) {
  await page.locator(".wz-foot .btn-primary").click();
}

/** The wizard for a reverse tcpmux tunnel from this server to the agent. */
async function fillWizard(page: Page, opts: { name: string; front: number; dial?: string }) {
  await page.locator(".page-band .btn-primary").click();
  await page.locator("#wz-name").fill(opts.name);
  await page.locator(".pick-list").nth(0).locator("button").nth(0).click();
  await page.locator(".pick-list").nth(1).locator("button").nth(1).click();
  await next(page); // kind: reverse, tcpmux, balanced
  await next(page);
  await page.locator("#wz-port").fill(String(await freePort()));
  if (opts.dial) await page.locator("#wz-dial").fill(opts.dial);
  await next(page);
  // listen on `front`, reach the echo server's port
  await page.locator("#wz-ports").fill(`${opts.front}=${echoPort}`);
  await next(page); // review
}

test("a server is added with a join code and comes online", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  await page.locator(".page-band .btn-primary").click();
  await page.locator("#add-name").fill("far-away");
  await page.getByRole("button", { name: /Make a join code/i }).click();
  const shown = await page.locator("pre.code").innerText();
  const code = /kz1_[A-Za-z0-9_-]+/.exec(shown)?.[0];
  expect(code, "a join code is shown").toBeTruthy();
  agent = startAgent(code!);
  await expect(page.locator(".waiting.done")).toBeVisible({ timeout: 45_000 });
  await page.getByRole("button", { name: /Done/i }).click();
  await expect(page.locator(".srv", { hasText: "far-away" })).toContainText(/online/i);
});

test.describe("the tunnel", () => {
  test.skip(process.platform === "win32", "the tunnels' status comes over a Unix socket");

  let front = 0;

  test("is made with the wizard, carries traffic, and is edited", async ({ page }) => {
    front = await freePort();
    await signIn(page);
    await goTo(page, "tunnels");
    await fillWizard(page, { name: "demo", front });
    await page.locator(".wz-foot .btn-primary").click(); // Build
    await expect(page.locator(".wz-step .ok")).toContainText(/Done/i, { timeout: 60_000 });
    for (const li of await page.locator(".checklist li").all()) await expect(li).toHaveClass(/ok/);
    await page.getByRole("button", { name: /Close/i }).click();
    expect(await echoes(front, "through the wizard's tunnel")).toBe(true);

    // it shows up as up, and its page opens
    const row = page.locator(".tunnels tr", { hasText: "demo" });
    await expect(row).toContainText(/up|connected/i, { timeout: 30_000 });
    await row.click();
    await page.getByRole("button", { name: "Edit" }).click();
    // the wizard is filled in from the servers: the ports step shows what is there now
    for (let i = 0; i < 3; i++) await next(page);
    await expect(page.locator("#wz-ports")).toHaveValue(`${front}=${echoPort}`);
    const newFront = await freePort();
    await page.locator("#wz-ports").fill(`${newFront}=${echoPort}`);
    await next(page);
    await page.locator(".wz-foot .btn-primary").click(); // Save
    await expect(page.locator(".wz-step .ok")).toContainText(/Done/i, { timeout: 60_000 });
    await page.getByRole("button", { name: /Close/i }).click();
    expect(await echoes(newFront, "after the edit")).toBe(true);
    expect(await refuses(front), "the old port is closed").toBe(true);
    front = newFront;
  });

  test("is stopped, started again and deleted, from its page", async ({ page }) => {
    await signIn(page);
    await goTo(page, "tunnels");
    const open = async () => {
      await page.locator(".tunnels tr", { hasText: "demo" }).click();
    };
    await open();
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await page.locator(".dialog .btn-primary").click();
    await expect(page.locator(".dialog .ok")).toContainText(/Done/i, { timeout: 60_000 });
    await page.getByRole("button", { name: /Close/i }).click();
    expect(await refuses(front), "a stopped tunnel listens nowhere").toBe(true);

    await open();
    await page.getByRole("button", { name: "Start", exact: true }).click();
    await page.locator(".dialog .btn-primary").click();
    await expect(page.locator(".dialog .ok")).toContainText(/Done/i, { timeout: 60_000 });
    await page.getByRole("button", { name: /Close/i }).click();
    expect(await echoes(front, "after starting again")).toBe(true);

    await open();
    await page.getByRole("button", { name: "Delete" }).click();
    await page.locator(".dialog .btn-danger").click();
    await expect(page.locator(".dialog .ok")).toContainText(/Done/i, { timeout: 60_000 });
    await page.getByRole("button", { name: /Close/i }).click();
    await expect(page.locator(".tunnels tr", { hasText: "demo" })).toHaveCount(0, { timeout: 30_000 });
    expect(await refuses(front), "a deleted tunnel listens nowhere").toBe(true);
  });

  test("that cannot connect is undone on both servers and says why", async ({ page }) => {
    const front2 = await freePort();
    await signIn(page);
    await goTo(page, "tunnels");
    // the other side is told an address nothing answers at
    await fillWizard(page, { name: "lost", front: front2, dial: "192.0.2.1" });
    await page.locator(".wz-foot .btn-primary").click();
    await expect(page.locator(".wz-step .err")).toBeVisible({ timeout: 80_000 });
    await expect(page.locator(".wz-step")).toContainText(/removed/i);
    expect(await refuses(front2), "nothing was left listening").toBe(true);
    await page.getByRole("button", { name: /Close/i }).click();
    await expect(page.locator(".tunnels tr", { hasText: "lost" })).toHaveCount(0, { timeout: 30_000 });
  });
});
