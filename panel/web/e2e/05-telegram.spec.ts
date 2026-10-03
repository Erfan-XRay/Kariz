import { expect, test } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { goTo, signIn } from "./helpers";

const TOKEN = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

// Nothing here talks to Telegram: the panel stores the bot, shows a connect code and lists
// chats. Sending is covered by panel/tests/telegram.rs against a stand-in for Telegram.
test("the Telegram section keeps a bot, shows a connect code and never shows the token again", async ({ page }) => {
  await signIn(page);
  await goTo(page, "settings");
  const card = page.locator("#set-tg");
  await expect(card).toBeVisible();

  // Without a bot there is only the token field.
  await expect(card.locator("input[type=password]")).toBeVisible();
  await card.locator("input[type=password]").fill("not a token");
  await card.getByRole("button", { name: /save the bot/i }).click();
  await expect(page.locator(".toast, [role=status]").filter({ hasText: /bot token/i }).first()).toBeVisible();

  await card.locator("input[type=password]").fill(TOKEN);
  await card.getByRole("button", { name: /save the bot/i }).click();
  await expect(card.getByText("Bot …Dsaw")).toBeVisible();
  await expect(card).not.toContainText(TOKEN);

  // The page's own answer never carries the token either.
  const body = await page.evaluate(async () => (await fetch("./api/telegram", { credentials: "same-origin" })).text());
  expect(body).not.toContain("AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw");

  // A connect code is shown as the message to send.
  await card.getByRole("button", { name: /connect a chat/i }).click();
  await expect(card.locator("code")).toHaveText(/^\/start [0-9A-F]{8}$/);

  // The section passes axe.
  const results = await new AxeBuilder({ page }).include("#set-tg").analyze();
  expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);

  // Taking the bot away puts the section back to its first state.
  await card.getByRole("button", { name: /remove the bot/i }).click();
  await expect(card.locator("input[type=password]")).toBeVisible();
});
