import { expect, test } from "@playwright/test";
import { PAGES, goTo, signIn } from "./helpers";

test("the language switch turns the page around, and the choice is kept", async ({ page }) => {
  await signIn(page, { lang: "en" });
  await expect(page.locator("html")).toHaveAttribute("dir", "ltr");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.locator(".sky-actions .square-btn").first().click();
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator("html")).toHaveAttribute("lang", "fa");
  // Persian text and digits
  await expect(page.locator(".rail")).toContainText("نقشه");
  await page.reload();
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
});

test("Night and Dawn are both there, and the choice is kept", async ({ page }) => {
  await signIn(page, { theme: "night" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "night");
  await page.locator(".sky-actions .square-btn").nth(1).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dawn");
  await page.reload();
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dawn");
});

for (const lang of ["en", "fa"] as const) {
  test(`no page scrolls sideways on a phone (${lang})`, async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await signIn(page, { lang });
    for (const name of PAGES) {
      await goTo(page, name);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, `${name} is wider than the phone by ${overflow}px`).toBeLessThanOrEqual(1);
    }
  });
}

test("the command palette opens with Ctrl+K and goes to a page", async ({ page }) => {
  await signIn(page);
  await page.keyboard.press("Control+k");
  const palette = page.locator("[role=dialog], .palette").first();
  await expect(palette).toBeVisible();
  await page.keyboard.type("servers");
  await page.keyboard.press("Enter");
  await expect(page.locator("h1.page-title")).toContainText(/servers/i);
});
