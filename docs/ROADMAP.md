# Roadmap

Each milestone ends in a usable build. Tick items off as they land and note the date. The design is in [DESIGN.md](DESIGN.md); the reasoning is in [DECISIONS.md](DECISIONS.md).

Legend: `[ ]` todo · `[~]` in progress · `[x]` done

## M1 · Core library (`timber-core`) — done 2026-10-21
- [x] Tree model (arena, paths, aliases, phandle index, labels from `__symbols__`)
- [x] DTB reader/writer
- [x] Lexer + C integer expression evaluator
- [x] Preprocessor: includes, object- and function-like macros (`#`, `##`, variadic, multi-line calls), conditionals, line markers
- [x] `Vfs` trait + kernel-style include path resolution
- [x] DTS parser, merge semantics, `/delete-*`, phandle assignment, `/plugin/` fragments + `__fixups__`/`__local_fixups__`, `/omit-if-no-ref/`
- [x] Source map (definitions, overrides, deletions) and structured diagnostics with error recovery
- [x] Value formatting (phandle specifiers, `reg`/`ranges` grouping, strings, cells, bytes) + reverse reference index
- [x] DTS decompile (export); decompile → recompile round-trip test
- [x] Unit tests (31), fixtures vs `dtc`, and a kernel golden test: 3,733 boards/overlays, 0 differences (D-013, D-014)
- [x] `examples/compile.rs` dev tool (timing + diagnostics + `--dts` output)

## M2 · UI skeleton (web build) — done 2026-10-21
- [x] `timber-ui` crate (Leptos 0.8 CSR + trunk), `Backend` trait, `WebBackend`
- [x] Open `.dtb` / `.dts` files, by picker or drag and drop; "which file is the board?" chooser; loose headers resolve by unique basename
- [x] Graph canvas: force layout, pan/zoom/drag, styling by node kind, legend, label culling, `+N` hidden-children badges
- [x] Child popup with type-to-filter and Enter / Ctrl+Enter / Ctrl+A / Backspace navigation; click toggles (D-015)
- [x] Global search with match-type chips; selecting a result adds the path to it
- [x] Node panel: properties, reference links, "referenced by", reference edges, per-property source locations
- [x] Compiler messages drawer
- [x] Export `.dts` / `.dtb`; light/dark/auto theme; phone layout
- [x] Verified in headless Chromium (Playwright) on a real RK3588 DTB and the DTS fixtures; no console errors
- [ ] Later: check the Playwright smoke test into the repo and run it in CI

## M3 · Desktop shell (`timber-desktop`)
- [ ] Tauri v2 app wrapping `timber-ui`; `TauriBackend`
- [ ] Native open/save dialogs
- [ ] Greyed-out desktop-only features in the web build ("Requires the desktop app")

## M4 · BSP folders
- [ ] Folder indexing (desktop native; web via Chromium `showDirectoryPicker`)
- [ ] Config input: `.config` / defconfig / Yocto `KERNEL_DEVICETREE`
- [ ] dts Makefile evaluation → board picker (active first)
- [ ] Source viewer drawer; open in editor (D-008)
- [ ] Folder watcher + recompile (desktop); Reload button (web)
- [ ] Overlay application + `*-dtbs` composites (D-009)

## M5 · SSH (desktop)
- [ ] Connect dialog; ssh-config aliases
- [ ] Auth: agent, key files, none, password, keyboard-interactive
- [ ] TOFU host keys in Timber's own `known_hosts` (D-007)
- [ ] Live fetch: `tar` procfs → `od` walker → `/sys/firmware/fdt`; tar → `Tree`
- [ ] `/proc/config.gz`; board auto-match by `compatible`

## M6 · Diff
- [ ] Phandle-normalised tree diff
- [ ] Diff colouring in the graph, filtered popup/search, side-by-side values
- [ ] Summary panel

## M7 · Packaging
- [ ] GitHub Actions release matrix (Linux AppImage/deb, Windows msi/exe, macOS dmg)
- [ ] Static web deploy
