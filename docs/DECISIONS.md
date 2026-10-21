# Decision log

The decisions behind Timber's design, with their reasons and the alternatives we rejected. Newest decisions go at the bottom. Don't edit an accepted decision; add a new one that supersedes it.

Status values: **Accepted**, **Proposed** (awaiting sign-off), **Superseded by D-xxx**.

---

## D-001 · Cargo workspace with a shared `timber-core`
- **Status:** Accepted · 2026-10-21
- **Decision:** Split the project into crates:
  - `timber-core`: pure Rust, no I/O, builds for native and wasm
  - `timber-ui`: Leptos
  - `timber-desktop`: Tauri
- **Why:** Parsing, compiling, formatting and diffing are shared by the desktop backend and the wasm UI. Keeping I/O out of core means it can be tested with plain `cargo test`.

## D-002 · SSH runs natively in a Tauri backend
- **Status:** Accepted · 2026-10-21. Supersedes two rejected approaches.
- **Decision:** russh runs in the Tauri backend and opens TCP connections directly.
- **Rejected 1: a native helper server that runs the system `ssh` binary.** You wanted SSH to happen inside the app, not by shelling out.
- **Rejected 2: russh compiled to wasm, behind a WebSocket↔TCP relay**, the model used by [sshclient-wasm](https://github.com/VerdigrisTech/sshclient-wasm). russh does build for `wasm32-unknown-unknown` (verified with russh 0.64.1 and `ring`). But a browser cannot listen on or open TCP sockets, so the relay can't live inside the wasm. Chrome's Direct Sockets API is limited to Isolated Web Apps. That would have meant shipping a relay binary alongside the UI anyway.
- **Why:** You want one reliable application. Native SSH removes the relay entirely and gives us ssh-agent, `~/.ssh/config` and `known_hosts`.

## D-003 · Tauri v2 desktop app, cross-platform
- **Status:** Accepted · 2026-10-21
- **Decision:** Ship one Tauri v2 application for Linux, Windows and macOS.
- **Rejected:**
  - **A single native binary that serves the UI to a browser.** Feels less like an app (localhost port, browser tab, possible firewall prompt), and SSH would still take the long route.
  - **Pure native egui.** A true single binary with no webview, but it would mean dropping Leptos, and rich panels (source viewer, property tables) are harder to build.
- **Accepted cost:** on Linux the app depends on the system WebKitGTK. The AppImage bundles it.

## D-004 · Leptos (CSR) for the UI; graph drawn on a canvas
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - Leptos 0.8, client-side rendering, built with trunk.
  - The graph is a `<canvas>` inside a Leptos component, driven by a `requestAnimationFrame` loop.
  - Leptos signals only hold what's visible and selected, never per-frame positions.
- **Why:** You prefer Leptos. A canvas stays smooth with hundreds of animated nodes, where reactive DOM/SVG updates wouldn't.

## D-005 · A web build of the same UI, with desktop features greyed out
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - The UI talks to a `Backend` interface, and picks `TauriBackend` or `WebBackend` at runtime by checking whether it's inside Tauri. One build serves both.
  - In the web build, SSH, open-in-editor and other desktop-only features stay visible but disabled, labelled *"Requires the desktop app"*.
- **Why:** File-only viewing with nothing to install is useful, and it costs little if we plan for it from the start.

## D-006 · Web build: opening folders is Chromium-only
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - The web build opens folders with `showDirectoryPicker` and reads files lazily.
  - On Firefox/Safari, folder opening is greyed out ("Requires Chrome/Edge or the desktop app"). Opening single files still works.
- **Why:** The fallback, `<input webkitdirectory>`, reads every file into memory up front. That's around 1 GB for a full kernel tree.
- **Revisit:** if Firefox ships the File System Access API.

## D-007 · Accepted host keys go to Timber's own `known_hosts`
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - Host keys are trusted on first use.
  - Timber reads `~/.ssh/known_hosts` but never modifies it.
  - New keys are written to a `known_hosts` file in Timber's app data directory.
  - A changed key is a hard stop. Removing the old entry is done in Settings, not by clicking through.

## D-008 · Opening files in an editor
- **Status:** Accepted · 2026-10-21
- **Decision:** Try these in order:
  1. `$VISUAL`, then `$EDITOR`. Terminal editors are launched in the platform terminal.
  2. VS Code (`code -g file:line:col`).
  3. A custom command template in Settings, e.g. `{file}` / `{line}` / `{col}` placeholders.

## D-009 · Apply overlays onto base trees
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - Implement overlay application in `timber-core` as part of milestone 4. Details are in DESIGN §5.4.
  - It covers `.dtbo` and `.dtso` files and Makefile `*-dtbs` composites.
  - Overlay contents are recorded in the source map.
- **Why:**
  - Kernel Makefiles increasingly define boards as base + overlays.
  - Many devices boot with overlays already applied (Raspberry Pi `dtoverlay=`, capes, U-Boot `fdtoverlay`). Diffing the live tree against an unpatched base would show those changes as false differences.
  - Merging reuses the compiler's `&label { }` code.
- **Limitation:** label-targeted fragments need the base tree's `__symbols__`. Trees compiled from source always have them; prebuilt DTBs need `dtc -@`.

## D-010 · Folder-based DTS with config-driven board detection
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - The input is a local BSP or kernel folder.
  - Includes resolve the way the kernel's `-I` paths do.
  - Active boards come from evaluating the dts Makefiles' `dtb-$(CONFIG_…)` lines against a config. The config can be a `.config`/defconfig, `/proc/config.gz` read over SSH, or a Yocto `KERNEL_DEVICETREE` line.
  - When connected over SSH, the board whose `compatible` matches the device is selected automatically.
  - We don't parse the full Kconfig language.
- **Why:** You want to open BSP folders and see the "active" layout. The Makefiles plus `.config` values are what actually decide which boards get built.

## D-011 · Interaction: drill-down popup and global search
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - **Child popup:** clicking a node opens a popup with the filter already focused.
    - `Enter` adds the child and moves the popup onto it.
    - `Ctrl+Enter` adds it and stays.
    - `Backspace` in an empty filter goes back up to the parent.
  - **Global search (top-left):** searches node names, labels, aliases and `compatible` strings. Each result has a chip saying which of those matched. Selecting one adds every node on the path from the root.

## D-012 · Source-vs-live diff is in scope
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - Diff any two trees: typically the live tree from SSH against the tree compiled from the source folder.
  - Phandle numbers differ between compilations, so phandle cells are rewritten to the target node's path before comparing.
  - Details are in DESIGN §7.

## D-013 · The DTS compiler matches `dtc` exactly
- **Status:** Accepted · 2026-10-21. Replaces the original non-goal "byte-for-byte dtc compatibility".
- **Decision:** Compiled trees must be structurally identical to `cpp | dtc` output: the same nodes, properties, order, values and phandle numbers. The behaviours this requires are listed in DESIGN §5.2.
- **Why:**
  - **It turned out to be achievable.** The kernel golden test reached zero differences across all 3,733 arm64/arm/riscv boards and overlays.
  - **It makes the diff more trustworthy (D-012).** A tree compiled from source matches what `dtc` would have produced for the device. Even phandle numbers can be compared directly when the device was built from the same sources.
  - **Users see the same property and child order** they would see with `dtc -O dts`.

## D-014 · Golden tests use an external kernel checkout
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - Hand-written MIT fixtures live in the repo.
  - The full-kernel comparison runs against a checkout pointed to by `TIMBER_KERNEL_DIR`. A sparse, shallow clone is enough. CI does the same.
- **Why:** Kernel dts files and `dt-bindings` headers are under a mix of GPL-2.0 and dual licenses. Vendoring them would bring that licensing into the repo. Testing against the whole kernel is also far more thorough than a vendored slice.

## D-015 · Clicking a popup row toggles it
- **Status:** Accepted · 2026-10-21. Refines D-011.
- **Decision:**
  - Clicking a child in the popup adds it, or removes it with its subtree if it's already in the web (marked ✓).
  - The keyboard flow is unchanged: `Enter` drills down, `Ctrl+Enter` adds and stays.
- **Why:** With checkmarks, the expected behaviour of a click is to toggle. It also gives a mouse-only way to prune the web without hunting for the node on the canvas.

## D-016 · Node colours use the validated dataviz palette
- **Status:** Accepted · 2026-10-21
- **Decision:**
  - The eight colour-coded kinds take the reference categorical palette, in fixed order, stepped separately for light and dark.
  - Root and data-only nodes stay neutral grey.
- **Why:**
  - The palette passed the validator in both modes. The worst adjacent pair still has CVD ΔE 9.1 in light mode and 8.4 in dark, and normal-vision ΔE 19.6 / 19.3.
  - Three light-mode hues are below 3:1 contrast against the background. That's acceptable because every node also has a visible text label and an outline, so kind is never shown by colour alone. The legend and the panel's kind text also name the kind.

