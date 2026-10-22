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

### Desktop app (`timber-desktop`, [D-025](DECISIONS.md))

**What it is:**
- A Tauri 2 app, built as a single `timber` binary (~8.6 MB release).
- The UI picks `TauriBackend` when `window.__TAURI__` exists (`withGlobalTauri`).

**Commands:**
| Command | Does |
|---|---|
| `pick_files`, `read_paths` | Native file dialog, or files given by path. |
| `pick_folder`, `read_folder` | Folders read straight from disk, pruned like the web walk. The rules live in `timber_core::folder`. Directory symlinks are followed with cycle protection; file symlinks are read through. |
| `save_file` | Raw request body (no JSON), suggested name in an `x-file-name` header, native save dialog. Reports whether you cancelled. |
| `watch_folder`, `unwatch_folder` | A recursive `notify` watcher per folder. A 400 ms quiet period after the last change to a source file emits `folder-changed`, and the UI reloads that folder. |
| `ssh_connect`, `ssh_fetch`, `ssh_device_config`, `ssh_disconnect`, `ssh_trust_host` | SSH sessions (§6). `ssh_fetch` answers in binary: `"TMBL"`, how it was read, whether `/proc/config.gz` is readable, the path read, then the DTB. Failures come back as `{kind, message, host_key, key_file}` so the dialog can ask for what's missing. |
| `startup`, `report`, `quit` | Command-line handling, below. |

**File transfer:** files go to the UI as **one binary message**:
- `"TMBR"`, version, folder handle, then length-prefixed name/data pairs
- JSON number arrays would be several times larger for a kernel tree

**Command line:** `timber [FILE…] [--source DIR…] [--config FILE] [--ssh TARGET [--ssh-options OPTS] [--tree-path PATH]] [--smoke | --smoke-watch]`
- Files and folders are loaded at startup.
- `--ssh` connects at launch. If that needs you (a password, a new host key), the connect dialog opens with the problem shown.
- `--smoke` prints a one-line summary of what loaded (sources, boards, live device, node count, matched board, differences) and exits.
- `--smoke-watch` also waits for a watched folder to change and reload, reports again, then exits.
- These are the app's end-to-end tests.

**Development:**
- `cargo tauri dev` (from `crates/timber-desktop`) runs `trunk serve` from the repo root and loads `http://127.0.0.1:8080`.
- `cargo tauri build` bundles a release with `trunk build --release` output.



| Feature | Desktop | Web |
|---|---|---|
| Open `.dtb` / `.dts` files | ✅ | ✅ |
| Add source folders | ✅ native dialog | ✅ Chrome/Edge: read in place with Reload (directory access API); Firefox/Safari: folder picker. Only source files are read ([D-017](DECISIONS.md), [D-022](DECISIONS.md)) |
| Recompile when the folder changes | ✅ file watcher | ⚠️ Chrome/Edge: Reload button; others: re-add the folder |
| Export PNG / SVG | ✅ | ✅ |
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
- **Labels merged into an existing node are prepended**, so the newest label comes first (dtc's `add_label`). This decides `__symbols__` order.
- **`-@` mode** (`dts::Options { symbols: true }`):
  - emits `__symbols__`, with every label → path, in tree order
  - **keeps labelled `/omit-if-no-ref/` nodes**, since an overlay might reference them
  - gives **every labelled node a phandle**: the lowest free one in the final tree, so phandles of dropped omit-nodes are reused. A node whose only labels were removed by `/delete-node/` still counts as labelled.

  Annotation compiles in this mode automatically when the DTB carries `__symbols__`; otherwise ~430 Rockchip pin groups looked "not in source" (D-024).

**Folder loading** follows **file** symlinks (e.g. `include/dt-bindings/input/linux-event-codes.h` → `uapi`) but not **directory** symlinks (`scripts/dtc/include-prefixes/*`), because the include resolver emulates those.

**Errors**
- Errors carry `file:line:col` plus the offending line.
- A compile with errors still reports them all where possible; we don't stop at the first.

### 5.3 BSP folders and the "active" layout

1. **Folders:** kernel trees are read pruned to `arch/*/boot/dts/**` (dts sources *and* Makefiles), `include/dt-bindings/**` and `include/uapi/**`. The rules are in `timber_core::folder`, shared by the web and desktop walks.
2. **Config** (Sources panel → Kernel config; a file dropped on the window; or `--config` on the desktop command line):
   - a **`.config`**: Kconfig output, recognised by its "Automatically generated file" header
   - a **defconfig**: symbols left at their defaults aren't listed, so the panel warns that some built boards may show as not built
   - a **Yocto file** setting `KERNEL_DEVICETREE` (`local.conf`, a machine `.conf`, or `bitbake -e` output). It handles `=`, `?=`, `??=`, `+=`, `:append`/`:prepend` and line continuations, and names the built targets directly.
3. **Makefile evaluation** (`timber_core::kbuild`), covering the make subset dts Makefiles use:
   - `dtb-$(CONFIG_X)` and `${…}` lists, and `dtb-y`
   - `subdir-y` / `subdir-$(CONFIG_X)`; a parent's gate applies to the whole subdirectory
   - composites: `name-dtbs := base.dtb x.dtbo …`
   - `DTC_FLAGS` and `DTC_FLAGS_<name>` containing `-@`, so those boards compile in `-@` mode when shown
   - simple variables, and `ifeq`/`ifneq`/`ifdef`/`ifndef`/`else`/`endif`; without a config every branch counts as possible
   - unsupported functions and `include` are skipped, with a warning

   **The config's architecture:**
   - A config only builds its own architecture's device trees. arm and arm64 share most `CONFIG_ARCH_*` symbols.
   - The architecture comes from the arch symbol (`CONFIG_ARM64`, …) or arch-only symbols.
   - Failing that, it's **inferred**: the architecture whose Makefiles use the most of the config's enabled symbols. The current arm64 defconfig names no arch symbol at all.
   - The panel marks an inferred architecture as such. A tie means no filtering.
4. **Board picker:**
   - With a config: "built" boards come first, and with an empty query the picker lists them.
   - Each row has a chip: **built** / **not built**, or, without a config, the `CONFIG_` symbols that gate it.
5. **DTB matching:** among equally good candidates, boards the config builds win.

**Measured on the kernel:**
- 4,042 Makefile targets: 478 composites, 271 built with `-@`
- 3,389 of 3,392 board `.dts` files are referenced by some Makefile
- `CONFIG_ARCH_ROCKCHIP=y` alone builds the 308 Rockchip targets
- the arm64 defconfig is inferred as arm64 and builds 1,913

### 5.4 Applying overlays ([D-009](DECISIONS.md), [D-027](DECISIONS.md))

`timber_core::overlay::apply` follows libfdt's `fdt_overlay_apply`, which is what U-Boot and `fdtoverlay` run:
1. **Phandles:** the overlay's own phandles are shifted above the base's highest, and the cells listed in `__local_fixups__` follow.
2. **`__fixups__`:** each label is resolved through the base's `__symbols__`. A base without them gets a clear "build the base with dtc -@" error.
3. **Phandle conflicts:** settled for **all** fragments before any is merged. An overlay node landing on a base node that already has a phandle drops its own phandle, and references are pointed at the existing one.
4. **Merging:** each fragment's `__overlay__` goes into its `target` (phandle) or `target-path`. libfdt's quirks are matched:
   - new properties and subnodes are inserted **before** existing ones
   - a name without a unit address matches an existing node *with* one (`spi` merges into `spi@fe300000`)
5. **Symbols:** the overlay's `__symbols__` are added to the base's, rewritten to their new paths.

`.dtso` sources are compiled in `-@` mode, as the kernel builds overlays. Applying overlays to a base needs the base's `__symbols__`, so a board from sources is compiled with `-@` whenever overlays are attached.

**In the UI ([D-027](DECISIONS.md)):**
- **Two lists in the Sources panel:**
  - *Overlays*, under Tree shown, applied to what's displayed
  - *Overlays applied at boot*, under a DTB's source board, applied to the source board before comparing
- **Adding overlays:** each list takes `.dtbo` files, `.dtso` files (added to the sources), or overlays already in the sources. Entries can be reordered or removed.
- **Composite boards** in the board picker (only when the base and every overlay are in the sources) open as the base with its overlays listed. Picking one as a DTB's source board fills the boot list.
- **Node panel:** properties an overlay set show "set by camera.dtso:12" (or "(at boot)"); nodes it created show "added by …".
- **Failures:** overlays that fail to compile or apply are reported in the compiler messages, and the rest still apply.

**Verified:** all **473** kernel composite boards (`name-dtbs := base.dtb x.dtbo…`) match `fdtoverlay` exactly. That's the `kernel_overlays` golden test; 5 are skipped because `dtc`/`fdtoverlay` themselves reject them.

---

## 6. SSH (desktop only)

**Library:** `russh` in the desktop app; the web build shows SSH greyed out.

**Connect dialog** (toolbar **SSH**, or Sources → Tree shown → *Connect to a device…*):
- **Fields:** host (accepts `user@host:port` or an `~/.ssh/config` alias), user, port, and auth (Automatic / Password / Key file), with password, key path and passphrase.
- **Extra options** ([D-029](DECISIONS.md)): OpenSSH-style, including `-J`/`ProxyJump` and `ProxyCommand`.
- **Remote tree path:** advanced; default `/proc/device-tree`.
- **Recent devices:** one-click reconnect, remembering host, user, port, auth method and options. Passwords are never stored.

**`~/.ssh/config`:** `Host` patterns (`*`, `?`, `!`), with the first value found winning, for `HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump` and `ProxyCommand`.

**Authentication** (Automatic tries each in order):
1. ssh-agent
2. key files: `IdentityFile`, else `~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa`. A key that needs a passphrase asks for one.
3. `none`
4. password, including an empty one (common for root on dev boards)
5. keyboard-interactive, answering password-style prompts with the password

**Host keys (trust on first use):**
- `~/.ssh/known_hosts` is read but never modified; Timber's own `known_hosts` is in the app data directory.
- **Unknown key:** the dialog shows the SHA-256 fingerprint. **Trust** saves it and reconnects.
- **Changed key** ([D-028](DECISIONS.md)): the connection stops. The warning shows the old and new fingerprints, with **"Device was reflashed: trust the new key"**.

**Fetching** runs one remote `sh` script, which prints a header line naming what it sends:
1. **`TGZ` / `TAR`:** `tar` of the tree path, `/proc/device-tree` by default, falling back to `/sys/firmware/devicetree/base`, piped through `gzip` when the device has it. This is the live tree, including runtime overlays. Every property is a tiny file padded to 512 bytes in the archive, so gzip shrinks a Rock 5B's tree from 4.8 MB to about 100 KB.
2. **`OD`:** no `tar`, so `find` + `od` hex dumps (busybox).
3. **`FDT`:** `cat /sys/firmware/fdt`, the boot-time blob (needs root).

The result is converted to a `Tree` (`timber_core::live`), then sent to the UI as a DTB. The kernel's synthetic `name` properties are dropped where they just repeat the node's base name. That directory lists entries in hash order, so nodes are sorted by unit address and properties into the usual DTS order (`compatible`, `reg`, … `status`, `phandle`). The tree is shown as **"host · live"** or **"host · boot FDT"**, and everything else (source matching, differences, boot overlays, export) applies.

**Device config:** if `/proc/config.gz` exists, Timber always asks whether to use it as the kernel config ([D-030](DECISIONS.md)).

**Live session** ([D-031](DECISIONS.md)): the connection stays open while the tree is shown.
- The toolbar shows the device (`user@host`), **Refresh**, **Auto-refresh** (off / 5 s / 10 s / 30 s / 60 s) and **Disconnect**. Disconnecting leaves the tree shown as a snapshot.
- The view only changes when the tree did. When it changes shape, the web is kept: nodes still present stay where they were, by path, as do the selection and camera.
- If the connection dropped (the board rebooted), the next read reconnects with the same settings, kept in memory only. While the device is unreachable, the live bar's dot turns red and only the first failure pops up.
- Opening a file or showing a board disconnects.
- **Recent devices** are kept in the app's local storage (no passwords or passphrases).

**Errors:** in plain language, e.g. "Host key changed", "Permission denied (tried: agent, key ~/.ssh/id_ed25519, password)", "No device tree at /proc/device-tree and /sys/firmware/fdt isn't readable (needs root)". Timeouts: connect 10 s, commands 30 s.

**Tests:**
- A throwaway `sshd` on a high port with temporary keys and a fake tree directory, which is why the tree path is configurable. It covers key auth, unknown/changed host keys, `tar` vs `od`, `config.gz`, and refresh.
- Password and keyboard-interactive auth need a manual check against a real board: a non-root `sshd` can't check passwords.

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

## 7a. Sources ([D-018](DECISIONS.md))

There's one **Sources** panel for everything loaded. It replaces the earlier "Open…" button.

### What it holds
- **Tree shown**, which is what the web displays. One of:
  - a **DTB/DTBO file**
  - a **board from the source folders**, compiled on the fly
  - later, a **live device** (desktop only)
- **Source folders and files:** any number of folders (kernel trees, BSP layers) and loose files. Each entry shows what was found, e.g. "linux: kernel tree · arm64, arm · 3,520 files · 3,667 boards". Entries can be removed.

### Building the combined VFS
- Each entry's files are mounted under its own name: `linux/arch/...`, `bsp/...`.
- Only source files are read: `.dts`, `.dtsi`, `.dtso`, `.h`.
- In a kernel tree, only `arch/*/boot/dts/**`, `include/dt-bindings/**` and `include/uapi/**` are read.
- Includes resolve from the includer's own folder first, then from the other entries in list order. Putting a BSP above the kernel therefore lets it take priority.

### The board index
- Each board `.dts` is scanned cheaply, without a full compile, for the root `compatible` and `model` it declares itself.
- That's enough to list boards and to match a DTB to one.

### Annotating a DTB from sources ([D-021](DECISIONS.md))

Custom embedded boards often keep their EVK's `compatible` or live out of tree, so matching is done in two stages.

1. **Shortlist** candidates from the board index, ranked by:
   1. the **file name** (`acme-gw.dtb` ↔ `acme-gw.dts`, since build systems name the blob after the source)
   2. root `compatible` (identical list, then same first entry, then most shared)
   3. `model`
2. **Confirm by content.** Compile the top 6 candidates with the build defines and score each against the DTB:
   - **½ × node coverage:** the share of the DTB's nodes found at the same path
   - **½ × property agreement:** the share of compared properties that are equal, with references compared by target path

   The highest score wins. The panel shows the board, its confidence and the reasons, e.g. "file name + identical compatible · 97% match".
3. **Override:** a searchable board picker, or "use this .dts file" directly. The choice is remembered per DTB file name in browser storage, and "Auto" goes back to matching.
4. **Map nodes:** the chosen board is compiled and each DTB node is mapped to its source node by path. The node panel then shows:
   - "defined at / overridden at" source locations
   - **"differs from source"** with the source value
   - **"not in source"** for DTB-only nodes
   - **"only in source"** for properties missing from the DTB
   - **labels from source**, for DTBs built without `__symbols__`
5. **Bootloader hints:** differences in properties U-Boot commonly patches are tagged *"likely set by U-Boot"*. These are `/memory*/reg`, anything under `/chosen`, `/reserved-memory`, `mac-address`, `local-mac-address`, `serial-number` and `status`. U-Boot is assumed to be the only boot-time patcher for now.
6. **Diagnostics:** the compile's messages appear in the compiler messages drawer.

**Build defines:** a field in the Sources panel (`BOARD_REV=2, FEATURE_X`) is passed to the preprocessor as `-D` for every compile, so `#if`-dependent sources build the way the real build does.

**Yocto tip** (shown in the panel): add the kernel's `tmp/work-shared/<MACHINE>/kernel-source`, which has the layer's DTS patches applied. Or add the kernel plus your layer's `files/` directories.

**Where differences show** ([D-023](DECISIONS.md)): amber `Δn` / `new` pills on graph nodes and popup rows; amber hidden-children badges with the count of differences below; a node panel header chip; a toolbar "N differ from source" button and a Sources panel summary, both with **Show differences** (adds every differing node to the web and fits). All differences are treated alike; the U-Boot hint is informational.

This is the lightweight form of the diff. M6 adds comparing two arbitrary trees (e.g. two DTBs, or a live device against source), side-by-side values with byte-level highlights, and diff filters in search.

### Node panel when there's no source info
- **No sources added:** an **"Add source folder…"** button that opens the Sources flow.
- **Sources added but no board matched:** a "Choose board…" button.

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
- **Open in editor** ([D-026](DECISIONS.md)), desktop only:
  - runs the Settings "Editor command" template (`{file}`, `{line}`, `{col}`)
  - with no template, uses the OS default application
  - examples are in [EDITORS.md](EDITORS.md)
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

### 8.6 Export ([D-019](DECISIONS.md))

An **Export ▾** menu in the toolbar.

**Tree formats:**
- `.dts` (decompiled)
- `.dtb`

**Images:**
- **SVG**, a vector image that scales cleanly in documentation, with real text
- **PNG**, at 2× resolution for sharing

**Options** (defaults in bold):
- what to include: **the whole web** or the current view
- theme: **light**, dark, or the current one
- **legend** on/off
- **title** on/off. The title shows the file or board name, the `model`, and the number of nodes shown out of the total.

**Rendering:**
- One drawing routine serves the screen, PNG and SVG, through a small `Painter` interface with a canvas implementation and an SVG implementation. Exports therefore look exactly like the screen.
- Selection rings, hover and reference edges are left out of exports.

**Saving:**
- Desktop: native save dialog.
- Web: browser download.

### 8.7 Shortcuts help ([D-020](DECISIONS.md))

- **When it shows:** a shortcuts popup appears on first visit. "Don't show on startup" (stored in browser storage) turns that off, and `?` or the toolbar's `?` button reopens it any time.
- **Groups:**
  - **Anywhere:** `/` / `Ctrl+K` search, `F` fit, `Esc` close popup, `Del` remove from web, `?` help
  - **Child popup:** type to filter, `↑↓`, `⏎` add and open, `Ctrl+⏎` add, `Ctrl+A` add all shown, `⌫` up
  - **Mouse:** click opens children, double-click collapses or expands, drag moves, scroll zooms

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
- **Result, 2026-10-22** (torvalds/linux `a804c57`): 1,900 arm64, 1,767 arm and 66 riscv sources compared, **0 differences**, both plain and with `TIMBER_KERNEL_SYMBOLS=1` (against `dtc -@`).
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
- **Web folders:** first Chromium only (D-006), then all browsers via `webkitdirectory` (D-017).
- **known_hosts:** writes go to Timber's own file (D-007).
- **Editor launch:** `$VISUAL`/`$EDITOR`, then VS Code, then a command in Settings (D-008).
- **Overlay application:** in scope, as part of milestone 4 (D-009).
