import { expect, test } from "@playwright/test";
import { goTo, signIn } from "./helpers";

// A private network of each kind made through the dialog: a VXLAN one asks for its UDP port
// and shows it on its card; a GRE one has no port. Nothing is linked, so no server is touched.
test("a network is made as GRE or as VXLAN on a port of its own", async ({ page }) => {
  await signIn(page);
  await goTo(page, "networks");

  await page.getByRole("button", { name: "New network" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("button", { name: "GRE", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(dialog.getByLabel("UDP port")).toHaveCount(0);

  await dialog.getByRole("button", { name: "VXLAN", exact: true }).click();
  const port = dialog.getByLabel("UDP port");
  await expect(port).toHaveValue("4789");
  const make = dialog.getByRole("button", { name: "Make the network" });
  await port.fill("70000");
  await expect(dialog.getByText("A port is a number from 1 to 65535.")).toBeVisible();
  await expect(make).toBeDisabled();
  await port.fill("8472");
  await dialog.getByLabel("Name").fill("vx");
  await make.click();
  await expect(dialog).toHaveCount(0);

  const vx = page.locator(".net-card", { hasText: "vx" });
  await expect(vx.getByText("VXLAN · UDP 8472")).toBeVisible();

  await page.getByRole("button", { name: "New network" }).first().click();
  await page.getByRole("dialog").getByLabel("Name").fill("plain");
  // the first pool is vx's now
  await page.getByRole("dialog").getByRole("button", { name: /10\.88\.0\.0\/16/ }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Make the network" }).click();
  const plain = page.locator(".net-card", { hasText: "plain" });
  await expect(plain.locator(".tag", { hasText: /^GRE$/ })).toBeVisible();

  // Neither has links, so both can go again: the dialog says only the pool is freed.
  for (const card of [vx, plain]) {
    await card.getByRole("button", { name: "Delete" }).click();
    const confirm = page.getByRole("dialog");
    await expect(confirm.getByText("It has no links: only its address pool is freed.")).toBeVisible();
    await confirm.getByRole("button", { name: "Delete" }).click();
    await expect(confirm).toHaveCount(0);
    await expect(card).toHaveCount(0);
  }
});

// Add server offers VXLAN beside GRE: with no VXLAN network yet, the panel says it makes one.
test("a server can be added over a VXLAN link as well as a GRE one", async ({ page }) => {
  await signIn(page);
  await goTo(page, "servers");
  await page.locator(".page-band .btn-primary").click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: /Over a GRE link/ }).click();
  await expect(dialog.getByText(/a new private network, kariz/)).toBeVisible();
  await dialog.getByRole("button", { name: /Over a VXLAN link/ }).click();
  await expect(dialog.getByRole("button", { name: /Over a VXLAN link/ })).toHaveAttribute("aria-pressed", "true");
  await expect(dialog.getByText(/a new VXLAN network, kariz-vxlan/)).toBeVisible();
  await expect(dialog.getByText(/Only UDP to the network's port/)).toBeVisible();

  // Reverse, across a link: the kind is a choice there.
  await dialog.getByRole("button", { name: /The panel connects to it/ }).click();
  await dialog.getByLabel(/Across a private link/).check();
  await dialog.getByRole("button", { name: "VXLAN", exact: true }).click();
  await expect(dialog.getByText(/a new VXLAN network, kariz-vxlan/)).toBeVisible();
});

// A network over IPv6: chosen in the dialog, shown on its card, and its servers' IPv6
// addresses can be set on the same page.
test("a network runs over IPv6 when asked", async ({ page }) => {
  await signIn(page);
  await goTo(page, "networks");
  await page.getByRole("button", { name: "New network" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("button", { name: "IPv4", exact: true })).toHaveAttribute("aria-pressed", "true");
  await dialog.getByRole("button", { name: "IPv6", exact: true }).click();
  await expect(dialog.getByText(/GRE becomes ip6gre/)).toBeVisible();
  await dialog.getByLabel("Name").fill("six");
  await dialog.getByRole("button", { name: "Make the network" }).click();
  await expect(dialog).toHaveCount(0);
  const six = page.locator(".net-card", { hasText: "six" });
  await expect(six.getByText("GRE · IPv6")).toBeVisible();
  // the IPv6 address of a server is asked for once a network uses IPv6
  const own = page.getByLabel(/^IPv6 address of /).first();
  await own.fill("2001:db8::5");
  await page.locator(".addr-row", { has: own }).getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("This address was set for the server.").last()).toBeVisible();
  await six.getByRole("button", { name: "Delete" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
  await expect(six).toHaveCount(0);
});
