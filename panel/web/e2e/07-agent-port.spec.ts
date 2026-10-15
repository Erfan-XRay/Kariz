import { expect, test } from "@playwright/test";
import { goTo, signIn } from "./helpers";
import { freePort } from "./stack";

// A server added with an agents port of its own: a bad port is refused in the page, a free one
// gives a code, and the server waits in the list with its port on its card.
test("a server can be added with an agents port of its own", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  await page.locator(".page-band .btn-primary").click();
  const dialog = page.getByRole("dialog");
  await dialog.locator("#add-name").fill("own-port");
  const port = dialog.getByLabel("Agents port (optional)");
  const make = dialog.getByRole("button", { name: /Make a join code/i });

  await port.fill("65535");
  await expect(dialog.getByText(/A port is a number from 1 to 65534/)).toBeVisible();
  await expect(make).toBeDisabled();

  const free = await freePort();
  await port.fill(String(free));
  // The ports to open follow the port typed.
  await expect(dialog.locator("code", { hasText: String(free) }).first()).toBeVisible();
  await make.click();
  const shown = await page.locator("pre.code").first().innerText();
  expect(/kz1_[A-Za-z0-9_-]+/.test(shown), "a join code is shown").toBeTruthy();
  await page.keyboard.press("Escape");

  const card = page.locator("li.srv", { hasText: "own-port" });
  await expect(card.getByText(`agents port ${free}`)).toBeVisible();
});
