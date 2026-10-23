// The toolbar holds together with a wide font (e.g. a desktop whose UI font is a
// programming font) and a long comparison label: one row above 1000 px, and nothing cut off
// at any width.

import { test, expect, needs, start, openFiles, idle } from "../helpers.js";

const WIDE_FONT = `* { font-family: "Hack Nerd Font Propo", "DejaVu Sans Mono", monospace !important; }`;

for (const width of [1845, 1280, 1000, 800]) {
  test(`toolbar fits at ${width}px with a wide font`, async ({ page }) => {
    const board = needs("board");
    await page.setViewportSize({ width, height: 900 });
    await start(page);
    await page.addStyleTag({ content: WIDE_FONT });
    await openFiles(page, board.dtb);
    // A long label: compared with another file.
    await page.getByRole("button", { name: /^Sources/ }).click();
    const [chooser] = await Promise.all([
      page.waitForEvent("filechooser"),
      page.locator(".modal.sources").getByRole("button", { name: "Compare with a DTB…" }).click(),
    ]);
    await chooser.setFiles(board.overlaidDtb);
    await idle(page);

    const toolbar = page.locator(".toolbar");
    const box = await toolbar.boundingBox();
    if (width > 1000) expect(box.height, "one row").toBeLessThan(60);
    const overflow = await page.evaluate(() =>
      [...document.querySelectorAll(".toolbar > *")]
        .filter((el) => el.getBoundingClientRect().right > window.innerWidth + 0.5)
        .map((el) => el.className || el.tagName),
    );
    expect(overflow, "items past the right edge").toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await expect(page.locator(".toolbar button.diag").filter({ hasText: /Δ|differ/ })).toBeVisible();
  });
}
