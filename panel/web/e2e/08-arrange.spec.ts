import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { goTo, signIn } from "./helpers";

const cardNames = (page: Page) => page.locator("li.srv .srv-name").evaluateAll((els) => els.map((e) => e.firstChild?.textContent?.trim() ?? ""));
const rowNames = (page: Page) => page.locator(".arr-row .arr-name > span:first-child").allInnerTexts();

// The servers put in another order with the keyboard alone (the move buttons and Alt+Down): the
// order is saved, and after a reload the cards and the map show it.
test("the servers can be put in order with the keyboard and keep it", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  // The panel's own server, the agent of the stack, and the one added in an earlier spec.
  await expect.poll(async () => (await cardNames(page)).length).toBeGreaterThan(1);
  const before = await cardNames(page);

  await page.getByRole("button", { name: "Arrange" }).click();
  expect(await rowNames(page)).toEqual(before);
  const [first] = before;
  // The first goes down one place with its button, then to the end with Alt+Down: the focus
  // stays on the row that moves.
  await page.getByRole("button", { name: `Move ${first} down` }).click();
  await expect(page.getByRole("status").filter({ hasText: `${first} moved to place 2 of ${before.length}` })).toBeAttached();
  for (let i = 2; i < before.length; i++) await page.keyboard.press("Alt+ArrowDown");
  const wanted = [...before.slice(1), first];
  expect(await rowNames(page)).toEqual(wanted);
  await expect(page.getByRole("button", { name: `Move ${first} down` })).toBeDisabled();
  await expect(page.getByRole("button", { name: `Move ${first} up` })).toBeFocused();

  // Cancel forgets it.
  await page.getByRole("button", { name: "Cancel" }).click();
  expect(await cardNames(page)).toEqual(before);

  // Again, and saved this time: the last comes up to the top.
  await page.getByRole("button", { name: "Arrange" }).click();
  const last = before[before.length - 1];
  await page.getByRole("button", { name: `Move ${last} up` }).focus();
  for (let i = 1; i < before.length; i++) await page.keyboard.press("Alt+ArrowUp");
  const saved = [last, ...before.slice(0, -1)];
  expect(await rowNames(page)).toEqual(saved);
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".toast", { hasText: "The new order is saved." })).toBeVisible();
  await expect.poll(() => cardNames(page)).toEqual(saved);

  await page.reload();
  await goTo(page, "servers");
  await expect.poll(() => cardNames(page)).toEqual(saved);
  // The map lays the servers out in the same order.
  await goTo(page, "map");
  await expect(page.getByText(`Map of servers and tunnels: ${saved.join(", ")}`)).toBeAttached();
});

// The servers as one row each: the same servers in the same order, and the choice is kept.
test("the servers can be shown as rows, and the page remembers it", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  await expect.poll(async () => (await cardNames(page)).length).toBeGreaterThan(1);
  const cards = await cardNames(page);
  const rows = () => page.locator(".srv-rows tbody .sr-name").evaluateAll((els) => els.map((e) => e.firstChild?.textContent?.trim() ?? ""));

  await page.getByRole("button", { name: "Rows" }).click();
  await expect(page.getByRole("button", { name: "Rows" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("li.srv")).toHaveCount(0);
  expect(await rows()).toEqual(cards);
  // every row has its actions in a menu
  await page.getByRole("button", { name: `Actions for ${cards[0]}` }).click();
  await expect(page.getByRole("menuitem", { name: "Usage" })).toBeVisible();
  await page.keyboard.press("Escape");

  await page.reload();
  await goTo(page, "servers");
  await expect.poll(rows).toEqual(cards);
  await page.getByRole("button", { name: "Cards" }).click();
  await expect.poll(() => cardNames(page)).toEqual(cards);
});
