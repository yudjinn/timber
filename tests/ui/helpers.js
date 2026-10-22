// Shared test setup: the fixture manifest, a `page` that fails the test on console
// errors, and the common steps (open files, search, mount a folder for the directory
// picker).

import { test as base, expect } from "@playwright/test";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

export { expect };

let cached;
/** What fixtures.js built (`board`, `kernel`; each may carry `skip`). */
export function fixtures() {
  cached ??= JSON.parse(readFileSync(new URL("./.fixtures/manifest.json", import.meta.url), "utf8"));
  return cached;
}

// Noise from the dev build's live-reload script, which has no trunk server to talk to.
const IGNORED = [/__trunk_address__/, /\/\.well-known\/trunk\/ws/];

export const test = base.extend({
  page: async ({ page }, use) => {
    const problems = [];
    page.on("console", (m) => {
      if (m.type() === "error" && !IGNORED.some((r) => r.test(m.text()))) problems.push(m.text());
    });
    page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
    await use(page);
    expect(problems, "errors in the browser console").toEqual([]);
  },
});

/** Skips the test unless the fixture group was built. */
export function needs(group) {
  const f = fixtures()[group];
  test.skip(!!f.skip, f.skip);
  return f;
}

/** Loads the app and dismisses the first-visit shortcuts help. */
export async function start(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Got it" }).click();
}

export const idle = (page) =>
  page.waitForFunction(() => !document.querySelector(".busy"), null, { timeout: 90_000 });

/** "Open DTB / DTS files…" (the first one on the page) with these files. */
export async function openFiles(page, files) {
  const [chooser] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.getByRole("button", { name: "Open DTB / DTS files…" }).first().click(),
  ]);
  await chooser.setFiles(files);
  await page.waitForSelector(".popup");
  await idle(page);
}

/** Global search: jump to the first match, then close the popup. */
export async function goTo(page, query) {
  await page.keyboard.press("/");
  await page.keyboard.type(query);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(600);
  await page.keyboard.press("Escape");
}

export const nodeTitle = async (page) => (await page.locator(".node-title").textContent()).trim();
export const panelText = async (page) => (await page.locator(".panel").textContent()).replace(/\s+/g, " ");

/** Every file under `dir`, as { relative path: text }. */
export function readTree(dir) {
  const out = {};
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else out[relative(dir, p)] = readFileSync(p, "utf8");
    }
  };
  walk(dir);
  return out;
}

/**
 * Makes `showDirectoryPicker` hand back a real directory handle named "linux" (from the
 * origin-private file system), as Chrome's folder dialog would. Call before `start`; then
 * `writeFolder` fills it.
 */
export async function useDirectoryPicker(page) {
  await page.addInitScript(() => {
    window.showDirectoryPicker = async () => (await navigator.storage.getDirectory()).getDirectoryHandle("linux");
  });
}

export async function writeFolder(page, files) {
  await page.evaluate(async (files) => {
    const root = await navigator.storage.getDirectory();
    const linux = await root.getDirectoryHandle("linux", { create: true });
    for (const [path, text] of Object.entries(files)) {
      const parts = path.split("/");
      let dir = linux;
      for (const p of parts.slice(0, -1)) dir = await dir.getDirectoryHandle(p, { create: true });
      const w = await (await dir.getFileHandle(parts.at(-1), { create: true })).createWritable();
      await w.write(text);
      await w.close();
    }
  }, files);
}

/** Adds the folder through the directory picker and waits until it's indexed. */
export async function addFolder(page) {
  await page.getByRole("button", { name: "Add source folder…" }).first().click();
  await page.waitForSelector(".toast >> text=Added linux", { timeout: 90_000 });
  await idle(page);
}
