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

## M2b · Sources, image export, shortcuts help (D-017…D-021) — done 2026-10-21
- [x] Core: board scan (root `compatible`/`model`, following includes for variant boards), board ranking, DTB↔source node mapping, property comparison by target path
- [x] Core: two-stage matching (file name shortlist + content score), U-Boot hints, build defines (D-021). Shared-buffer `MemVfs`
- [x] Sources panel: tree shown (DTB file / board from sources), folders and files list, add/remove, searchable board picker, per-DTB override remembered, build defines, Yocto tip
- [x] Folder reading in every browser (`webkitdirectory`, source files only; kernel trees read only dts/dt-bindings/uapi), combined VFS with per-entry mounts
- [x] Node panel: source locations for DTB nodes, "differs from source" with source value, "not in source", "only in source", "likely set by U-Boot", labels borrowed from source (also used for references and search), "Add source folder…" / "Choose board…"
- [x] Export menu: SVG, PNG (2×), `.dts`, `.dtb`; whole web / current view, theme, legend, title; one `Painter` for screen, PNG, SVG
- [x] Shortcuts popup on first visit; `?` and a toolbar button reopen it
- [x] Verified in headless Chromium: kernel checkout (8,442 source files, 3,728 boards) added as a folder in ~10 s; a U-Boot-patched Rock 5B DTB auto-matched at 100% with bootargs/MAC flagged; SVG/PNG downloads rendered
- [x] Kernel test `kernel_board_scan`: cheap scan agrees with full compiles on 3,317 / 3,326 boards

## M2c · Folder access polish (D-022) — done 2026-10-22
- [x] Directory access API in Chrome/Edge (read in place, pruned walk, Reload), `webkitdirectory` fallback
- [x] "Nothing is uploaded" wording in the Sources panel and empty state
- [x] Verified with a real directory handle (OPFS) in headless Chromium: pruning, 100% match, Reload picks up an edit

## M2d · Differences on the graph (D-023) — done 2026-10-22
- [x] Per-node diff classification on link (same / differs n / not in source) with subtree counts
- [x] Graph pills, dashed ring for not-in-source, amber hidden-children badges; included in exports
- [x] Popup row pills, panel header chip, toolbar "N differ from source", Sources summary, Show differences
- [x] Verified in headless Chromium with a DTB patched like U-Boot plus a disabled controller: 3 nodes flagged and revealed

## Fix · `dtc -@` fidelity (D-024) — done 2026-10-22
- [x] `-@` compile mode (symbols, labelled omit-nodes kept, phandles for labelled nodes), label prepend order, deleted-label phandle quirk
- [x] Annotation compiles in `-@` mode for DTBs with `__symbols__`; the samples DTB now shows 0 differences
- [x] Kernel golden test in both modes: 3,733 sources, 0 differences each

## M3 · Desktop shell (`timber-desktop`) — done 2026-10-22 (D-025)
- [x] Tauri 2.12 app wrapping `timber-ui`; `TauriBackend` picked automatically; one ~8.6 MB `timber` binary; app icon
- [x] Native open/save dialogs (save reports cancel); folders read from disk with the shared `timber_core::folder` rules
- [x] Binary file transfer (`TMBR` batch) instead of JSON
- [x] Folder watcher → automatic reload (pulled forward from M4)
- [x] Command line: `timber FILE… --source DIR…`, `--smoke`, `--smoke-watch`
- [x] Greyed-out desktop-only features in the web build ("Requires the desktop app"); desktop SSH says "arrives in M5"
- [x] Verified: `--smoke` (kernel tree + patched DTB → 100% match, 3 differences, under 3 s); `--smoke-watch` (editing a .dts while open → reloaded, 4 differences); web build re-checked in headless Chromium
- [x] Manual check of the native dialogs and Open in editor on Linux (2026-10-22)
- [ ] Manual check on Windows and macOS

## M4 · BSP folders — done 2026-10-22
- [x] Folder indexing (desktop native, M3; web, M2b/M2c)
- [x] Config input: `.config` / defconfig / Yocto `KERNEL_DEVICETREE` (panel, drop, `--config`); arch from symbols or inferred from Makefiles
- [x] dts Makefile evaluation (`dtb-$(CONFIG_…)`, subdirs, composites, `DTC_FLAGS -@`, variables, conditionals) → board picker (built first, chips); built boards win matching ties; kernel test `kernel_makefiles`
- [x] Source viewer drawer (syntax highlight, block highlight, include/label links, history); open in editor via template (D-026); Settings dialog; clickable source locations in the node panel and compiler messages
- [x] Folder watcher + recompile (desktop, M3); Reload button (web, M2c)
- [x] Core overlay application (libfdt semantics); kernel test `kernel_overlays`: 473/473 composites match `fdtoverlay`
- [x] UI: two overlay lists (D-027) with reorder/remove, from files or sources; composites in the board picker (only when buildable); "set by / added by overlay" source links
- [x] Verified in headless Chromium: composite `rk3588-rock-5b-pcie-ep` builds with overlay source links; a DTB with the overlay applied at boot goes from 4 differences to 0 once the overlay is added under "applied at boot"
- [x] Esc closes dialogs even while typing in one of their fields

## M5 · SSH (desktop) (D-028…D-031)
- [x] russh client in the desktop app; `~/.ssh/config` (Host patterns, Include, HostName/User/Port/IdentityFile/ProxyJump/ProxyCommand/ConnectTimeout)
- [x] Auth: agent, key files (+ passphrase), none, password (incl. empty), keyboard-interactive (password-style)
- [x] Host keys: TOFU into Timber's `known_hosts`, read-only `~/.ssh/known_hosts`, changed-key re-trust (D-028)
- [x] Extra options: `-J`/ProxyJump via system ssh, ProxyCommand, `-p/-l/-i/-o …` (D-029)
- [x] Fetch: tar (gzipped when possible) → od → /sys/firmware/fdt; tar/od → `Tree`; configurable tree path
- [x] `/proc/config.gz` offered as kernel config (D-030)
- [x] Connect dialog, recent devices, live session with Refresh / auto-refresh, reconnect after reboot (D-031)
- [x] `timber --ssh TARGET [--ssh-options …] [--tree-path …]`
- [x] Verified: throwaway-sshd tests (key auth, passphrases, unknown/changed host keys, ProxyCommand, tar.gz/tar/od, config.gz, reconnect); `--ssh … --smoke` against the Rock 5B tree served as a fake `/proc/device-tree` (813 nodes, rk3588-rock-5b.dts at 100%, 3 differences, the same as the DTB file); UI flows in headless Chromium against a mocked backend
- [ ] Manual check against a real board: password and keyboard-interactive logins (an unprivileged sshd can't test them), a real `/proc/device-tree`, busybox `tar`/`od`
- [ ] Manual check on Windows (agent over the OpenSSH named pipe, ProxyCommand via `cmd /C`) and macOS

## M6 · Diff
- [ ] Phandle-normalised tree diff
- [ ] Compare two arbitrary trees (DTB vs DTB, live vs source); filtered popup/search; side-by-side values with byte highlights
- [ ] Summary panel

## M7 · Packaging
- [ ] GitHub Actions release matrix (Linux AppImage/deb, Windows msi/exe, macOS dmg)
- [ ] Static web deploy
