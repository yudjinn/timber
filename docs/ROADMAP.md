# Roadmap

Each milestone ends in a usable build. Tick items off as they land and note the date. The design is in [DESIGN.md](DESIGN.md); the reasoning is in [DECISIONS.md](DECISIONS.md).

Legend: `[ ]` todo · `[~]` in progress · `[x]` done

## M1 · Core library (`timber-core`)
- [~] Tree model (arena, paths, aliases, phandle index, labels from `__symbols__`). Draft exists.
- [~] DTB reader/writer. Draft exists.
- [ ] Lexer + C integer expression evaluator
- [ ] Preprocessor: includes, object- and function-like macros, conditionals, line markers
- [ ] `Vfs` trait + kernel-style include path resolution
- [ ] DTS parser, merge semantics, `/delete-*`, phandle assignment, `/plugin/` fragments
- [ ] Source map (definitions, overrides, deletions)
- [ ] Value formatting (phandle specifiers, `reg`/`ranges` grouping, strings, cells, bytes)
- [ ] DTS decompile (export)
- [ ] Tests, plus golden comparisons against `dtc`

## M2 · UI skeleton (web build)
- [ ] `timber-ui` crate (Leptos CSR + trunk), `Backend` trait, `WebBackend`
- [ ] Open `.dtb` / `.dts` files, by picker or drag and drop
- [ ] Graph canvas: force layout, pan/zoom/drag, styling by node kind, legend
- [ ] Child popup with type-to-filter and Enter / Ctrl+Enter / Backspace navigation
- [ ] Global search with match-type chips; selecting a result adds the path to it
- [ ] Node panel: properties, reference links, "referenced by", reference edges
- [ ] Export `.dts` / `.dtb`; light/dark theme

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
