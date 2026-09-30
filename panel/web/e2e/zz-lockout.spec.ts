import { expect, test } from "@playwright/test";
import { readStack, setPassword } from "./stack";

// Last of all, because it locks this machine's address out for 15 minutes.
test("five wrong passwords lock the address out, even for the right one", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kariz.lang", "en");
    localStorage.setItem("kariz.low", "1");
  });
  setPassword("a long enough panel password");
  await page.goto(readStack().base);
  await expect(page.locator("#login")).toBeVisible({ timeout: 30_000 });
  for (let i = 0; i < 5; i++) {
    await page.locator("#pw").fill(`wrong number ${i} of them`);
    await page.locator(".login-form .btn-primary").click();
    await expect(page.locator("#pw-err")).toContainText(/tries left|too many/i);
  }
  await expect(page.locator("#pw-err")).toContainText(/too many tries/i);
  // the right password is refused now, and no session is made
  await page.locator("#pw").fill("a long enough panel password");
  await page.locator(".login-form .btn-primary").click();
  await expect(page.locator("#pw-err")).toContainText(/too many tries/i);
  await expect(page.locator("#app .rail")).toHaveCount(0);
});
