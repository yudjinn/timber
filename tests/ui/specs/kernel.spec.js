// Source folders against a kernel checkout (TIMBER_KERNEL_DIR): matching a device's DTB to
// its board, differences from source, the directory picker with Reload, kernel configs,
// and overlays. Skipped without a kernel.

import { test, expect, needs, start, openFiles, goTo, idle, panelText, readTree, useDirectoryPicker, writeFolder, addFolder } from "../helpers.js";

const diffButton = (page) => page.locator(".toolbar button.diag .long", { hasText: "differ from source" });

test("a device DTB is matched to its board and its differences are flagged", async ({ page }) => {
  const k = needs("kernel");
  await useDirectoryPicker(page);
  await start(page);
  await writeFolder(page, readTree(k.linux));
  await openFiles(page, k.deviceDtb);
  await addFolder(page);

  // U-Boot's bootargs and MAC, and the disabled I2C controller.
  await expect(diffButton(page)).toHaveText("3 differ from source");
  await page.getByRole("button", { name: /^Sources/ }).click();
  await expect(page.locator(".modal.sources")).toContainText("rk3588-rock-5b.dts");
  await expect(page.locator(".modal.sources")).toContainText("nothing is uploaded");
  await page.keyboard.press("Escape");

  // In the popup, on the panel, and "Show differences".
  await page.keyboard.type("i2c@fec8");
  await expect(page.locator(".popup-row .diff-pill").first()).toBeVisible();
  await page.keyboard.press("Escape");
  await goTo(page, "i2c@fec80000");
  await expect(page.locator(".node-kind .chip.warn").first()).toBeVisible();
  expect(await panelText(page)).toContain("differs from source");
  // Labels come from the source even though the DTB has none.
  await goTo(page, "chosen");
  expect(await panelText(page)).toContain("bootargs");
  await diffButton(page).click();
});

test("the directory picker reads only sources, and Reload picks up edits", async ({ page }) => {
  const k = needs("kernel");
  await useDirectoryPicker(page);
  await start(page);
  await writeFolder(page, {
    ...readTree(k.linux),
    "drivers/gpio/should-not-be-read.h": "#error read by mistake\n",
    ".git/HEAD": "ref: refs/heads/master\n",
  });
  await openFiles(page, k.deviceDtb);
  await addFolder(page);

  await page.evaluate(async () => {
    let dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("linux");
    for (const p of ["arch", "arm64", "boot", "dts", "rockchip"]) dir = await dir.getDirectoryHandle(p);
    const file = await dir.getFileHandle("rk3588-rock-5b.dts");
    const text = await (await file.getFile()).text();
    const w = await file.createWritable();
    await w.write(text.replace('model = "Radxa ROCK 5B";', 'model = "Radxa ROCK 5B (edited)";'));
    await w.close();
  });
  await page.getByRole("button", { name: /^Sources/ }).click();
  await page.locator(".modal.sources").getByRole("button", { name: "Reload" }).click();
  await page.waitForSelector(".toast >> text=Reloaded linux", { timeout: 90_000 });
  await idle(page);
  await page.keyboard.press("Escape");
  const root = await panelText(page);
  expect(root).toContain("(edited)");
  expect(root).toContain("differs from source");
});

test("a kernel config lists the boards it builds first", async ({ page }) => {
  const k = needs("kernel");
  await useDirectoryPicker(page);
  await start(page);
  await writeFolder(page, readTree(k.linux));
  await addFolder(page);
  await page.getByRole("button", { name: /^Sources/ }).click();
  const section = page.locator(".modal.sources section", { hasText: "Kernel config" });
  const [chooser] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.locator(".modal").getByRole("button", { name: "Add config…" }).click(),
  ]);
  await chooser.setFiles(k.config);
  await expect(section).toContainText("test.config");
  const first = page.locator(".modal.sources .picker-row").first();
  await expect(first).toContainText("built");
  await first.click();
  await page.waitForSelector(".popup");
  await expect(page.locator(".doc-name")).toContainText("rk3588-rock-5b");
});

test("overlays: a composite board, and an overlay applied at boot", async ({ page }) => {
  const k = needs("kernel");
  await useDirectoryPicker(page);
  await start(page);
  await writeFolder(page, readTree(k.linux));
  await addFolder(page);
  await page.getByRole("button", { name: /^Sources/ }).click();
  const [chooser] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.locator(".modal").getByRole("button", { name: "Add config…" }).click(),
  ]);
  await chooser.setFiles(k.config);

  // The Makefile's rk3588-rock-5b-pcie-ep.dtb = rock-5b + the pcie-ep overlay.
  await page.locator(".modal.sources .picker-row", { hasText: "pcie-ep" }).first().click();
  await page.waitForSelector(".popup");
  await idle(page);
  await page.getByRole("button", { name: /^Sources/ }).click();
  await expect(page.locator(".modal.sources section").first()).toContainText("rk3588-rock-5b-pcie-ep.dtso");
  await page.keyboard.press("Escape");
  await goTo(page, "pcie3x4_ep");
  expect(await panelText(page)).toMatch(/set by rk3588-rock-5b-pcie-ep\.dtso:\d+/);

  // A device DTB that had the overlay applied by the bootloader: differences until the
  // same overlay is attached to its source board.
  await page.getByRole("button", { name: /^Sources/ }).click();
  const [c2] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.locator(".modal").getByRole("button", { name: "Open DTB / DTS files…" }).click(),
  ]);
  await c2.setFiles(k.overlaidDtb);
  await idle(page);
  await expect(diffButton(page)).toHaveText(/\d+ differ from source/);
  if (!(await page.locator(".modal.sources").count())) await page.getByRole("button", { name: /^Sources/ }).click();
  const boot = page.locator(".modal.sources section", { hasText: "Overlays applied at boot" });
  await boot.getByPlaceholder("Or add an overlay from the sources…").fill("pcie-ep");
  await boot.locator(".picker-row").first().click();
  await idle(page);
  await expect(diffButton(page)).toHaveCount(0);
  await page.keyboard.press("Escape");
  await goTo(page, "pcie3x4_ep");
  expect(await panelText(page)).toMatch(/set by [^ ]+ \(at boot\)/);
});
