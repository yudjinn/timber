// Viewing a tree: the first-visit help, the child popup, search, following references,
// export, and the source viewer. Uses the repo's own fixture board.

import { test, expect, fixtures, needs, start, openFiles, goTo, nodeTitle, idle } from "../helpers.js";

test("the shortcuts show on startup until turned off", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Getting around")).toBeVisible();
  await page.getByRole("button", { name: "Got it" }).click();
  await expect(page.locator(".empty-card")).toBeVisible();
  await expect(page.getByRole("link", { name: "Timber on GitHub" })).toHaveAttribute("href", "https://github.com/yudjinn/timber");
  await page.reload();
  await expect(page.getByText("Getting around")).toBeVisible();
  await page.getByLabel("Don't show on startup").check();
  await page.getByRole("button", { name: "Got it" }).click();
  await page.reload();
  await expect(page.locator(".empty-card")).toBeVisible();
  await expect(page.getByText("Getting around")).toHaveCount(0);
});

test("browse a DTB: popup filter, drill down, add all, search, follow a reference", async ({ page }) => {
  const board = needs("board");
  await start(page);
  await openFiles(page, board.dtb);
  await expect(page.locator(".doc-name")).toHaveAttribute("title", /^board\.dtb · \d+ nodes$/);

  // Type to filter the root's children; Enter adds and drills into the match.
  await page.keyboard.type("soc");
  await expect(page.locator(".popup-row")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect.poll(() => nodeTitle(page)).toBe("soc");

  // Backspace goes back up; Ctrl+A adds every child shown.
  await page.keyboard.press("Backspace");
  await expect.poll(() => nodeTitle(page)).toBe("/");
  await page.keyboard.press("Control+a");

  // Search by name, then follow the first reference in the panel.
  await page.keyboard.press("Escape");
  await goTo(page, "pmic");
  await expect.poll(() => nodeTitle(page)).toBe("pmic@20");
  await page.locator(".prop", { hasText: "vcc-supply" }).locator("button.link").first().click();
  await expect.poll(() => nodeTitle(page)).toBe("regulator-vcc3v3");

  await page.getByRole("button", { name: "Fit" }).click();
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(page.locator("body")).toHaveCSS("background-color", /rgb\((\d|[1-5]\d),/);
});

test("phone width has no horizontal scroll", async ({ page }) => {
  const board = needs("board");
  await page.setViewportSize({ width: 390, height: 844 });
  await start(page);
  await openFiles(page, board.dtb);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});

test("export SVG, PNG and .dts", async ({ page }) => {
  const board = needs("board");
  await start(page);
  await openFiles(page, board.dtb);
  await page.keyboard.press("Control+a");
  await idle(page);

  const save = async (label) => {
    await page.getByRole("button", { name: "Export ▾" }).click();
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.locator(".menu-panel").getByRole("button", { name: label, exact: true }).click(),
    ]);
    return download;
  };
  const svg = await save("SVG");
  expect(svg.suggestedFilename()).toBe("board-web.svg");
  const svgText = (await (await svg.createReadStream()).toArray()).join("");
  expect(svgText).toContain("<svg");
  expect(svgText).toContain("regulator-vcc3v3");

  const png = await save("PNG");
  expect(png.suggestedFilename()).toBe("board-web.png");
  const pngBytes = Buffer.concat(await (await png.createReadStream()).toArray());
  expect(pngBytes.subarray(1, 4).toString()).toBe("PNG");

  const dts = await save(".dts source");
  const dtsText = (await (await dts.createReadStream()).toArray()).join("");
  expect(dtsText).toMatch(/^\/dts-v1\/;/);
});

test("source viewer: open from a property, follow #include and &label, go back", async ({ page }) => {
  await start(page);
  await openFiles(page, fixtures().board.sources);
  await page.keyboard.press("Escape");
  await goTo(page, "uart0");

  // "overridden board.dts:NN" on the status property opens the viewer at that line.
  await page.locator(".prop", { hasText: "status" }).locator(".src-link-btn", { hasText: "overridden" }).click();
  await expect(page.locator(".source-path")).toHaveText(/board\.dts:\d+$/);
  await expect(page.locator(".src-line.hl")).not.toHaveCount(0);

  await page.locator(".src-line .t-str.src-link", { hasText: "soc.dtsi" }).first().click();
  await expect(page.locator(".source-path")).toHaveText(/soc\.dtsi(:\d+)?$/);
  await page.locator(".source-drawer").getByRole("button", { name: "Back" }).click();
  await expect(page.locator(".source-path")).toHaveText(/board\.dts:\d+$/);

  // A &label in the source selects that node.
  await page.locator(".src-line .t-ref", { hasText: "&uart0" }).first().click();
  await expect.poll(() => nodeTitle(page)).toMatch(/^serial@/);
  // Opening in an editor needs the desktop app.
  await expect(page.locator(".source-drawer").getByRole("button", { name: "Open in editor" })).toHaveAttribute("title", /desktop app/);
});
