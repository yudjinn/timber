// Global setup: builds the test inputs into .fixtures/ (gitignored) and writes
// .fixtures/manifest.json for the specs.
//
// - Always: the repo's own fixture board (crates/timber-core/tests/fixtures), compiled
//   with cpp + dtc, plus the same board with its overlay applied by fdtoverlay.
// - With TIMBER_KERNEL_DIR (a kernel checkout, like the golden tests): a small staged
//   kernel holding just what rk3588-rock-5b needs, a "device" DTB patched the way
//   U-Boot would (fdtput), and the board with the pcie-ep overlay applied. Nothing from
//   the kernel is committed (D-014).

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../..");
const out = join(here, ".fixtures");
const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { maxBuffer: 1 << 28, ...opts });

function have(tool) {
  try {
    run(tool, ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

/** cpp | dtc, the way the kernel builds device trees. */
function compile(file, includes, output, symbols = false) {
  const pre = run("cpp", ["-nostdinc", "-undef", "-D__DTS__", "-x", "assembler-with-cpp", ...includes.flatMap((i) => ["-I", i]), file]);
  const args = ["-q", "-I", "dts", "-O", "dtb", "-o", output, "-i", dirname(file), ...includes.flatMap((i) => ["-i", i])];
  if (symbols) args.push("-@");
  run("dtc", [...args, "-"], { input: pre });
}

/** The files `file` includes (cpp -M), relative to `root`. */
function deps(file, includes, root) {
  const text = run("cpp", ["-nostdinc", "-undef", "-D__DTS__", "-x", "assembler-with-cpp", "-M", ...includes.flatMap((i) => ["-I", i]), file]).toString();
  return text
    .replace(/\\\n/g, " ")
    .split(/\s+/)
    .slice(1)
    .filter((p) => p && !p.endsWith(":"))
    .map((p) => relative(root, p));
}

function boardFixtures(manifest) {
  const src = join(repo, "crates/timber-core/tests/fixtures");
  manifest.board = {
    sources: ["board.dts", "soc.dtsi", "include/dt-bindings/soc.h"].map((f) => join(src, f)),
  };
  if (!have("cpp") || !have("dtc") || !have("fdtoverlay")) {
    manifest.board.skip = "needs cpp, dtc and fdtoverlay on PATH";
    return;
  }
  const dir = join(out, "board");
  mkdirSync(dir, { recursive: true });
  const inc = [join(src, "include")];
  compile(join(src, "board.dts"), inc, join(dir, "board.dtb"));
  compile(join(src, "board.dts"), inc, join(dir, "board-symbols.dtb"), true);
  compile(join(src, "overlay.dtso"), inc, join(dir, "overlay.dtbo"), true);
  run("fdtoverlay", ["-i", join(dir, "board-symbols.dtb"), "-o", join(dir, "board-overlaid.dtb"), join(dir, "overlay.dtbo")]);
  Object.assign(manifest.board, {
    dtb: join(dir, "board.dtb"),
    overlaidDtb: join(dir, "board-overlaid.dtb"),
  });
}

function kernelFixtures(manifest) {
  const kernel = process.env.TIMBER_KERNEL_DIR;
  if (!kernel) {
    manifest.kernel = { skip: "set TIMBER_KERNEL_DIR to a kernel checkout to run" };
    return;
  }
  if (!have("cpp") || !have("dtc") || !have("fdtput") || !have("fdtoverlay")) {
    manifest.kernel = { skip: "needs cpp, dtc, fdtput and fdtoverlay on PATH" };
    return;
  }
  const rockchip = "arch/arm64/boot/dts/rockchip";
  const board = join(kernel, rockchip, "rk3588-rock-5b.dts");
  const overlay = join(kernel, rockchip, "rk3588-rock-5b-pcie-ep.dtso");
  if (!existsSync(board) || !existsSync(overlay)) {
    manifest.kernel = { skip: `${kernel} has no ${rockchip}/rk3588-rock-5b.dts and -pcie-ep.dtso` };
    return;
  }
  const inc = [join(kernel, "include"), join(kernel, "arch/arm64/boot/dts")];

  // A staged kernel: the board's and overlay's includes, the Makefiles, and a marker that
  // makes it look like a kernel root.
  const linux = join(out, "linux");
  rmSync(linux, { recursive: true, force: true });
  const files = new Set([
    ...deps(board, inc, kernel),
    ...deps(overlay, inc, kernel),
    "arch/arm64/boot/dts/Makefile",
    `${rockchip}/Makefile`,
  ]);
  for (const f of files) {
    mkdirSync(dirname(join(linux, f)), { recursive: true });
    cpSync(join(kernel, f), join(linux, f));
  }
  mkdirSync(join(linux, "scripts/dtc"), { recursive: true });
  writeFileSync(join(linux, "scripts/dtc/Makefile"), "# marker\n");

  const dir = join(out, "kernel");
  mkdirSync(dir, { recursive: true });
  // The board as a device would boot it: U-Boot set bootargs and a MAC address, and
  // something disabled an I2C controller.
  const device = join(dir, "rk3588-rock-5b.dtb");
  compile(board, inc, device);
  run("fdtput", ["-t", "s", device, "/chosen", "bootargs", "console=ttyS2,1500000 root=/dev/mmcblk0p2"]);
  run("fdtput", ["-t", "bx", device, "/ethernet@fe1c0000", "local-mac-address", "2", "4", "6", "8", "a", "c"]);
  run("fdtput", ["-t", "s", device, "/i2c@fec80000", "status", "disabled"]);
  // The board with the pcie-ep overlay applied at boot.
  const base = join(dir, "base.dtb");
  const dtbo = join(dir, "pcie-ep.dtbo");
  const overlaid = join(dir, "overlaid", "rk3588-rock-5b.dtb");
  mkdirSync(dirname(overlaid), { recursive: true });
  compile(board, inc, base, true);
  compile(overlay, inc, dtbo, true);
  run("fdtoverlay", ["-i", base, "-o", overlaid, dtbo]);

  const config = join(dir, "test.config");
  writeFileSync(config, "# Linux/arm64 Kernel Configuration\nCONFIG_ARM64=y\nCONFIG_ARCH_ROCKCHIP=y\n# CONFIG_ARCH_SUNXI is not set\n");
  manifest.kernel = { linux, deviceDtb: device, overlaidDtb: overlaid, config };
}

export default function setup() {
  mkdirSync(out, { recursive: true });
  const manifest = {};
  boardFixtures(manifest);
  kernelFixtures(manifest);
  writeFileSync(join(out, "manifest.json"), JSON.stringify(manifest, null, 2));
}
