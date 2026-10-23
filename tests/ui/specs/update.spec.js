// Noticing a newer build (D-036): the web build asks, the desktop app in dev mode reloads.
// "A newer build" is simulated by answering the app's check of the page with one that names
// a different wasm.

import { test, expect, start } from "../helpers.js";

const isPage = (url) => new URL(url).pathname === "/";

/** From now on, the app's own check of the page sees a newer build. */
async function newerBuildOnServer(page) {
  await page.route(isPage, async (route) => {
    if (route.request().resourceType() !== "fetch") return route.continue();
    const response = await route.fetch();
    const body = (await response.text()).replaceAll(/timber-ui-[0-9a-f]{16}_bg\.wasm/g, "timber-ui-0000000000000000_bg.wasm");
    await route.fulfill({ response, body });
  });
}

const focus = (page) => page.evaluate(() => window.dispatchEvent(new Event("focus")));
const notice = (page) => page.getByRole("alertdialog", { name: "New version" });
const badge = (page) => page.getByRole("button", { name: "Update available" });

test("web: a newer build asks to reload; Later leaves a badge until the next load", async ({ page }) => {
  await start(page);
  await focus(page);
  await page.waitForTimeout(300);
  await expect(notice(page)).toHaveCount(0);
  await expect(badge(page)).toHaveCount(0);

  await newerBuildOnServer(page);
  await focus(page);
  await expect(notice(page)).toContainText("A new version of Timber is available");
  await notice(page).getByRole("button", { name: "Later" }).click();
  await expect(notice(page)).toHaveCount(0);
  await expect(badge(page)).toBeVisible();

  // Not asked again before the next load.
  await focus(page);
  await page.waitForTimeout(300);
  await expect(notice(page)).toHaveCount(0);

  // The badge brings the notice back; Reload loads the new build.
  await badge(page).click();
  await page.unroute(isPage);
  await Promise.all([page.waitForEvent("load"), notice(page).getByRole("button", { name: "Reload" }).click()]);
  await expect(page.getByText("Getting around")).toBeVisible();
  await expect(badge(page)).toHaveCount(0);
});

test("desktop in dev mode: reloads by itself, once", async ({ page }) => {
  await page.addInitScript(() => {
    sessionStorage.setItem("loads", String(Number(sessionStorage.getItem("loads") || 0) + 1));
    window.__TAURI__ = {
      event: { listen: async () => () => {} },
      core: {
        invoke: async (cmd) =>
          cmd === "startup" ? { files: [], sources: [], config: null, smoke: false, smoke_watch: false } : null,
      },
    };
  });
  // The server keeps answering with a build the reload doesn't get: one reload, then the badge.
  await newerBuildOnServer(page);
  await page.goto("/");
  await expect(badge(page)).toBeVisible();
  expect(await page.evaluate(() => sessionStorage.getItem("loads"))).toBe("2");
  await expect(notice(page)).toHaveCount(0);
});
