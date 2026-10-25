# Timber

A device tree viewer. Open a `.dtb` or `.dts`, add the kernel/BSP folders it comes from, or (in the desktop app) read a running device's tree over SSH. The tree is drawn as a web you grow outward: click a node to see its children, then pick the ones to add.

With source folders added, a DTB is matched to the board it was built from, so every node shows where it's defined and overridden, and anything the bootloader changed is flagged. Views export as SVG or PNG for documentation.

Status and plans live in [`docs/`](docs/README.md): design, decision log, and roadmap.

**Your files stay on your computer.** The web build reads files inside the page, and its Content-Security-Policy stops the page from connecting anywhere but its own origin. The desktop app works from disk and talks only to the devices you connect to. See [SECURITY.md](SECURITY.md).

## Running

Requirements: Rust (stable, edition 2024), the `wasm32-unknown-unknown` target, and [trunk](https://trunkrs.dev).

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

trunk serve              # web build at http://127.0.0.1:8080
trunk build --release    # static site in dist/
```

## Desktop app

Linux needs WebKitGTK 4.1 (`webkit2gtk-4.1`); Windows and macOS use the system webview.

```sh
cargo install tauri-cli --version "^2" --locked
cd crates/timber-desktop
cargo tauri dev          # runs trunk serve and opens the app with live reload
cargo tauri build        # release bundles (AppImage/deb, msi, dmg) in target/release/bundle

# The binary takes files and source folders:
timber board.dtb --source ~/src/linux --source ~/bsp --config ~/src/linux/.config

# ...or connects to a device at launch (host, user@host:port, or an ~/.ssh/config alias):
timber --ssh root@192.168.1.50 --source ~/src/linux
timber --ssh rock5b --ssh-options "-J me@gateway"
```

**SSH:** toolbar → **SSH**. Logins try your agent, your keys, then a password (an empty one first, as many dev boards use). Host keys you accept are kept in Timber's own `known_hosts` in the app data folder (`~/.local/share/dev.timber.viewer/` on Linux); `~/.ssh/known_hosts` is read but never changed. Jump hosts (`-J`, `ProxyJump`) go through your system `ssh`, so they need key or agent logins.

## Development

```sh
cargo test                       # unit tests + golden tests against dtc (needs cpp and dtc on PATH)
.github/scripts/fetch-kernel.sh v7.2 ~/linux   # the sparse kernel checkout CI uses (~100 MB)
                                 # SSH tests start a throwaway sshd as you (skipped without /usr/bin/sshd)

# Compare against every board and overlay in a kernel checkout:
TIMBER_KERNEL_DIR=~/src/linux TIMBER_KERNEL_ARCH=arm64 cargo test --release --test golden kernel_tree
# ...and against dtc -@ builds:
TIMBER_KERNEL_SYMBOLS=1 TIMBER_KERNEL_DIR=~/src/linux cargo test --release --test golden kernel_tree

# UI tests: the web build in headless Chromium (needs Node, plus cpp/dtc/fdtoverlay for fixtures):
cd tests/ui && npm install && npx playwright install chromium
npm run test:all                                   # trunk build, then the tests
TIMBER_KERNEL_DIR=~/src/linux npm test             # also the kernel-backed tests (rk3588-rock-5b)

# Time and inspect a single compile:
cargo run --release -p timber-core --example compile -- ~/src/linux arch/arm64/boot/dts/rockchip/rk3588-rock-5b.dts --dts
```

| Crate | |
|---|---|
| `crates/timber-core` | Tree model, DTB reader/writer, DTS compiler (preprocessor, dtc-compatible), formatting |
| `crates/timber-ui` | Leptos UI compiled to wasm |
| `crates/timber-desktop` | Tauri 2 desktop app: native dialogs, folders from disk, watcher, SSH (russh) |

## Releases

CI (`.github/workflows/ci.yml`) runs on every push to `main` and every pull request. The full comparison against dtc (`golden.yml`) runs weekly, or from the Actions tab with any kernel tag.

To release:

1. Set the new version in `Cargo.toml` (`[workspace.package]`) and commit. It's the only place the version lives; the desktop app takes it from there.
2. Tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.
3. `release.yml` checks the tag matches the version and runs CI. It then builds the Linux (AppImage, .deb) and Windows (.msi, .exe) apps into a **draft** release, and deploys the web build to Cloudflare.
4. Check the draft under Releases and publish it.

**Cloudflare setup (once).** The web build runs on Cloudflare Workers as static assets (`wrangler.jsonc`). Nothing needs creating in the dashboard; the first deploy creates the Worker.
1. Cloudflare dashboard → **Manage account** → **Account API tokens** → **Create Token** → the **Edit Cloudflare Workers** template, scoped to your account.
2. Copy your **Account ID** (shown on the account's home page and in Workers & Pages).
3. In GitHub: **Settings** → **Secrets and variables** → **Actions**. Add the secrets `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. To name the Worker something other than `timber`, add the variable `CLOUDFLARE_WORKER_NAME`.
4. Run **Deploy web** from the Actions tab. The site appears at `https://timber.<your-subdomain>.workers.dev`. Add a custom domain under the Worker's **Settings** → **Domains & Routes**.

**The builds are unsigned:**
- **Windows:** SmartScreen warns on first run; choose *More info* → *Run anyway*.
- **Linux:** `chmod +x Timber_*.AppImage` before running it. Or install the .deb with `sudo apt install ./timber_*.deb`.

## How Timber was built

Timber was written with [Claude Code](https://claude.com/claude-code), Anthropic's AI coding tool, and the commits credit it as co-author. I designed Timber and reviewed the work: each feature was talked through before it was built, and every decision is recorded in [`docs/DECISIONS.md`](docs/DECISIONS.md) with the reasons and the alternatives that were rejected.

The tests are there so you don't have to take that on trust:
- **Device tree compiler:** compiled trees are compared node for node with `cpp | dtc` output, across every board and overlay in a Linux kernel checkout.
- **SSH:** tested against a real, throwaway `sshd`.
- **UI:** end-to-end tests of the web build in headless Chromium, including checks that the Content-Security-Policy blocks connections to other sites.

The commands to run them are under [Development](#development).

## License

[MIT](LICENSE). Timber reads Linux kernel sources at runtime and in its tests, but none are included in this repository.

