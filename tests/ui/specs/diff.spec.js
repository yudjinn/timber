// Comparing trees (M6, D-032…D-035): Compare with a DTB, ghosts, side-by-side values,
// Only differences, the summary and its Markdown, and the default comparison with source.

import { readFileSync } from "node:fs";
import { test, expect, needs, start, openFiles, goTo, nodeTitle, idle, panelText, readTree, useDirectoryPicker, writeFolder, addFolder } from "../helpers.js";

const diffButton = (page) => page.locator(".toolbar button.diag .long").filter({ hasText: /differ from|Same as/ });

async function compareWithFile(page, file) {
  await page.getByRole("button", { name: /^Sources/ }).click();
  const [chooser] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.locator(".modal.sources").getByRole("button", { name: "Compare with a DTB…" }).click(),
  ]);
  await chooser.setFiles(file);
  await idle(page);
}

test("compare with another DTB: ghosts, summary, Markdown, Only differences", async ({ page, context }) => {
  const board = needs("board");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await start(page);
  await openFiles(page, board.dtb);
  await compareWithFile(page, board.overlaidDtb);
  // The overlay added /soc/csi and a camera under i2c1 (plus its port and endpoint), and
  // changed i2c1 and /soc.
  await expect(diffButton(page)).toHaveText(/^\d+ differ from board-overlaid\.dtb$/);
  await expect(page.locator(".doc-name")).toHaveAttribute("title", /^board\.dtb · \d+ nodes$/);

  // The summary lists what's gone, as subtree roots.
  await diffButton(page).click();
  const summary = page.locator(".drawer.summary");
  await expect(summary).toContainText("Differences from board-overlaid.dtb");
  await expect(summary).toContainText("Only in board-overlaid.dtb");
  await expect(summary.getByRole("button", { name: /\/soc\/csi$/ })).toBeVisible();
  const camera = summary.getByRole("button", { name: /camera@36$/ });
  await expect(camera).toBeVisible();

  // A ghost: dashed in the popup, its properties come from the other tree.
  await camera.click();
  await expect.poll(() => nodeTitle(page)).toBe("camera@36");
  expect(await panelText(page)).toContain("Only in board-overlaid.dtb: this node isn't in the tree shown.");
  expect(await panelText(page)).toContain('"acme,camera"');
  await page.keyboard.press("Escape");
  // Its parent's popup (via the breadcrumbs) lists it, dashed.
  await page.locator(".crumbs .crumb button").nth(-2).click();
  await expect(page.locator(".popup-row.ghost", { hasText: "camera@36" })).toBeVisible();

  // Only differences hides unchanged children.
  const rows = await page.locator(".popup-row").count();
  await page.locator(".popup").getByRole("button", { name: "Only differences" }).click();
  expect(await page.locator(".popup-row").count()).toBeLessThan(rows);
  await expect(page.locator(".popup-row.ghost", { hasText: "camera@36" })).toBeVisible();
  await page.locator(".popup").getByRole("button", { name: "Only differences" }).click();

  // Markdown for a bug report.
  await summary.getByRole("button", { name: "Copy as Markdown" }).click();
  await expect(page.locator(".toast")).toContainText("Copied the differences as Markdown");
  const md = await page.evaluate(() => navigator.clipboard.readText());
  expect(md).toMatch(/^## Differences: board\.dtb vs board-overlaid\.dtb\n/);
  expect(md).toContain("### Only in board-overlaid.dtb");
  expect(md).toMatch(/- `\/soc\/i2c@[0-9a-f]+\/camera@36` \(\+2 below\)/);

  // Ghosts never reach an export.
  await page.getByRole("button", { name: "Export ▾" }).click();
  const [dts] = await Promise.all([
    page.waitForEvent("download"),
    page.locator(".menu-panel").getByRole("button", { name: ".dts source", exact: true }).click(),
  ]);
  const text = (await (await dts.createReadStream()).toArray()).join("");
  expect(text).not.toContain("camera@36");

  // Stop comparing: no source here, so no differences shown.
  await page.getByRole("button", { name: /^Sources/ }).click();
  await page.locator(".modal.sources").getByRole("button", { name: "Stop comparing" }).click();
  await idle(page);
  await expect(diffButton(page)).toHaveCount(0);
});

test("side by side: the differing part of a value is highlighted", async ({ page }) => {
  const board = needs("board");
  await start(page);
  // The other way round: the overlaid tree compared with the plain board.
  await openFiles(page, board.overlaidDtb);
  await compareWithFile(page, board.dtb);
  await expect(diffButton(page)).toHaveText(/differ from board\.dtb$/);
  await goTo(page, "camera@36");
  await expect(page.locator(".node-kind")).toContainText("not in board.dtb");
  await goTo(page, "soc");
  await expect.poll(() => nodeTitle(page)).toBe("soc");
  // __symbols__ is ignored; /soc gained the csi node only, so it's not itself changed.
  await expect(page.locator(".node-kind")).toContainText("differ below");
});

test("the comparison stays when another tree is opened", async ({ page }) => {
  const board = needs("board");
  await start(page);
  await openFiles(page, board.dtb);
  await compareWithFile(page, board.dtb);
  await expect(diffButton(page)).toHaveText("Same as board.dtb");
  await page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "overlaid.dtb"));
    document.querySelector(".app").dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, readFileSync(board.overlaidDtb).toString("base64"));
  await expect(page.locator(".doc-name")).toHaveAttribute("title", /^overlaid\.dtb/);
  await expect(diffButton(page)).toHaveText(/\d+ differ from board\.dtb/);
});

test("from source by default; side by side; compare with a board; back to source", async ({ page }) => {
  const k = needs("kernel");
  await useDirectoryPicker(page);
  await start(page);
  await writeFolder(page, readTree(k.linux));
  await openFiles(page, k.deviceDtb);
  await addFolder(page);
  await expect(diffButton(page)).toHaveText("3 differ from source");

  // status "disabled" on the device vs "okay" in the source.
  await goTo(page, "i2c@fec80000");
  const status = page.locator(".prop", { hasText: "status" });
  await expect(status.locator(".prop-compare")).toBeVisible();
  await expect(status.locator(".tok-changed").first()).toHaveText('"disabled"');
  await expect(status.locator(".tok-changed").nth(1)).toHaveText('"okay"');
  await expect(status).toContainText("likely set by U-Boot");

  // The same board, picked explicitly: same differences, named after the board.
  await page.getByRole("button", { name: /^Sources/ }).click();
  const section = page.locator(".modal.sources section", { hasText: "Compare with" });
  await section.getByPlaceholder("Compare with a board…").fill("rk3588-rock-5b.dts");
  await section.locator(".picker-row").first().click();
  await idle(page);
  await expect(diffButton(page)).toHaveText("3 differ from rk3588-rock-5b.dts");
  await page.getByRole("button", { name: /^Sources/ }).click();
  await page.locator(".modal.sources").getByRole("button", { name: "Back to source" }).click();
  await idle(page);
  await expect(diffButton(page)).toHaveText("3 differ from source");
});
