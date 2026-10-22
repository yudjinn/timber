# Timber

A device tree viewer. Open a `.dtb` or `.dts`, add the kernel/BSP folders it comes from, or (in the desktop app) read a running device's tree over SSH. The tree is drawn as a web you grow outward: click a node to see its children, then pick the ones to add.

With source folders added, a DTB is matched to the board it was built from, so every node shows where it's defined and overridden, and anything the bootloader changed is flagged. Views export as SVG or PNG for documentation.

Status and plans live in [`docs/`](docs/README.md): design, decision log, and roadmap.

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
