# Timber

A device tree viewer. Open a `.dtb`, a `.dts` with its includes, or (soon) a BSP folder or a live device over SSH. The tree is drawn as a web you grow outward: click a node to see its children, then pick the ones to add.

Status and plans live in [`docs/`](docs/README.md): design, decision log, and roadmap.

## Running

Requirements: Rust (stable, edition 2024), the `wasm32-unknown-unknown` target, and [trunk](https://trunkrs.dev).

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

trunk serve              # web build at http://127.0.0.1:8080
trunk build --release    # static site in dist/
```

## Development

```sh
cargo test                       # unit tests + golden tests against dtc (needs cpp and dtc on PATH)

# Compare against every board and overlay in a kernel checkout:
TIMBER_KERNEL_DIR=~/src/linux TIMBER_KERNEL_ARCH=arm64 cargo test --release --test golden kernel_tree

# Time and inspect a single compile:
cargo run --release -p timber-core --example compile -- ~/src/linux arch/arm64/boot/dts/rockchip/rk3588-rock-5b.dts --dts
```

| Crate | |
|---|---|
| `crates/timber-core` | Tree model, DTB reader/writer, DTS compiler (preprocessor, dtc-compatible), formatting |
| `crates/timber-ui` | Leptos UI compiled to wasm |
