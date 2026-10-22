//! Golden tests: compile sources with both `cpp | dtc` (as the kernel build does) and
//! timber, then compare the resulting trees.
//!
//! - `fixtures`: hand-written sources in `tests/fixtures`; skipped if `cpp`/`dtc` are missing.
//! - `kernel_tree`: every board `.dts` and overlay `.dtso` in a kernel checkout. Set `TIMBER_KERNEL_DIR` to the
//!   checkout, optionally `TIMBER_KERNEL_ARCH` (default `arm64`) and `TIMBER_KERNEL_FILTER`
//!   (substring of the board path).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use timber_core::vfs::MemVfs;
use timber_core::{Tree, dtb, dts};

fn have_tools() -> bool {
    let ok = |cmd: &str| {
        Command::new(cmd)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    };
    ok("cpp") && ok("dtc")
}

/// Runs the kernel's `cpp | dtc` pipeline. `Err` carries dtc's message.
fn dtc_compile(file: &Path, include_dirs: &[PathBuf], symbols: bool) -> Result<Tree, String> {
    let mut cpp = Command::new("cpp");
    cpp.args([
        "-nostdinc",
        "-undef",
        "-D__DTS__",
        "-x",
        "assembler-with-cpp",
    ]);
    for dir in include_dirs {
        cpp.arg("-I").arg(dir);
    }
    let pre = cpp.arg(file).output().map_err(|e| e.to_string())?;
    if !pre.status.success() {
        return Err(format!("cpp: {}", String::from_utf8_lossy(&pre.stderr)));
    }
    let mut dtc = Command::new("dtc");
    dtc.args(["-q", "-I", "dts", "-O", "dtb", "-o", "-"]);
    if symbols {
        dtc.arg("-@");
    }
    // `/include/` is resolved by dtc itself, against the source's directory and -i paths.
    if let Some(dir) = file.parent() {
        dtc.arg("-i").arg(dir);
    }
    for dir in include_dirs {
        dtc.arg("-i").arg(dir);
    }
    let mut child = dtc
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || std::io::Write::write_all(&mut stdin, &pre.stdout));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(format!("dtc: {}", String::from_utf8_lossy(&out.stderr)));
    }
    dtb::parse(&out.stdout).map_err(|e| e.to_string())
}

/// Loads every source-like file under `root` into a VFS, keyed relative to `root`.
fn load_dir(root: &Path, subdirs: &[&str]) -> MemVfs {
    fn walk(dir: &Path, root: &Path, vfs: &mut MemVfs) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Directory symlinks (scripts/dtc/include-prefixes) are emulated by the resolver;
            // file symlinks (dt-bindings headers pointing into uapi) must be followed.
            if kind.is_symlink() && path.is_dir() {
                continue;
            }
            if kind.is_dir() {
                walk(&path, root, vfs);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("dts" | "dtsi" | "dtso" | "h")
            ) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                vfs.insert(&rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut vfs = MemVfs::new();
    for sub in subdirs {
        walk(&root.join(sub), root, &mut vfs);
    }
    vfs
}

/// Structural differences between two trees (labels ignored: DTBs don't carry them).
fn diff_trees(a: &Tree, b: &Tree) -> Vec<String> {
    fn walk(a: &Tree, ai: usize, b: &Tree, bi: usize, out: &mut Vec<String>) {
        if out.len() > 20 {
            return;
        }
        let path = a.path(ai);
        let (na, nb) = (a.node(ai), b.node(bi));
        let pa: Vec<&str> = na.props.iter().map(|p| p.name.as_str()).collect();
        let pb: Vec<&str> = nb.props.iter().map(|p| p.name.as_str()).collect();
        if pa != pb {
            out.push(format!("{path}: props timber={pa:?} dtc={pb:?}"));
        }
        for p in &na.props {
            if let Some(q) = nb.prop(&p.name)
                && p.value != q
            {
                out.push(format!(
                    "{path}:{} timber={:02x?} dtc={:02x?}",
                    p.name, p.value, q
                ));
            }
        }
        let ca: Vec<&str> = na
            .children
            .iter()
            .map(|&c| a.node(c).name.as_str())
            .collect();
        let cb: Vec<&str> = nb
            .children
            .iter()
            .map(|&c| b.node(c).name.as_str())
            .collect();
        if ca != cb {
            out.push(format!("{path}: children timber={ca:?} dtc={cb:?}"));
        }
        for &c in &na.children {
            if let Some(d) = b.child_by_name(bi, &a.node(c).name)
                && b.node(d).name == a.node(c).name
            {
                walk(a, c, b, d, out);
            }
        }
    }
    let mut out = Vec::new();
    if a.mem_reserve != b.mem_reserve {
        out.push(format!(
            "memreserve timber={:x?} dtc={:x?}",
            a.mem_reserve, b.mem_reserve
        ));
    }
    walk(a, Tree::ROOT, b, Tree::ROOT, &mut out);
    out
}

#[test]
fn fixtures() {
    if !have_tools() {
        eprintln!("skipping: cpp and dtc are required");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let vfs = load_dir(&root, &["."]);
    let includes = [root.clone(), root.join("include")];
    for name in ["board.dts", "overlay.dtso"] {
        let expected = dtc_compile(&root.join(name), &includes, false)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let ours = dts::compile(&vfs, name);
        let errors: Vec<String> = ours.errors().map(|d| d.to_string()).collect();
        assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
        let diffs = diff_trees(&ours.tree, &expected);
        assert!(
            diffs.is_empty(),
            "{name} differs from dtc:\n{}",
            diffs.join("\n")
        );
    }
}

#[test]
fn kernel_tree() {
    let Ok(dir) = std::env::var("TIMBER_KERNEL_DIR") else {
        eprintln!("skipping: set TIMBER_KERNEL_DIR to a kernel checkout");
        return;
    };
    if !have_tools() {
        eprintln!("skipping: cpp and dtc are required");
        return;
    }
    let root = PathBuf::from(dir);
    let arch = std::env::var("TIMBER_KERNEL_ARCH").unwrap_or_else(|_| "arm64".into());
    let filter = std::env::var("TIMBER_KERNEL_FILTER").unwrap_or_default();
    // TIMBER_KERNEL_SYMBOLS=1 compares against `dtc -@` builds.
    let symbols = std::env::var("TIMBER_KERNEL_SYMBOLS").is_ok_and(|v| v == "1");
    let dts_dir = format!("arch/{arch}/boot/dts");
    // Boards may include other arches' sources via include-prefixes (`arm/...`).
    let vfs = load_dir(&root, &["arch", "include/dt-bindings"]);
    let includes = [
        root.join("scripts/dtc/include-prefixes"),
        root.join(&dts_dir),
        root.join("include"),
    ];

    let boards: Vec<String> = timber_core::vfs::Vfs::paths(&vfs)
        .filter(|p| {
            p.starts_with(&dts_dir)
                && (p.ends_with(".dts") || p.ends_with(".dtso"))
                && p.contains(&filter)
        })
        .map(str::to_string)
        .collect();
    let (mut passed, mut skipped, mut failures) = (0, 0, Vec::new());
    for board in &boards {
        let expected = match dtc_compile(&root.join(board), &includes, symbols) {
            Ok(t) => t,
            Err(e) => {
                if skipped < 3 {
                    eprintln!("dtc rejected {board}: {}", e.lines().next().unwrap_or(""));
                }
                skipped += 1; // dtc itself rejects it; nothing to compare against
                continue;
            }
        };
        let ours = dts::compile_with(
            &vfs,
            board,
            &dts::Options {
                symbols,
                ..dts::Options::default()
            },
        );
        let mut problems: Vec<String> = ours.errors().map(|d| d.to_string()).collect();
        if problems.is_empty() {
            problems = diff_trees(&ours.tree, &expected);
        }
        if problems.is_empty() {
            passed += 1;
        } else {
            failures.push(format!(
                "== {board}\n{}",
                problems
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    eprintln!(
        "kernel {arch}{}: {passed} match dtc, {} differ, {skipped} rejected by dtc",
        if symbols { " (-@)" } else { "" },
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{}",
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The cheap board scan (used to match blobs to boards) agrees with a full compile.
#[test]
fn kernel_board_scan() {
    let Ok(dir) = std::env::var("TIMBER_KERNEL_DIR") else {
        eprintln!("skipping: set TIMBER_KERNEL_DIR to a kernel checkout");
        return;
    };
    let root = PathBuf::from(dir);
    let arch = std::env::var("TIMBER_KERNEL_ARCH").unwrap_or_else(|_| "arm64".into());
    let dts_dir = format!("arch/{arch}/boot/dts");
    let vfs = load_dir(&root, &["arch", "include/dt-bindings"]);
    let resolver = timber_core::vfs::IncludeResolver::new(&vfs);
    let (mut agree, mut empty, mut differ) = (0, 0, Vec::new());
    for path in timber_core::vfs::Vfs::paths(&vfs)
        .filter(|p| p.starts_with(&dts_dir) && p.ends_with(".dts"))
    {
        let scanned = timber_core::bsp::scan_board_in(&vfs, &resolver, path);
        let compiled = dts::compile(&vfs, path).tree;
        let real: Vec<&str> = compiled.prop_strings(Tree::ROOT, "compatible");
        if scanned.compatible.is_empty() {
            empty += 1;
        } else if real.starts_with(
            &scanned
                .compatible
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ) {
            // Equal, or a .dtsi appended generic entries; matching uses the leading ones.
            agree += 1;
        } else {
            differ.push(format!(
                "{path}: scanned {:?} compiled {real:?}",
                scanned.compatible
            ));
        }
    }
    let total = agree + empty + differ.len();
    eprintln!(
        "board scan {arch}: {agree}/{total} agree, {empty} declare no root compatible, {} differ",
        differ.len()
    );
    for d in differ.iter().take(8) {
        eprintln!("  {d}");
    }
    assert!(
        agree * 100 >= total * 97,
        "scan agrees on fewer than 97% of boards"
    );
}
