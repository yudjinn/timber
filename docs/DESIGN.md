# Timber — Design

Status: **approved** · 2026-10-21

Related docs: [decision log](DECISIONS.md) · [roadmap](ROADMAP.md) · [docs index](README.md)

Timber is a device tree viewer. It loads a device tree from a compiled blob, from a BSP source folder, or live from a running device over SSH. The tree is drawn as an interactive web of nodes that you grow outward one node at a time.

---

## 1. Goals and non-goals

**Goals**

- View device trees from:
  - `.dtb` / `.dtbo` files
  - `.dts` files plus their includes
  - a whole BSP/kernel source folder
  - a live device over SSH (password auth included)
- Show the tree as an explorable web. Clicking a node opens a searchable popup of its children; picking one adds it to the web.
- Find any node by name, label, alias or `compatible` string, and add the path to it.
- Map every node and property back to the source file and line that defined it, and open that location.
- Work out which board `.dts` files are active from a kernel config.
- Diff the live tree from a device against the tree compiled from source.
- Run on Linux, Windows and macOS as a single desktop application.
- Ship a web build of the same UI for viewing files. Desktop-only features are visible but disabled, labelled *"Requires the desktop app"*.

**Non-goals (for now)**

- Editing device trees or writing changes back to source.
- Parsing the full Kconfig language. We read `.config` values and dts Makefiles, which is enough to work out the active boards.
- An integrated SSH terminal.

---

## 2. Architecture

```
┌──────────────────────── Tauri v2 desktop app ────────────────────────┐
│                                                                      │
│  Webview (system: WebView2 / WebKitGTK / WKWebView)                  │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │ timber-ui  (Leptos 0.8 CSR → wasm, built with trunk)           │  │
│  │   graph canvas · child popup · search · panels · source viewer │  │
│  │   Backend trait ── TauriBackend (invoke) │ WebBackend (browser)│  │
│  └───────────────────────────────┬────────────────────────────────┘  │
│                       Tauri IPC  │ commands + events                 │
│  ┌───────────────────────────────▼────────────────────────────────┐  │
│  │ timber-desktop (native Rust, Tauri backend)                    │  │
│  │   filesystem + folder watching · SSH (russh) · launch editor   │  │
│  └───────────────────────────────┬────────────────────────────────┘  │
└──────────────────────────────────┼───────────────────────────────────┘
                                   │ both sides link
                         ┌─────────▼──────────┐
                         │ timber-core        │  pure Rust, no I/O
                         │ model · dtb · dts  │  (wasm + native)
                         │ preprocessor · bsp │
                         │ format · diff      │
                         └────────────────────┘
```

### Crates

| Crate | Kind | Purpose |
|---|---|---|
| `crates/timber-core` | lib (native + wasm) | Tree model, DTB read/write, DTS compiler + preprocessor, source map, BSP/config analysis, value formatting, diff. No I/O: all file access goes through a `Vfs` trait. |
| `crates/timber-ui` | bin (wasm, trunk) | Leptos app. Has no idea whether it runs in Tauri or a browser; it only talks to a `Backend`. |
| `crates/timber-desktop` | bin (Tauri v2) | Native backend: commands for filesystem, SSH, editor launch. Bundles the built UI. |

### Why Tauri + Leptos

- **Native SSH.** A browser can't open TCP connections, so a browser-only app would need a relay. Doing SSH natively removes the relay and gives us ssh-agent, `~/.ssh/config` and `known_hosts`.
- **Full filesystem access.** We get folder watching (recompile when a `.dtsi` is saved) and can launch your editor at `file:line`.
- **The UI stays Rust/wasm and Leptos**, which you prefer. The same UI also serves as the web build.

**Accepted cost:** on Linux the app depends on the system WebKitGTK. The AppImage bundles it; `.deb`/`.rpm` users need `libwebkit2gtk-4.1`.

---

## 3. Runtime modes and the `Backend` interface

The UI checks at startup whether it is inside Tauri (`window.__TAURI_INTERNALS__`). One UI build serves both modes; there are no compile-time feature flags.

```rust
// timber-ui: abstract over where bytes and capabilities come from
trait Backend {
    fn capabilities(&self) -> Capabilities;            // drives greyed-out UI
    async fn open_files(&self) -> Result<Vec<SourceFile>>;
    async fn open_folder(&self) -> Result<FolderHandle>;
    async fn read(&self, folder: &FolderHandle, path: &str) -> Result<Vec<u8>>;
    async fn ssh_connect(&self, req: SshRequest) -> Result<SshSession>;
    async fn ssh_fetch_tree(&self, s: &SshSession, src: LiveSource) -> Result<Vec<u8>>;
    async fn ssh_fetch_config(&self, s: &SshSession) -> Result<Option<Vec<u8>>>;
    async fn open_in_editor(&self, path: &str, line: u32) -> Result<()>;
    fn watch(&self, folder: &FolderHandle, on_change: Callback<Vec<String>>);
}
```

### Capability matrix

| Feature | Desktop | Web |
|---|---|---|
| Open `.dtb` / `.dts` files | ✅ | ✅ |
| Open BSP folder | ✅ native dialog | ⚠️ Chromium only (`showDirectoryPicker`, lazy reads). Firefox/Safari: greyed out, *Requires Chrome/Edge or the desktop app* |
| Recompile when the folder changes | ✅ file watcher | ⚠️ Chromium: manual "Reload" button |
| Apply overlays | ✅ | ✅ |
| Board detection from config | ✅ | ✅ (config file upload) |
| Source viewer | ✅ | ✅ |
| Open in external editor | ✅ | ❌ greyed out: *Requires the desktop app* |
| SSH: live tree, `/proc/config.gz` | ✅ | ❌ greyed out: *Requires the desktop app* |
| Live vs source diff | ✅ | ⚠️ only against a live tree loaded from a `.dtb` file (e.g. one you exported earlier) |

Disabled controls stay visible, with a tooltip and a small "Desktop" badge. Clicking one explains why it's unavailable and links to the desktop download.

### Where compilation happens

Compilation runs **in the UI's wasm** in both modes, so there's a single pipeline. `timber-core`'s preprocessor reads files through a `Vfs`.

- **Web:** the VFS holds an in-memory map of the opened folder.
- **Desktop:** the VFS asks the backend for file bytes.
  - To avoid an IPC round trip per `#include`, the backend first sends the folder index (paths and sizes).
  - It then sends the contents of only the files under the dts directories and `include/`, in one batched command.
  - If that turns out slow on large kernels, compilation can move into the backend without UI changes, since `timber-core` is shared.

---

## 4. Data model (`timber-core`)

```rust
type NodeId = usize;                 // arena index; 0 = root

struct Tree {
    nodes: Vec<Node>,
    mem_reserve: Vec<(u64, u64)>,
    boot_cpuid: u32,
    // derived: phandle → NodeId index, labels from __symbols__
}

struct Node {
    name: String,                    // "i2c@fe5a0000"; "" for root
    labels: Vec<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    props: Vec<Property>,
}

struct Property { name: String, value: Vec<u8> }

// Only for trees compiled from source:
struct SourceMap {
    files: Vec<String>,              // VFS paths
    nodes: Vec<NodeSources>,         // indexed by NodeId
    deleted: Vec<(String, SourceLoc)>, // "/path" or "/path:prop" removed by /delete-*/
}
struct NodeSources {
    node: Vec<SourceLoc>,                    // every block that touched the node
    props: Vec<(String, Vec<SourceLoc>)>,    // definition + overrides, in order
}
struct SourceLoc { file: u32, line: u32, col: u32, kind: LocKind } // Defined | Overridden | Deleted (+ Overlay in M4)

// Compiler output:
struct Compiled {
    tree: Tree,
    source_map: SourceMap,
    diagnostics: Vec<Diagnostic>,  // { severity, file, line, col, message, snippet }
    plugin: bool,                  // source was /plugin/
    inputs: Vec<String>,           // every file read: what the folder watcher must watch
}
```

- `serde` derives are added when a type first has to cross Tauri IPC (M3). Compilation runs in the UI's wasm (§3), so the tree may never need to.
- A loaded tree is a `Document`: `{ tree, source_map: Option<SourceMap>, origin: Origin }`.
  - `Origin` is one of: file, folder+board, or SSH host + live/fdt.
  - Up to two documents can be loaded at once, a *primary* and a *comparison*, for diffing.

---

## 5. Input formats

### 5.1 DTB / DTBO

- Full FDT reader (versions 16–17; older versions best-effort) and writer, for exporting.
- Labels are recovered from `__symbols__` when present (DTBs built with `-@`).
- An overlay (`/fragment@N/__overlay__`) opened alone is shown as-is, with a badge marking it as an overlay. Applying it onto a base tree is covered in §5.4.

### 5.2 DTS compiler

**Fidelity target ([D-013](DECISIONS.md)):** the compiled tree is structurally identical to `cpp | dtc` output. That means the same nodes, properties, order, values and phandle numbers. This is verified against every board and overlay in the kernel (§11).

**Preprocessor (cpp subset)**
- Directives: `#include "…"` / `<…>`, `/include/`, `#define` (object-like and function-like, `#`, `##`, `__VA_ARGS__`), `#undef`, `#if` / `#ifdef` / `#ifndef` / `#elif` / `#else` / `#endif`, `defined()`, `#error`.
- Comment stripping and line continuations, keeping line numbers intact.
- Emits cpp-style line markers, so every token maps back to its original `file:line`.

**Include resolution**, modelled on the kernel's `-I` paths:
1. the directory of the including file
2. `<root>/include`
3. `<root>/arch/*/boot/dts` and `<root>/scripts/dtc/include-prefixes`
4. as a fallback, a unique basename match anywhere in the folder

A missing include is a clear error that names the file and the line that included it.

**DTS grammar**
- `/dts-v1/`, `/plugin/`, `/memreserve/`
- labels; cell lists with `/bits/ 8|16|32|64`
- strings, byte strings, `&label` / `&{/path}` references (phandle in cells, path otherwise)
- C integer expressions in parentheses
- `&label { }` overrides
- `/delete-node/`, `/delete-property/`, `/omit-if-no-ref/`

**Merging and resolution**
- Same-named nodes merge, properties are overridden, and every merge records a `SourceLoc`.
- Phandles are assigned to referenced nodes that don't have one.
- In `/plugin/` mode, `&label { }` becomes `fragment@N`. Unresolved labels go into `__fixups__`.

**dtc behaviours we replicate** (each found by the kernel golden test):
- **Deleted nodes and properties keep their place.** A later re-add revives them in their original position instead of appending.
- **New phandles take the lowest unused number**, assigned in tree order (depth-first, properties in order).
- **References resolve before `/omit-if-no-ref/` removal.** References from nodes that end up removed still count, and still allocate phandles.
- **Redundant `name` properties are dropped**: a `name` equal to the node's base name.
- **`&{label/sub/path}`** is a path whose first component is a label.
- **In `/plugin/` mode:**
  - `&{/path} { }` always becomes a fragment with `target-path`
  - `&label { }` merges into the overlay's own node when an earlier fragment defined that label; otherwise it becomes a fragment
- **Error recovery:** a fully parsed statement whose target is missing is reported without skipping the next statement.

**Folder loading** follows **file** symlinks (e.g. `include/dt-bindings/input/linux-event-codes.h` → `uapi`) but not **directory** symlinks (`scripts/dtc/include-prefixes/*`), because the include resolver emulates those.

**Errors**
- Errors carry `file:line:col` plus the offending line.
- A compile with errors still reports them all where possible; we don't stop at the first.

### 5.3 BSP folders and the "active" layout

1. **Index the folder:** find all `*.dts`, `*.dtsi`, `*.h` under `arch/*/boot/dts/**` and `include/dt-bindings/**`. For non-kernel layouts, find every `*.dts*` in the tree.
2. **Load a config** from any of:
   - a `.config` file, or a `defconfig`. Caveat: a defconfig leaves out symbols set to their defaults. The UI warns that results may be incomplete; `ARCH_*` and `CONFIG_<board>` symbols are usually set explicitly anyway.
   - `/proc/config.gz` from the device (desktop, over SSH).
   - a Yocto `KERNEL_DEVICETREE = "…"` line, pasted or read from a `local.conf`/machine `.conf`.
3. **Evaluate the dts Makefiles:**
   - the `dtb-$(CONFIG_X) += a.dtb b.dtb` forms, plus `dtb-y`, `subdir-y` / `subdir-$(CONFIG_X)`, and `+=` / `:=` variable references
   - `ifeq` / `ifdef` blocks are evaluated where they only involve `CONFIG_*`; anything else counts as enabled, with a warning
   - `*-dtbs := base.dtb overlay.dtbo` composites are listed as boards and built by applying the overlays to the base (§5.4)
4. **Board picker:** active boards are listed first, then all other `.dts` files under "Inactive / unknown".
5. **Auto-match:** if an SSH session is open, the board whose root `compatible` (then `model`) matches the live device is pre-selected.

### 5.4 Applying overlays ([D-009](DECISIONS.md))

Overlays are applied onto a base tree **on request**, using the "Apply overlays…" action or a Makefile composite.

- **Inputs:** `.dtbo` files, or `.dtso` / `/plugin/` sources compiled on the fly.
- **Stacking:** several overlays apply in order.
- **Resolving each fragment's target**, in this order:
  1. `target-path`
  2. `target = <phandle>`, a phandle already present in the base
  3. a label, through `__fixups__` and the base tree's `__symbols__`
- **Merging:** the `__overlay__` contents are merged with the same semantics as `&label { }` in the compiler.
- **Phandles:**
  - The overlay's internal phandles are renumbered above the base tree's maximum, with `__local_fixups__` updating the cells that refer to them.
  - `__fixups__` entries are patched with the target's phandle, which is assigned if needed.
- **Source map:** every node and property an overlay touches is recorded with an `Overlay(name)` location, so the node panel shows e.g. "set by `rk3568-evb-camera.dtbo`".
- **Errors:**
  - A prebuilt base `.dtb` without `__symbols__` can't resolve label-targeted fragments. That's reported per fragment ("rebuild the base with `dtc -@`"); other fragments still apply.
  - A missing target is an error naming the fragment and the label.

---

## 6. SSH (desktop only)

**Library:** `russh` (client) in the Tauri backend, on Tokio.

**Connect dialog**
- Fields: host (accepts `user@host:port` or a `~/.ssh/config` alias), user, port, auth method.
- Remembered per host: user, port and the last auth method. **Passwords are never stored.**

**Auth methods, tried in this order** unless you pick one:
1. ssh-agent (Unix socket / Windows OpenSSH agent pipe / Pageant)
2. key files from `~/.ssh/config` `IdentityFile` or default names, with a passphrase prompt if needed
3. `none`, for root with an empty password, common on dev boards
4. password
5. keyboard-interactive, with prompts relayed to the UI so PAM/dropbear password prompts work

**Host keys (trust on first use)**
- Known keys are read from `~/.ssh/known_hosts` (read-only, never modified) and from Timber's own `known_hosts` in the app data dir ([D-007](DECISIONS.md)).
- **First connection:** show the fingerprint. If you accept, it's written to Timber's file.
- **Key changed:** a hard stop with a big warning. There's no one-click bypass; you remove the old entry in Settings.

**Fetching the tree** (`LiveSource`)
- **Auto (default):**
  1. Try `tar -C /proc/device-tree -cf - .`, falling back to `/sys/firmware/devicetree/base`. This is the live tree, including runtime overlays, and needs no root.
  2. If there's no `tar`, use a POSIX `sh` + `od` walker that streams a simple length-prefixed format.
  3. Otherwise read `/sys/firmware/fdt`, which needs root and is the boot-time tree.
- **The fetch result says which source was used**, shown in the UI as "Live (procfs)" vs "Boot FDT".
- The tar result becomes a `Tree` in core. The kernel's synthetic `name` properties are dropped when they just repeat the node's base name.
- **Config:** `zcat /proc/config.gz` is fetched when present, for board detection.
- **Timeouts:** connect 10 s, command 30 s. Errors are shown in plain language, e.g. "Host key changed", "Permission denied (password)", "tar not found, fell back to /sys/firmware/fdt".

---

## 7. Live vs source diff

- Compare two documents: typically **live** (SSH) against **source** (folder + board), or any two DTBs.
- **Matching:**
  - Nodes match by full path.
  - Properties match by name.
  - Unit-address case is normalised; on a unique match `foo` equals `foo@0`.
- **Phandle normalisation:** phandle numbers differ between two compilations. Before comparing, phandle cells in known reference properties are rewritten to the target node's path. That covers `clocks`, `interrupt-parent`, `*-gpios`, `pinctrl-N`, `*-supply`, and the rest of the format table in §9. Without this, nearly every reference would show as a false difference.
- **Ignored by default** (toggle in the diff panel): `phandle` / `linux,phandle`, `__symbols__` / `__fixups__` / `__local_fixups__`, `/chosen` bootargs-like props, and the kernel's synthetic `name`.
- **Result:** each node is one of *same*, *changed* (props differ), *only-in-A*, *only-in-B*; each property is *same*, *changed*, *added* or *removed*.
- **UI:**
  - A "Diff" toggle colours graph nodes by status. A legend shows counts.
  - The node popup and search can filter to "changed only".
  - The side panel shows A/B values side by side with byte-level highlights.
  - Summary list: "12 nodes changed, 3 only on device, 41 disabled in source but okay live…" Each line is clickable and reveals the node in the web.

---

## 8. User interface

### 8.1 Layout

```
┌─[🔍 Search…]──[Open ▾]──[Folder]──[Board ▾]──[SSH ⇄ Desktop]──[Diff]──────[⚙]┐
│                                                     ┌── Node panel ───────────┐│
│                                                     │ /soc/i2c@fe5a0000       ││
│                (graph canvas — the "web")           │ labels: i2c3  ·  okay   ││
│                                                     │ compatible: rk3568-i2c  ││
│         ○──○                                        │ ─ Properties ─          ││
│        /    \                                       │ reg = <0x0 0xfe5a…>     ││
│   ●root──○soc──◉i2c3 ┌─[ pm│        ]─┐            │ clocks = <&cru 12>, …   ││
│               \       │ ▸ pmic@20  (14)│            │ ─ Sources ─             ││
│                ○      │   pmu_io…     │            │ rk3568.dtsi:412 (def)   ││
│                       └───────────────┘            │ board.dts:88  (override)││
│ legend · node counts · source badge                 │ ─ Referenced by ─ …     ││
└─────────────────────────────────────────────────────┴─────────────────────────┘
```

- The **source viewer** opens as a resizable bottom drawer. It has syntax highlighting, jumps to the line, and highlights the node's block. On desktop there's also an "Open in editor" button.
- **Open in editor** ([D-008](DECISIONS.md)):
  1. `$VISUAL`, then `$EDITOR`. A terminal editor (vim, nano, …) is launched in the platform terminal.
  2. Otherwise VS Code (`code -g file:line:col`).
  3. Otherwise a custom command template in Settings, e.g. `subl {file}:{line}` or `idea --line {line} {file}`.
- **Themes:** light and dark, following the OS, with an override in settings.

### 8.2 The web (graph)

- **Starts with the root only.** "Show enabled top-level nodes" is a toolbar shortcut.
- **Layout:** force-directed. Nodes repel each other, edges act as springs, and a light pull keeps everything near the centre. It cools down when idle and reheats when nodes are added or removed.
- **Mouse:**
  - drag a node to move it
  - drag the background to pan
  - scroll to zoom around the cursor
  - double-click to collapse a subtree (or expand all children if it's a leaf in the web)
- **Keys:** `F` fits the view, `Del` removes the selected subtree from the web, `Esc` closes popups.
- **Node styling:**
  - coloured by kind: device, cpu, bus, clock, interrupt-controller, power/regulator, gpio/pinctrl, memory, plus neutral grey for data-only and root ([D-016](DECISIONS.md))
  - a node is classified in this order:
    1. pin groups (under a pinctrl node, no `compatible`)
    2. cpu
    3. memory
    4. GPIO, which wins over interrupt controller for GPIO banks
    5. interrupt controller
    6. clock
    7. power
    8. bus
    9. any other node with `compatible` is a device; anything left is data
  - disabled nodes are dimmed and dashed
  - a "+N" badge shows hidden children
  - the selected node gets an accent ring
- **Reference edges:** the selected node's phandle references are drawn as dashed edges to any visible targets.
- **Rendering:** a `<canvas>` inside a Leptos component, run by a `requestAnimationFrame` loop that holds non-reactive simulation state. Leptos signals carry only *what* is visible and selected, never per-frame positions.
- **Label culling:** labels are placed in priority order and skipped where they'd overlap one already placed. Zooming in reveals them. The priority order is:
  1. selected, hovered and popup nodes, which are always shown
  2. the root
  3. expanded nodes
  4. nodes with more children
- **Phones:** below 760 px the panel moves under the graph and the legend starts collapsed.

### 8.3 Child popup (clicking a node)

- Opens beside the node with a **focused filter input**. It follows the node as the web moves.
- **Rows:** child name, kind dot, `(N)` descendant count, a status dot, and a ✓ if it's already in the web.
- **Filtering:** typing filters by fuzzy match on name; labels and compatible match too, with a chip showing which.
- **Keys:**
  - `↑` / `↓` move the selection
  - `Enter` adds the child and **moves the popup onto it** (drill-down: `soc⏎ i2c3⏎ pmic⏎`)
  - `Ctrl+Enter` adds it and stays, so you can add siblings
  - `Ctrl+A` adds all filtered children
  - `Backspace` in an empty filter goes back up to the parent
  - `Esc` closes
- **Clicking a row toggles it:** it adds the child, or, if the child is already in the web (✓), removes it with its subtree. Double-clicking behaves like `Enter` ([D-015](DECISIONS.md)).

### 8.4 Global search (top-left)

- Searches recursively across **node names, labels, aliases (`/aliases`) and `compatible` strings**.
- **Results:**
  - each has a **chip naming the match type**: `name`, `label`, `alias`, `compatible`
  - shows the full path with the matched part highlighted
  - ranked exact > prefix > substring > fuzzy, then by depth
- **Shortcut:** `Ctrl+K` / `/` focuses search.
- **Selecting a result:** adds every node on the path from the root, selects the target, animates the camera to it, and opens its child popup.

### 8.5 Node panel

- Breadcrumb path (each segment clickable), labels, status, compatible.
- **Properties:** decoded (see §9). References are links: clicking one reveals and selects the target.
- **Sources** (compiled trees only): definition and override locations. Clicking opens the source viewer.
- **Referenced by:** reverse phandle index, clickable.
- **Actions:** expand children, collapse, focus, copy path, copy as DTS snippet.

### 8.6 Export

Export the current tree as `.dts` (decompiled) or `.dtb`.
- Desktop: native save dialog.
- Web: browser download.

---

## 9. Property value formatting

The aim is to print values close to `dtc -O dts` output, with extra detail where it helps.

| Case | Display |
|---|---|
| empty | `true` (boolean property) |
| known phandle-list props (`interrupt-parent`, `*-supply`, `*-handle`, `pinctrl-N`, `memory-region`, `remote-endpoint`, `operating-points-v2`, `nvmem-cells`, …) | `<&label>` links |
| phandle + specifier props (`clocks`, `resets`, `*-gpios`, `dmas`, `phys`, `power-domains`, `iommus`, `interrupts-extended`, `mboxes`, `pwms`, `io-channels`, `interconnects`, `thermal-sensors`, `sound-dai`, `cooling-device`, `assigned-clock*`) | `<&cru 12>, <&cru 40>`, using the target's `#*-cells` to split entries |
| `reg` | one `<…>` per entry, split by the parent's `#address-cells` / `#size-cells`. Tooltip shows the decoded address and size. |
| `ranges`, `dma-ranges` | split by child address + parent address + child size cells |
| string list | `"a", "b"` |
| `#*-cells` | decimal |
| other multiples of 4 bytes | `<0x… 0x…>` |
| otherwise | `[01 02 ab]` |

If a heuristic doesn't fit the bytes, for example a phandle that doesn't exist, the value falls back to the generic format. We never show a wrong decoding.

---

## 10. Build, packaging and repository layout

```
timber/
  Cargo.toml                 # workspace
  crates/timber-core/
  crates/timber-ui/          # index.html, Trunk.toml, src/, style
  crates/timber-desktop/     # Tauri v2: tauri.conf.json, capabilities/, src/
  docs/DESIGN.md
  justfile                   # dev / build / web / test shortcuts
```

**Dev**
- `cargo tauri dev` runs `trunk serve` for the UI and hot-reloads it inside the Tauri window.
- `trunk serve` alone gives the web build.

**Release**
- `cargo tauri build` produces:
  - `.msi`/NSIS `.exe` on Windows
  - `.AppImage` and `.deb` on Linux
  - `.dmg` and `.app` on macOS
- CI: a GitHub Actions matrix with one job per OS.

**Web**
- `trunk build --release` produces a static `dist/` that can be deployed to any static host.

**Versions:** pin current stable Tauri 2.x, Leptos 0.8.x and russh 0.6x at the start of implementation.

---

## 11. Testing

**`timber-core` unit tests**
- DTB round-trip
- expression evaluator, macro expansion, include resolution, merge semantics, `/delete-*`, phandle assignment
- formatting heuristics
- Makefile/config evaluation
- diff with phandle normalisation

**Golden tests against `dtc`**
- Gated on `dtc` being on PATH, which it is in CI.
- Compile fixture `.dts` files with both `dtc` and Timber, then compare the trees semantically: same nodes, properties and values, with phandles normalised.
- **Fixtures:** hand-written sources in `crates/timber-core/tests/fixtures` (MIT), covering includes, macros, overrides, deletions, `/omit-if-no-ref/`, `/bits/`, expressions and an overlay.
- **Kernel test:** compiles every board `.dts` and overlay `.dtso` in a kernel checkout and compares each against `dtc`. Set these environment variables:
  - `TIMBER_KERNEL_DIR` (required)
  - `TIMBER_KERNEL_ARCH` (default `arm64`)
  - `TIMBER_KERNEL_FILTER` (optional)

  No kernel sources are vendored ([D-014](DECISIONS.md)).
- **Result, 2026-10-21** (torvalds/linux `a804c57`): 1,900 arm64, 1,767 arm and 66 riscv sources compared, **0 differences**.
- **A sparse checkout is enough:** `arch/*/boot/dts`, `include/dt-bindings`, `include/uapi` and `scripts/dtc`.
- **Speed:** an 813-node RK3588 board (23 input files) compiles in about 21 ms in a native release build. Measure with `cargo run --release -p timber-core --example compile -- <kernel> <board.dts>`.

**Other tests**
- **Live fetch:** tar → `Tree` conversion tested with captured tarballs.
- **Desktop SSH:** an integration test against a throwaway `sshd` container in CI (Linux only), covering password, key and TOFU.
- **UI:** manual checklist per milestone. Playwright smoke tests for the web build later.

---

## 12. Milestones

See [ROADMAP.md](ROADMAP.md) for milestones and progress.

---

## 13. Resolved questions

All four open questions from the first draft are resolved. See [DECISIONS.md](DECISIONS.md):
- **Web folders:** Chromium only for now (D-006).
- **known_hosts:** writes go to Timber's own file (D-007).
- **Editor launch:** `$VISUAL`/`$EDITOR`, then VS Code, then a command in Settings (D-008).
- **Overlay application:** in scope, as part of milestone 4 (D-009).
