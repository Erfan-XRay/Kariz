import { expect, test } from "@playwright/test";
import { newLink, readStack, setPassword } from "./stack";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kariz.lang", "en");
    localStorage.setItem("kariz.low", "1");
  });
});

test("a one-time link signs in, disappears from the address, and works only once", async ({ page, browser }) => {
  const link = newLink();
  await page.goto(link);
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
  // the token is in the fragment, and the page took it out of the address bar and history
  expect(page.url()).not.toContain("#t=");
  expect(page.url()).not.toContain("t=");

  // the same link again, in a browser that has nothing: refused
  const other = await browser.newContext({ ignoreHTTPSErrors: true });
  const second = await other.newPage();
  await second.addInitScript(() => localStorage.setItem("kariz.lang", "en"));
  await second.goto(link);
  await expect(second.locator("#login")).toBeVisible({ timeout: 30_000 });
  await expect(second.locator("#pw-err")).toContainText(/wrong|used up|expired/i, { timeout: 15_000 });
  await other.close();
});

test("the panel answers only under its secret path, with the plain nginx page anywhere else", async ({ request }) => {
  const { base } = readStack();
  const origin = new URL(base).origin;
  for (const path of ["/", "/admin", "/login", "/api/session", "/k-other/"]) {
    const r = await request.get(origin + path);
    expect(r.status(), path).toBe(404);
    expect(r.headers()["server"], path).toBe("nginx");
    expect(await r.text(), path).toContain("nginx");
  }
  const page = await request.get(base);
  expect(page.status()).toBe(200);
  const h = page.headers();
  expect(h["x-frame-options"]).toBe("DENY");
  expect(h["content-security-policy"]).toContain("script-src 'self'");
  expect(h["x-content-type-options"]).toBe("nosniff");
});

test("a password signs in, and a wrong one says how many tries are left", async ({ page }) => {
  setPassword("a long enough panel password");
  await page.goto(readStack().base);
  await expect(page.locator("#login")).toBeVisible({ timeout: 30_000 });
  await page.locator("#pw").fill("not the password at all");
  await page.locator(".login-form .btn-primary").click();
  await expect(page.locator("#pw-err")).toContainText(/\d tries left/i);
  await page.locator("#pw").fill("a long enough panel password");
  await page.locator(".login-form .btn-primary").click();
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
});

test("signing out ends the session for good", async ({ page }) => {
  await page.goto(newLink());
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
  await page.locator(".rail-extra").click();
  await expect(page.locator("#login")).toBeVisible({ timeout: 15_000 });
  await page.reload();
  await expect(page.locator("#login")).toBeVisible({ timeout: 30_000 });
});
