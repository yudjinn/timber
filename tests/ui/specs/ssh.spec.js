// SSH (desktop) flows against a mocked desktop backend (window.__TAURI__): host-key
// prompts, the live session, the device's kernel config, recent devices, and passwords.
// The real backend is tested against a throwaway sshd in timber-desktop.

import { readFileSync } from "node:fs";
import { test, expect, needs, goTo, nodeTitle, idle } from "../helpers.js";

/** Installs the mock. Trees: `a` and `b` (DTB bytes, base64); `window.__mock` drives it. */
async function mockDesktop(page, a, b) {
  await page.addInitScript(({ a, b }) => {
    const bytes = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
    const trees = { a: bytes(a), b: bytes(b) };
    const mock = (window.__mock = { trusted: new Set(), changed: false, tree: "a", disconnects: [], fetches: 0 });
    const pack = (dtb) => {
      const path = new TextEncoder().encode("/proc/device-tree");
      const out = new Uint8Array(10 + path.length + dtb.length);
      out.set(new TextEncoder().encode("TMBL"), 0);
      out[4] = 0; // read with tar
      out[5] = 1; // /proc/config.gz is readable
      new DataView(out.buffer).setUint32(6, path.length, true);
      out.set(path, 10);
      out.set(dtb, 10 + path.length);
      return out.buffer;
    };
    const hostKey = (host, changed) => ({
      host, port: 22, algorithm: "ssh-ed25519",
      fingerprint: changed ? "SHA256:NEWNEWNEW" : "SHA256:abcdefghijk",
      old_fingerprint: changed ? "SHA256:abcdefghijk" : null, key: "ssh-ed25519 AAAAC3Nza",
    });
    const fail = (kind, message, host_key = null) => ({ kind, message, host_key, key_file: null });
    window.__TAURI__ = {
      event: { listen: async () => () => {} },
      core: {
        invoke: async (cmd, args) => {
          switch (cmd) {
            case "startup":
              return { files: [], sources: [], config: null, smoke: false, smoke_watch: false };
            case "ssh_connect": {
              const r = (mock.lastRequest = args.request);
              const host = r.target.replace(/^.*@/, "");
              if (!mock.trusted.has(host)) throw fail("host_key_unknown", `${host} hasn't been seen before`, hostKey(host, false));
              if (mock.changed) throw fail("host_key_changed", "changed", hostKey(host, true));
              if (host.startsWith("pw") && r.password == null) throw fail("need_password", `root@${host} needs a password`);
              return { id: 7, label: `root@${host}`, host };
            }
            case "ssh_trust_host":
              mock.trusted.add(args.host);
              mock.changed = false;
              return null;
            case "ssh_fetch":
              mock.fetches++;
              return pack(trees[mock.tree]);
            case "ssh_device_config":
              return new TextEncoder().encode("CONFIG_ARM64=y\nCONFIG_ARCH_ROCKCHIP=y\n").buffer;
            case "ssh_disconnect":
              mock.disconnects.push(args.id);
              return null;
            default:
              return null;
          }
        },
      },
    };
  }, { a: readFileSync(a).toString("base64"), b: readFileSync(b).toString("base64") });
}

const mock = (page, f) => page.evaluate(f);
const docTitle = (page) => page.locator(".doc-name").getAttribute("title");
const host = (page) => page.locator(".ssh-form input[placeholder^='root@']");
const toast = (page) => page.locator(".toast");

async function connect(page, target) {
  await page.getByRole("button", { name: "SSH", exact: true }).click();
  await host(page).fill(target);
  await page.locator(".ssh-form button[type=submit]").click();
}

test.beforeEach(async ({ page }) => {
  const board = needs("board");
  await mockDesktop(page, board.dtb, board.overlaidDtb);
  await page.goto("/");
  await page.getByRole("button", { name: "Got it" }).click();
});

test("first connection asks to trust the host key, then shows the live tree", async ({ page }) => {
  await connect(page, "root@rock5b");
  const panel = page.locator(".hostkey");
  await expect(panel).toContainText("First connection to rock5b.");
  await expect(panel).toContainText("SHA256:abcdefghijk");
  await panel.getByRole("button", { name: "Trust and connect" }).click();

  await expect(page.locator(".live-label")).toHaveText("root@rock5b");
  await expect(page.locator(".modal.ssh")).toHaveCount(0);
  await idle(page);
  expect(await docTitle(page)).toMatch(/^rock5b · live · \d+ nodes$/);

  // The device has /proc/config.gz: always asked (D-030).
  const offer = page.locator(".offer");
  await expect(offer).toContainText("rock5b has /proc/config.gz");
  await offer.getByRole("button", { name: "Use it" }).click();
  await expect(toast(page)).toContainText("Using config.gz from rock5b");
});

test("refresh: no changes, then a changed tree keeps the selection; auto-refresh", async ({ page }) => {
  await mock(page, () => window.__mock.trusted.add("rock5b"));
  await connect(page, "root@rock5b");
  await expect(page.locator(".live-bar")).toBeVisible();
  await idle(page);

  await page.locator(".live-bar").getByRole("button", { name: "Refresh" }).click();
  await expect(toast(page)).toHaveText("No changes on the device");

  await page.keyboard.press("Escape");
  await goTo(page, "pmic");
  await expect.poll(() => nodeTitle(page)).toBe("pmic@20");
  const before = await docTitle(page);
  await mock(page, () => (window.__mock.tree = "b"));
  await page.locator(".live-bar").getByRole("button", { name: "Refresh" }).click();
  await expect(toast(page)).toHaveText("The device's tree changed; updated");
  await idle(page);
  expect(await docTitle(page)).not.toBe(before);
  await expect.poll(() => nodeTitle(page)).toBe("pmic@20");

  // Auto-refresh every 5 s picks up the next change by itself.
  await page.locator(".live-bar select").selectOption("5");
  await mock(page, () => { window.__mock.tree = "a"; window.__mock.fetches = 0; });
  await expect.poll(() => docTitle(page), { timeout: 12_000 }).toBe(before);
  expect(await mock(page, () => window.__mock.fetches)).toBeGreaterThanOrEqual(1);
});

test("a reflashed device: recent devices, changed-key warning, re-trust", async ({ page }) => {
  await mock(page, () => window.__mock.trusted.add("rock5b"));
  await connect(page, "root@rock5b");
  await expect(page.locator(".live-bar")).toBeVisible();
  await page.locator(".live-bar").getByRole("button", { name: "Disconnect" }).click();
  await expect(page.locator(".live-bar")).toHaveCount(0);

  await mock(page, () => (window.__mock.changed = true));
  await page.getByRole("button", { name: "SSH", exact: true }).click();
  await expect(page.locator(".recent")).toContainText("root@rock5b");
  await page.locator(".recent-pick").first().click();
  const panel = page.locator(".hostkey.changed");
  await expect(panel).toContainText("The host key for rock5b has changed.");
  await expect(panel).toContainText("SHA256:abcdefghijk");
  await expect(panel).toContainText("SHA256:NEWNEWNEW");
  await panel.getByRole("button", { name: "Device was reflashed: trust the new key" }).click();
  await expect(page.locator(".live-label")).toHaveText("root@rock5b");
});

test("a device that needs a password; the password isn't saved", async ({ page }) => {
  await mock(page, () => window.__mock.trusted.add("pwbox"));
  await connect(page, "root@pwbox");
  await expect(page.locator(".form-error")).toHaveText("root@pwbox needs a password");
  await expect(page.locator("label.needed input[type=password]")).toBeFocused();
  await page.keyboard.type("hunter2");
  await page.keyboard.press("Enter");
  await expect(page.locator(".live-label")).toHaveText("root@pwbox");
  expect(await mock(page, () => window.__mock.lastRequest.password)).toBe("hunter2");
  expect(await mock(page, () => localStorage.getItem("timber.ssh.recent"))).not.toContain("hunter2");
});

test("opening a file ends the live session", async ({ page }) => {
  const board = needs("board");
  await mock(page, () => window.__mock.trusted.add("rock5b"));
  await connect(page, "root@rock5b");
  await expect(page.locator(".live-bar")).toBeVisible();
  await idle(page);
  await page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "other.dtb"));
    document.querySelector(".app").dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, readFileSync(board.dtb).toString("base64"));
  await expect(page.locator(".doc-name")).toHaveAttribute("title", /^other\.dtb/);
  await expect(page.locator(".live-bar")).toHaveCount(0);
  expect(await mock(page, () => window.__mock.disconnects)).toEqual([7]);
});
