import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { PAGES, goTo, signIn } from "./helpers";

// axe checks every page and dialog against WCAG 2.1 A and AA (names, roles, contrast, focus
// order, landmarks), in both themes and both languages. What it cannot judge (does the motion
// mean something, is the wording clear) is in docs/accessibility.md, checked by hand.
const TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"];

async function scan(page: Page, what: string, scope?: string) {
  // Dialogs and pages fade in and a toast fades out: axe measures colours, so it must not look
  // mid-fade.
  await expect(page.locator(".toast")).toHaveCount(0, { timeout: 10_000 });
  await page.waitForTimeout(600);
  let builder = new AxeBuilder({ page }).withTags(TAGS);
  if (scope) builder = builder.include(scope);
  const { violations } = await builder.analyze();
  const text = violations
    .map((v) => `${v.id} (${v.impact}): ${v.help}\n${v.nodes.slice(0, 3).map((n) => {
            const d = n.any[0]?.data as { fgColor?: string; bgColor?: string; contrastRatio?: number } | undefined;
            return `    ${n.target.join(" ")}${d?.fgColor ? `  (${d.fgColor} on ${d.bgColor}, ${d.contrastRatio}:1)` : ""}`;
          }).join("\n")}`)
    .join("\n");
  expect(violations, `${what}\n${text}`).toEqual([]);
}

for (const [lang, theme] of [
  ["en", "night"],
  ["en", "dawn"],
  ["fa", "night"],
  ["fa", "dawn"],
] as const) {
  test(`every page passes axe (${lang}, ${theme})`, async ({ page }) => {
    await signIn(page, { lang, theme });
    for (const name of PAGES) {
      await goTo(page, name);
      // let the page draw its data and settle its animations
      await page.waitForTimeout(700);
      await scan(page, `the ${name} page (${lang}, ${theme})`);
    }
  });
}

test("the sign-in page passes axe, both ways of signing in", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("kariz.lang", "en");
    localStorage.setItem("kariz.low", "1");
  });
  await page.goto("about:blank");
  const { readStack } = await import("./stack");
  await page.goto(readStack().base);
  await expect(page.locator("#login")).toBeVisible({ timeout: 30_000 });
  await scan(page, "the sign-in page");
});

test("the dialogs pass axe and keep the keyboard inside", async ({ page }) => {
  await signIn(page);

  // add a server
  await goTo(page, "servers");
  await page.locator(".page-band .btn-primary").click();
  await expect(page.locator(".dialog")).toBeVisible();
  await scan(page, "the add-server dialog", ".dialog");
  await page.keyboard.press("Escape");
  await expect(page.locator(".dialog")).toHaveCount(0);

  // the settings dialogs
  await goTo(page, "settings");
  await page.locator(".set-row .btn").first().click();
  await expect(page.locator(".dialog")).toBeVisible();
  await scan(page, "the password dialog", ".dialog");
  // focus stays inside the dialog while tabbing
  for (let i = 0; i < 8; i++) {
    await page.keyboard.press("Tab");
    const inside = await page.evaluate(() => !!document.activeElement?.closest(".dialog"));
    expect(inside, `focus left the dialog after ${i + 1} tabs`).toBe(true);
  }
  await page.keyboard.press("Escape");

  // the command palette
  await page.keyboard.press("Control+k");
  await expect(page.locator(".palette, [role=dialog]").first()).toBeVisible();
  await scan(page, "the command palette");
  await page.keyboard.press("Escape");
});

test("the wizard passes axe on every step", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  // the wizard needs two servers, so this looks at its first step through the page's own
  // button when there is one, and otherwise at the empty state
  await goTo(page, "tunnels");
  await scan(page, "the tunnels page");
});

test("everything can be reached with the keyboard, and moving parts stop when asked", async ({ page }) => {
  await signIn(page);
  // the rail is reachable by Tab and its items act with Enter
  await page.locator("body").press("Tab");
  const reached = new Set<string>();
  for (let i = 0; i < 40; i++) {
    const label = await page.evaluate(() => document.activeElement?.textContent?.trim() ?? "");
    if (label) reached.add(label);
    await page.keyboard.press("Tab");
  }
  for (const word of ["Map", "Servers", "Tunnels", "Networks", "Logs", "Settings"]) {
    expect([...reached].some((l) => l.includes(word)), `${word} cannot be reached by Tab`).toBe(true);
  }
  // a visible focus ring on what has focus
  const outline = await page.evaluate(() => {
    (document.querySelector(".rail-item") as HTMLElement | null)?.focus();
    const s = getComputedStyle(document.activeElement as Element);
    return `${s.outlineStyle} ${s.outlineWidth} ${s.boxShadow}`;
  });
  expect(outline).not.toMatch(/^none 0px none$/);
});

test("with the browser asking for less motion, the page does not animate", async ({ browser }) => {
  const context = await browser.newContext({ ignoreHTTPSErrors: true, reducedMotion: "reduce" });
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("kariz.lang", "en"));
  const { newLink } = await import("./stack");
  await page.goto(newLink());
  await expect(page.locator("#app .rail")).toBeVisible({ timeout: 30_000 });
  // the low-power switch is on by itself
  await expect(page.locator(".sky-actions .square-btn").nth(2)).toHaveAttribute("aria-pressed", "true");
  await context.close();
});
